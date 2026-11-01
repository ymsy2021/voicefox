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
    /// 已经淘汰过的 URL：不再自动重试。
    ///
    /// 搜索一次可能返回几百条结果，远多于 [`CACHE_CAPACITY`]。若淘汰后允许
    /// 重新请求，被淘汰的那张会在下一帧变回「既不在 entries 也不在 pending」，
    /// 于是再次下载、再次上传纹理、再次淘汰 —— 每帧几十次网络与纹理上传，
    /// UI 线程被淹没，表现为界面点不动。这里记住淘汰项，只允许用户显式重试。
    evicted: std::collections::HashSet<String>,
    loader: Option<Arc<dyn CoverLoader>>,
    /// 本地生成的纹理（二维码等），与封面共用容量与淘汰策略。
    generated: HashMap<String, TextureHandle>,
}

impl CoverCache {
    pub fn get(&self, url: &str) -> Option<CoverImage> {
        self.entries.get(url).cloned()
    }

    /// 二维码纹理：`payload` 是链接（本地编码）或 base64 图片；同 key 复用纹理。
    ///
    /// 编码/解码只在内容变化时做一次：扫码轮询每秒更新状态，URL 不变时
    /// 不重复生成。图片渠道失败（base64 损坏）时返回 `None`，界面只显示状态文字。
    pub fn qr_texture(&mut self, ctx: &Context, key: &str, payload: &str) -> Option<TextureHandle> {
        if let Some(texture) = self.generated.get(key) {
            return Some(texture.clone());
        }
        let image = qr_code_image(payload)?;
        let size = [image.width() as usize, image.height() as usize];
        let color_image = eframe::egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        let texture = ctx.load_texture(format!("qr:{key}"), color_image, TextureOptions::NEAREST);
        if self.generated.len() >= CACHE_CAPACITY {
            self.generated.clear();
        }
        self.generated.insert(key.to_string(), texture.clone());
        Some(texture)
    }

