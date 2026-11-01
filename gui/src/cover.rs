//! 封面缓存：把歌曲封面 URL / 本地路径落地成 egui 纹理。
//!
//! 网络请求与解码在服务线程的 tokio 阻塞池里跑，结果回到 UI 线程后
//! 由 [`CoverCache::apply`] 变成纹理；egui 每帧只看缓存，不 await。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{Context, TextureHandle, TextureOptions};
use voicefox_runtime::CoverService;

/// 最多缓存多少张封面（超出时清掉最早的那张）。
const CACHE_CAPACITY: usize = 24;

/// 封面纹理尺寸上限：原图交给 GPU 会浪费显存，缩略图也够用。
const MAX_COVER_SIDE: u32 = 512;

/// 一张已加载的封面。
#[derive(Clone)]
pub enum CoverImage {
    Ready(TextureHandle),
    Failed,
}

/// 服务线程解码完成、等待 UI 线程转成纹理的结果。
pub type CoverResults = Arc<Mutex<Vec<(String, Option<image::RgbaImage>)>>>;

/// 按 URL 缓存的封面纹理（UI 线程独占）。
#[derive(Default)]
pub struct CoverCache {
    entries: HashMap<String, CoverImage>,
    order: Vec<String>,
    pending: std::collections::HashSet<String>,
    loader: Option<Arc<dyn CoverLoader>>,
}

impl CoverCache {
    pub fn get(&self, url: &str) -> Option<CoverImage> {
        self.entries.get(url).cloned()
    }

    /// 请求加载一张封面；已缓存或已在途时什么也不做。
    pub fn request(&mut self, url: &str) {
        if url.is_empty() || self.entries.contains_key(url) || self.pending.contains(url) {
            return;
        }
        self.pending.insert(url.to_string());
        if let Some(loader) = self.loader.clone() {
            loader.load(url.to_string());
        }
    }

    /// 设置加载器（由启动方提供；桌面端用 [`RuntimeCoverLoader`]）。
    pub fn with_loader(mut self, loader: Arc<dyn CoverLoader>) -> Self {
        self.loader = Some(loader);
        self
    }

    /// 每帧调用：新的加载结果转成纹理，并为已知 URL 确保加载已排队。
    pub fn sync(&mut self, ctx: &Context, results: &CoverResults, wanted: &[String]) -> bool {
        let mut changed = self.drain(ctx, results);
        for url in wanted {
            if !url.is_empty() && !self.entries.contains_key(url) && !self.pending.contains(url) {
                self.request(url);
                changed = true;
            }
        }
        changed
    }

    /// 把服务线程的结果放进纹理缓存；返回是否新增了纹理。
    pub fn drain(&mut self, ctx: &Context, results: &CoverResults) -> bool {
        let drained: Vec<_> = match results.lock() {
            Ok(mut guard) => guard.drain(..).collect(),
            Err(_) => Vec::new(),
        };
        let mut changed = false;
        for (url, rgba) in drained {
            self.pending.remove(&url);
            let image = match rgba {
                Some(buffer) => {
                    let buffer = downscale(buffer);
                    let size = [buffer.width() as usize, buffer.height() as usize];
                    let pixels = buffer.into_raw();
                    let color_image =
                        eframe::egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                    CoverImage::Ready(ctx.load_texture(
                        format!("cover:{url}"),
                        color_image,
                        TextureOptions::LINEAR,
                    ))
                }
                None => CoverImage::Failed,
            };
            self.insert(url, image);
            changed = true;
        }
        changed
    }

    fn insert(&mut self, url: String, image: CoverImage) {
        if !self.entries.contains_key(&url) {
            self.order.push(url.clone());
            if self.order.len() > CACHE_CAPACITY {
                let evicted = self.order.remove(0);
                self.entries.remove(&evicted);
            }
        }
        self.entries.insert(url, image);
    }
}

impl Clone for CoverCache {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries.clone(),
            order: self.order.clone(),
            pending: self.pending.clone(),
            loader: self.loader.clone(),
        }
    }
}

/// 封面加载器：按 URL 解码成 RGBA。
pub trait CoverLoader: Send + Sync + 'static {
    fn load(&self, url: String);
}

