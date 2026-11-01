//! 窗口与帧循环：eframe 入口、后台服务、封面加载运行时。
//!
//! UI 线程只做绘制与请求分发；播放、搜索、网络、封面解码都在后台线程，
//! 每帧不 await、不阻塞（首帧尚未拿到快照时先画空界面）。

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use lx_core::model::config::Config;
use voicefox_runtime::{ApplicationRuntime, CoverService};

use crate::controller::VoicefoxGuiController;
use crate::cover::{CoverLoader, CoverResults, RuntimeCoverLoader};
use crate::service::{self, AppService, Snapshot};
use crate::theme::GuiTheme;
use crate::ui::UiState;

/// 窗口创建失败（无显示服务等）时的可读错误。
#[derive(Debug)]
pub struct GuiStartError(pub String);

impl std::fmt::Display for GuiStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GuiStartError {}

/// 封面加载用的独立 runtime：网络与解码不能堵住播放服务线程。
fn cover_runtime() -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name("voicefox-gui-cover")
        .build()
        .ok()
}

/// 启动整个桌面 GUI：建服务、装载封面加载器、打开窗口。
pub fn run(config: Config, runtime: ApplicationRuntime) -> anyhow::Result<()> {
    let cover_service = Arc::clone(&runtime.cover);
    // 验收钩子：`VOICEFOX_GUI_AUTOSEARCH=<关键词>` 在窗口打开前就发起搜索，
    // 截图与冒烟测试可以免去模拟键盘输入。
    let mut startup = Vec::new();
    if let Ok(keyword) = std::env::var("VOICEFOX_GUI_AUTOSEARCH")
        && !keyword.trim().is_empty()
    {
        startup.push(crate::service::GuiRequest::Dispatch(
            voicefox_application::ApplicationCommand::Search {
                keyword: keyword.trim().to_string(),
                source: None,
            },
        ));
    }
    let service = service::spawn(runtime, startup);
    let cover_runtime = cover_runtime();
    let exit = service.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(crate::TITLE)
            .with_app_id("voicefox")
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([880.0, 560.0]),
        ..Default::default()
    };
    let app = VoicefoxGuiApp {
        controller: None,
        service,
        cover_service,
        cover_runtime: None,
        cover_loader: None,
        cover_results: Arc::new(std::sync::Mutex::new(Vec::new())),
        ui: UiState::default(),
        theme: GuiTheme::from_config(&config),
        #[cfg(not(target_arch = "wasm32"))]
        capture: capture::CaptureState::default(),
    };
    let result = eframe::run_native(
        crate::TITLE,
        options,
        Box::new(move |cc| Ok(Box::new(app.startup(cc, cover_runtime)))),
    );
    // 窗口关闭后请服务线程收尾（播放器与 MPRIS 一起停下）。
    let _ = exit.send(crate::service::GuiRequest::Quit);
    result.map_err(|error| anyhow::anyhow!("GUI 启动失败: {error}"))?;
    Ok(())
}

struct VoicefoxGuiApp {
    controller: Option<VoicefoxGuiController>,
    service: AppService,
    cover_service: Arc<CoverService>,
    /// 封面加载用的 tokio runtime，随窗口关闭一起释放。
    cover_runtime: Option<tokio::runtime::Runtime>,
    /// 封面加载运行时放在 egui 上下文里存活到窗口关闭。
    cover_loader: Option<Arc<dyn CoverLoader>>,
    cover_results: CoverResults,
    ui: UiState,
    theme: GuiTheme,
    #[cfg(not(target_arch = "wasm32"))]
    capture: capture::CaptureState,
}

/// 中文字体：egui 自带字体没有 CJK 字形，不装这个界面会全是方框。
///
/// 字体与 libmpv 同属原生依赖，不随仓库分发；构建前把 Noto Sans CJK 放到
/// `gui/assets/NotoSansCJK-Regular.ttc`（`scripts/fetch-gui-assets.sh` 会做这件事）。
/// 运行期也可以用 `VOICEFOX_GUI_FONT` 指向任意 `.ttc`/`.ttf`。
const CJK_FONT: &[u8] = include_bytes!("../assets/NotoSansCJK-Regular.ttc");

/// 字体加载失败时给用户的一句话。
pub const MISSING_FONT_HINT: &str = "未安装中文字体：请运行 scripts/fetch-gui-assets.sh，\
或用 VOICEFOX_GUI_FONT 指向一个 .ttc/.ttf，或在系统里安装 Noto Sans CJK";

/// 中文字体数据：优先 `VOICEFOX_GUI_FONT`，否则用构建时嵌入的那份。
fn cjk_font_data() -> std::sync::Arc<egui::FontData> {
    if let Ok(path) = std::env::var("VOICEFOX_GUI_FONT")
        && let Ok(bytes) = std::fs::read(&path)
    {
        return std::sync::Arc::new(egui::FontData::from_owned(bytes));
    }
    std::sync::Arc::new(egui::FontData::from_static(CJK_FONT))
}