    /// 请求加载一张封面；已缓存、已在途或已被淘汰时什么也不做。
    pub fn request(&mut self, url: &str) {
        if url.is_empty()
            || self.entries.contains_key(url)
            || self.pending.contains(url)
            || self.evicted.contains(url)
        {
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
        // `drain` 每帧只处理有限几张；若队列里还有积压，必须请求下一帧，
        // 否则界面空闲下来后剩下的封面永远排不空。
        let backlog = results
            .lock()
            .map(|guard| !guard.is_empty())
            .unwrap_or(false);
        if backlog {
            ctx.request_repaint();
        }
        for url in wanted {
            if !url.is_empty()
                && !self.entries.contains_key(url)
                && !self.pending.contains(url)
                && !self.evicted.contains(url)
            {
                self.request(url);
                changed = true;
            }
        }
        changed
    }

    /// 每帧最多把几张解码结果转成纹理。
    ///
    /// 一次搜索可能返回上百条结果，解码后的封面会在同一瞬间涌入队列。
    /// 若在一个 UI 帧里全部 `downscale` + 转 `ColorImage` + 上传 GPU
    /// （190 张 500×500 就是 ~181MB），这一帧会卡住好几秒，表现为
    /// 「搜索完界面短暂无响应」。分摊到多帧后每帧只做一点点，界面始终可交互。
    const TEXTURES_PER_FRAME: usize = 2;

    /// 把服务线程的结果放进纹理缓存；返回是否新增了纹理。
    pub fn drain(&mut self, ctx: &Context, results: &CoverResults) -> bool {
        // 只取有限条，剩下的留在队列里下一帧继续处理。
        let drained: Vec<_> = match results.lock() {
            Ok(mut guard) => {
                let take = guard.len().min(Self::TEXTURES_PER_FRAME);
                guard.drain(..take).collect()
            }
            Err(_) => Vec::new(),
        };
        let mut changed = false;
        for (url, rgba) in drained {
            self.pending.remove(&url);
            // 缩放已在解码线程里做完（见 `decode`），这里只做廉价的颜色转换
            // 与纹理上传。以前 `downscale` 在这里跑：一张 1200² 封面用
            // Triangle 滤波要约 530ms，190 张就会把 UI 线程锁死好几秒，
            // 表现为「搜索完界面短暂无响应」。
            let image = match rgba {
                Some(buffer) => {
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
                // 记住淘汰项：否则下一帧它又会被当作「从未请求」重新下载，
                // 在结果数远超容量时空转（见字段注释）。
                self.evicted.insert(evicted);
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
            evicted: self.evicted.clone(),
            loader: self.loader.clone(),
            generated: self.generated.clone(),
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
        // 共享一个解码 runtime。
        //
        // 以前这里对**每张封面**调用 `Builder::new_current_thread().build()`，
        // 也就是每个 URL 都新建一套 epoll / timer / IO driver 线程。搜索结果
        // 动辄上百条时，瞬间创建上百个 runtime 和数百个线程，CPU 全耗在建/销毁
        // runtime 上，UI 线程连一帧都跑不完 —— 表现为「搜完就卡住」。
        //
        // 并发上限交给 tokio 的阻塞池（默认 512，远大于同时在途的封面数），
        // 关键是**不再重复创建 runtime**。
        tokio::task::spawn_blocking(move || {
            let decoded = decode_runtime().block_on(decode(&service, &url));
            if let Ok(mut guard) = sink.lock() {
                guard.push((url, decoded));
            }
            if let Some(controller) = controller {
                controller.request_repaint();
            }
        });
    }
}

/// 共享的解码 runtime（只在首次使用时创建一次）。
fn decode_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("voicefox-gui-cover-decode")
            .build()
            .expect("cover decode runtime")
    })
}

async fn decode(service: &CoverService, url: &str) -> Option<image::RgbaImage> {
    let path = service
        .cache_path(Some(url.to_string()))
        .await
        .ok()
        .flatten()?;
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    // 缩放在解码线程里做（它是 CPU 重活，不能留在 UI 线程）。
    Some(downscale(image))
}

/// 二维码 payload → RGBA：base64 图片直接解码，链接用 `qrcode` 本地编码。
pub fn qr_code_image(payload: &str) -> Option<image::RgbaImage> {
    use base64::Engine;

    // `data:image/png;base64,...` 取逗号后的部分；纯 base64 直接用。
    let body = payload
        .split_once(',')
        .filter(|(header, _)| header.contains(";base64"))
        .map_or(payload, |(_, body)| body);
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(body.trim())
        && let Ok(image) = image::load_from_memory(&decoded)
    {
        return Some(image.to_rgba8());
    }
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let width = code.width();
    let module: u32 = 8;
    let quiet: u32 = 2;
    let side = (width as u32 + quiet * 2) * module;
    Some(image::RgbaImage::from_fn(side, side, |x, y| {
        let mx = (x / module).saturating_sub(quiet) as usize;
        let my = (y / module).saturating_sub(quiet) as usize;
        let dark = x / module >= quiet
            && y / module >= quiet
            && mx < width
            && my < width
            && code[(mx, my)] == qrcode::Color::Dark;
        if dark {
            image::Rgba([17, 17, 27, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    }))
}

/// 把任意尺寸封面缩到 GPU 友好的大小（等比）。
///
/// 这是纯 CPU 的活，只应在解码线程里调用（见 [`decode`]），不要放到 UI
/// 线程上：1200² → 512² 用 Triangle 滤波单张就要 ~530ms。
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
    use super::{CoverCache, CoverImage, CoverLoader, CoverResults, downscale, qr_code_image};
    use crate::cover::Context;
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
    fn qr_payloads_encode_to_square_images() {
        // 链接（网易云/酷狗等渠道）在本地用 qrcode 编码。
        let image = qr_code_image("https://example.com/login?key=abc").expect("链接可编码");
        assert_eq!(image.width(), image.height());
        assert!(image.width() >= 100, "二维码应有足够的分辨率");
        // 而不是二维码的普通文本也不应 panic（编码失败返回 None 或可编码）。
        let _ = qr_code_image("plain text");
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

    /// 回归用例：结果数远超缓存容量时，淘汰项不能被无限重新请求。
    ///
    /// 修复前 189 个 URL / 24 容量跑 200 帧会发出 3 万多次请求（每帧都在重新
    /// 下载并上传纹理），UI 线程被淹没，表现为「界面点不动」。
    #[test]
    fn evicted_covers_are_not_requested_again() {
        let loader = Arc::new(RecordingLoader(Mutex::new(Vec::new())));
        let mut cache =
            CoverCache::default().with_loader(Arc::clone(&loader) as Arc<dyn CoverLoader>);
        let watched: Vec<String> = (0..189).map(|i| format!("u{i}")).collect();

        // 模拟：结果逐步返回，每轮把上一轮请求过的都算作加载成功。
        // 直接用 `drain` 灌结果，避免依赖 egui 纹理。
        let results: CoverResults = Arc::new(Mutex::new(Vec::new()));
        let mut completed: Vec<String> = Vec::new();
        for _ in 0..200 {
            for url in completed.drain(..) {
                results
                    .lock()
                    .unwrap()
                    .push((url, Some(image::RgbaImage::new(2, 2))));
            }
            let ctx = Context::default();
            let mut new_changed = false;
            for url in &watched {
                if !cache.entries.contains_key(url)
                    && !cache.pending.contains(url)
                    && !cache.evicted.contains(url)
                {
                    cache.request(url);
                    new_changed = true;
                }
            }
            cache.drain(&ctx, &results);
            let _ = new_changed;
            completed = loader.0.lock().unwrap().clone();
        }

        let total = loader.0.lock().unwrap().len();
        // 每个 URL 最多请求一次：189 条结果就该只有 ~189 次请求。
        assert_eq!(
            total, 189,
            "每个封面只应请求一次，实际 {total} 次（存在淘汰后重复请求）"
        );
    }
}