/// 基于 `voicefox-runtime` CoverService 的加载器。
///
/// `controller` 只在网络成功后请求一次重绘；请求本身在阻塞池里跑，
/// UI 线程永远不会等网络。
pub struct RuntimeCoverLoader {
    service: Arc<CoverService>,
    controller: Option<Context>,
    sink: CoverResults,
    request: Arc<dyn Fn(String) + Send + Sync>,
}

impl RuntimeCoverLoader {
    pub fn new(service: Arc<CoverService>, controller: Context) -> Self {
        Self {
            service,
            controller: Some(controller),
            sink: Arc::new(Mutex::new(Vec::new())),
            request: Arc::new(|_| {}),
        }
    }

    /// 服务线程里构造：网络请求直接投给服务线程的 runtime，不必新起线程池。
    pub fn on_service_thread(
        service: Arc<CoverService>,
        request: impl Fn(String) + Send + Sync + 'static,
    ) -> Self {
        Self {
            service,
            controller: None,
            sink: Arc::new(Mutex::new(Vec::new())),
            request: Arc::new(request),
        }
    }

    pub fn results(&self) -> CoverResults {
        Arc::clone(&self.sink)
    }
}

impl CoverLoader for RuntimeCoverLoader {
    fn load(&self, url: String) {
        let service = Arc::clone(&self.service);
        let sink = Arc::clone(&self.sink);
        let controller = self.controller.clone();
        (self.request)(url.clone());
        tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let decoded = runtime
                .ok()
                .and_then(|runtime| runtime.block_on(decode(&service, &url)));
            if let Ok(mut guard) = sink.lock() {
                guard.push((url, decoded));
            }
            if let Some(controller) = controller {
                controller.request_repaint();
            }
        });
    }
}

async fn decode(service: &CoverService, url: &str) -> Option<image::RgbaImage> {
    let path = service
        .cache_path(Some(url.to_string()))
        .await
        .ok()
        .flatten()?;
    let bytes = std::fs::read(path).ok()?;
    image::load_from_memory(&bytes)
        .ok()
        .map(|image| image.to_rgba8())
}

/// 把任意尺寸封面缩到 GPU 友好的大小（等比）。
pub fn downscale(buffer: image::RgbaImage) -> image::RgbaImage {
    if buffer.width() <= MAX_COVER_SIDE && buffer.height() <= MAX_COVER_SIDE {
        return buffer;
    }
    let scale = MAX_COVER_SIDE as f32 / buffer.width().max(buffer.height()) as f32;
    let width = (buffer.width() as f32 * scale).round().max(1.0) as u32;
    let height = (buffer.height() as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(
        &buffer,
        width,
        height,
        image::imageops::FilterType::Triangle,
    )
}

pub fn load_decoded(service: &CoverService, url: &str) -> Option<image::RgbaImage> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(decode(service, url))
}

#[cfg(test)]
mod tests {
    use super::{CoverCache, CoverImage, CoverLoader, downscale};
    use std::sync::{Arc, Mutex};

    struct RecordingLoader(Mutex<Vec<String>>);

    impl CoverLoader for RecordingLoader {
        fn load(&self, url: String) {
            self.0.lock().unwrap().push(url);
        }
    }

    #[test]
    fn requests_are_deduplicated_while_pending() {
        let loader = Arc::new(RecordingLoader(Mutex::new(Vec::new())));
        let mut cache = CoverCache::default().with_loader(loader.clone());
        cache.request("https://example.com/a.jpg");
        cache.request("https://example.com/a.jpg");
        cache.request("");
        assert_eq!(loader.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn cache_evicts_oldest_beyond_capacity() {
        let mut cache = CoverCache::default();
        for index in 0..super::CACHE_CAPACITY + 3 {
            cache.insert(format!("u{index}"), CoverImage::Failed);
        }
        assert_eq!(cache.order.len(), super::CACHE_CAPACITY);
        assert!(cache.get("u0").is_none());
        assert!(
            cache
                .get(&format!("u{}", super::CACHE_CAPACITY + 2))
                .is_some()
        );
    }

    #[test]
    fn large_covers_are_downscaled() {
        let resized = downscale(image::RgbaImage::new(1024, 768));
        assert_eq!((resized.width(), resized.height()), (512, 384));
    }

    #[test]
    fn small_covers_are_left_alone() {
        assert_eq!(downscale(image::RgbaImage::new(300, 300)).width(), 300);
    }
}