/// 把中文字体接到比例与等宽字体的末尾（拉丁字母仍用 egui 默认字体）。
///
/// 返回 `false` 表示没有可用的中文字体，调用方应提示用户。
fn install_fonts(ctx: &egui::Context) -> bool {
    let mut fonts = egui::FontDefinitions::default();
    if std::env::var("VOICEFOX_GUI_FONT").is_ok_and(|path| !std::path::Path::new(&path).is_file()) {
        eprintln!("{MISSING_FONT_HINT}");
        ctx.set_fonts(fonts);
        return false;
    }
    fonts.font_data.insert("cjk".to_owned(), cjk_font_data());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    true
}

/// 装好中文字体（测试与演示程序用；生产路径在 [`VoicefoxGuiApp::startup`]）。
pub fn install_cjk_font(ctx: &egui::Context) {
    let _ = install_fonts(ctx);
}

impl VoicefoxGuiApp {
    fn startup(
        mut self,
        cc: &eframe::CreationContext<'_>,
        cover_runtime: Option<tokio::runtime::Runtime>,
    ) -> Self {
        if !install_fonts(&cc.egui_ctx) {
            eprintln!("{MISSING_FONT_HINT}");
        }
        let loader = Arc::new(RuntimeCoverLoader::new(
            Arc::clone(&self.cover_service),
            cc.egui_ctx.clone(),
        ));
        self.cover_results = loader.results();
        let loader_dyn: Arc<dyn CoverLoader> = loader;
        self.cover_loader = Some(Arc::clone(&loader_dyn));
        self.ui.tab = crate::ui::Tab::Search;
        self.ui.set_cover_loader(loader_dyn);
        let mut controller = VoicefoxGuiController::new(&cc.egui_ctx, self.service.clone());
        if let Ok(path) = std::env::var("VOICEFOX_GUI_INPUT") {
            match std::fs::File::open(&path) {
                Ok(file) => controller.listen_input(file),
                Err(error) => eprintln!("VOICEFOX_GUI_INPUT 打开失败: {path}: {error}"),
            }
        }
        self.controller = Some(controller);
        // 封面加载运行时必须活到窗口关闭：挂到 App 自身，随它一起析构。
        self.cover_runtime = cover_runtime;
        self
    }
}

impl eframe::App for VoicefoxGuiApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let base = self.theme.base;
        [
            base.r() as f32 / 255.0,
            base.g() as f32 / 255.0,
            base.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 开发/验收用的截图钩子：VOICEFOX_GUI_SCREENSHOT=<目录> 时每帧落一张
        // PNG，便于在没有截屏工具的会话里核对界面。
        #[cfg(not(target_arch = "wasm32"))]
        capture::maybe_capture(&ctx, frame, &mut self.capture);
        let snapshot: Snapshot = self.service.take_snapshot();
        let Some(controller) = self.controller.as_mut() else {
            return;
        };
        controller.apply_snapshot(snapshot);
        self.ui
            .show(ui, controller, self.theme, &self.cover_results);

        if self.service.quit_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let _ = frame;
        // 进度条以 ~4Hz 刷新；封面加载完成时加载器自己请求重绘。
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cover_runtime_can_be_built() {
        // 封面加载运行时是纯 CPU/IO，构建失败会让封面永远空着。
        let runtime = super::cover_runtime().expect("cover runtime");
        drop(runtime);
    }
}
#[cfg(not(target_arch = "wasm32"))]
mod capture {
    use std::io::Write;

    use eframe::egui;

    /// 截图钩子：设置 `VOICEFOX_GUI_SCREENSHOT` 为目录时，按固定节奏把窗口
    /// 内容写成 PNG（`latest.png` 始终是最新一张），便于在没有截屏工具的
    /// 会话里核对界面。只在设置了该环境变量时才起作用。
    pub fn maybe_capture(
        ctx: &egui::Context,
        _frame: &mut eframe::Frame,
        state: &mut CaptureState,
    ) {
        let Some(dir) = std::env::var_os("VOICEFOX_GUI_SCREENSHOT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        if std::fs::create_dir_all(&dir).is_err() || state.in_flight {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(last) = state.last
            && now.duration_since(last) < std::time::Duration::from_millis(500)
        {
            return;
        }
        state.last = Some(now);
        let target = dir.clone();
        let mut counter = state.counter;
        let repaint = ctx.clone();
        ctx.request_screenshot(move |image| {
            counter += 1;
            if let Some(buffer) = image::RgbaImage::from_raw(
                image.size[0] as u32,
                image.size[1] as u32,
                image.as_raw().to_vec(),
            ) {
                let mut out = std::io::Cursor::new(Vec::new());
                let dynamic = image::DynamicImage::ImageRgba8(buffer);
                if dynamic.write_to(&mut out, image::ImageFormat::Png).is_ok() {
                    let bytes = out.into_inner();
                    let _ = std::fs::write(target.join(format!("gui-{counter:05}.png")), &bytes);
                    let _ = std::fs::write(target.join("latest.png"), bytes);
                }
            }
            repaint.request_repaint();
        });
        state.in_flight = false;
    }

    #[derive(Default)]
    pub struct CaptureState {
        last: Option<std::time::Instant>,
        counter: u64,
        in_flight: bool,
    }

    #[allow(dead_code)]
    fn _ensure_traits(_: &mut dyn Write) {}
}
