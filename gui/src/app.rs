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
use crate::service::{self, AppService, GuiRequest, Snapshot};
use crate::theme::GuiTheme;
use crate::tray::WindowCommands;
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
    // 托盘「设置」菜单项：只置一个标志，由 UI 线程切页签（托盘线程不碰 UI）。
    let show_settings = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // 托盘在服务线程里建：要 D-Bus 连接与配置写盘，UI 线程不做 IO。
    let window_commands = WindowCommands::new(Arc::clone(&show_settings));
    let mut initial_tab: Option<crate::ui::Tab> = None;
    startup.push(service::GuiRequest::StartTray { show_settings });
    // 验收钩子：`VOICEFOX_GUI_TAB=settings` 让窗口直接停在某一页（截图用）。
    if let Ok(tab) = std::env::var("VOICEFOX_GUI_TAB") {
        startup.push(service::GuiRequest::RefreshSettings);
        if tab.eq_ignore_ascii_case("settings") {
            initial_tab = Some(crate::ui::Tab::Settings);
        }
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
        resolved: (
            GuiTheme::from_config(&config),
            true,
            config.ui.appearance,
            config.theme.name.clone(),
        ),
        reported_dark: None,
        window_commands,
        initial_tab,
        tray_active: false,
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

/// 桌面实际呈现的深浅色：`[ui].appearance` 强制时以配置为准，
/// 「跟随系统」时读 egui 的系统主题通道（eframe 的 glow 后端每帧填充）。
///
/// 读不到系统主题（无桌面、Wayland 未上报）时按深色处理，
/// 与历史版本观感一致。
pub fn resolve_dark(forced: lx_core::model::config::Appearance, ctx: &egui::Context) -> bool {
    forced
        .forced_dark()
        .unwrap_or_else(|| !matches!(ctx.system_theme(), Some(egui::Theme::Light)))
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
    /// 当前生效的配色（明暗 + 具名主题），变化时重算。
    resolved: (GuiTheme, bool, lx_core::model::config::Appearance, String),
    /// 上一次报告的桌面深浅色：只在变化时通知服务线程。
    reported_dark: Option<bool>,
    /// 托盘请求的窗口命令（显示 / 隐藏、退出、打开设置），每帧消费。
    window_commands: WindowCommands,
    /// 启动时要停在哪一页（`VOICEFOX_GUI_TAB` 验收钩子）。
    initial_tab: Option<crate::ui::Tab>,
    /// 托盘是否真的注册上了（没有 D-Bus 时关闭窗口就是退出）。
    tray_active: bool,
    #[cfg(not(target_arch = "wasm32"))]
    capture: capture::CaptureState,
}

/// 中文字体：egui 自带字体没有 CJK 字形，不装这个界面会全是方框。
///
/// 字体与 libmpv 同属原生依赖，不随仓库分发：构建时把 Noto Sans CJK
/// （SIL OFL 1.1）放在 `gui/assets/NotoSansCJK-Regular.ttc`，由
/// `scripts/fetch-gui-assets.sh` 准备（CI 与本地构建都会先跑它）。
const CJK_FONT: &[u8] = include_bytes!("../assets/NotoSansCJK-Regular.ttc");

/// 把中文字体接到比例与等宽字体的末尾：拉丁字母与图标仍用 egui 默认字体，
/// 缺字时才回落到中文字体。
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(CJK_FONT)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// 装好中文字体（测试与演示程序用；生产路径在 [`VoicefoxGuiApp::startup`]）。
pub fn install_cjk_font(ctx: &egui::Context) {
    install_fonts(ctx);
}

impl VoicefoxGuiApp {
    fn startup(
        mut self,
        cc: &eframe::CreationContext<'_>,
        cover_runtime: Option<tokio::runtime::Runtime>,
    ) -> Self {
        install_fonts(&cc.egui_ctx);
        let loader = Arc::new(RuntimeCoverLoader::new(
            Arc::clone(&self.cover_service),
            cc.egui_ctx.clone(),
        ));
        self.cover_results = loader.results();
        let loader_dyn: Arc<dyn CoverLoader> = loader;
        self.cover_loader = Some(Arc::clone(&loader_dyn));
        self.ui.tab = self.initial_tab.unwrap_or(crate::ui::Tab::Search);
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

impl VoicefoxGuiApp {
    /// 每帧按 `[ui].appearance` 与桌面深浅色重算配色。
    ///
    /// 只有「强制/Auto 的结果」真的变化时才重算调色板：主题库解析一次
    /// 要遍历全部具名主题，不该每帧都做。
    fn apply_appearance(&mut self, ctx: &egui::Context, controller: &VoicefoxGuiController) {
        let config = controller.config();
        let appearance = config.ui.appearance;
        let dark = resolve_dark(appearance, ctx);
        let name = config.theme.name.clone();
        let changed =
            self.resolved.1 != dark || self.resolved.2 != appearance || self.resolved.3 != name;
        if changed {
            self.theme = GuiTheme::for_appearance(config, dark);
            self.resolved = (self.theme, dark, appearance, name);
            self.reported_dark = None;
        }
        // 桌面主题翻转后 egui 的默认视觉风格也要跟着换（未覆盖的部件）。
        if self.reported_dark != Some(dark) {
            self.reported_dark = Some(dark);
            let mut visuals = self.theme.visuals_with(dark);
            visuals.dark_mode = dark;
            ctx.set_visuals(visuals);
        }
    }

    /// GUI 兑现的全局快捷键子集（其余键位原样留给 TUI）。
    ///
    /// 只在没有文本输入框获得焦点时触发，否则会吞掉搜索框里的按键
    /// （例如「/」本来就是进入搜索的键位）。
    fn apply_keybindings(&mut self, ctx: &egui::Context, controller: &mut VoicefoxGuiController) {
        use lx_core::keybinding::{Action, parse_keybinding};
        use voicefox_application::ApplicationCommand;

        if ctx.memory(|memory| memory.focused().is_some()) {
            return;
        }
        // 本帧按下且没有重复触发的按键 + 当时的修饰键。
        let pressed: Vec<(egui::Key, egui::Modifiers)> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        if pressed.is_empty() {
            return;
        }
        let mut requests: Vec<GuiRequest> = Vec::new();
        for action in crate::service::GUI_ACTIONS {
            let Some(text) = controller.keybinding(action) else {
                continue;
            };
            let Some(binding) = parse_keybinding(&text) else {
                continue;
            };
            let gesture = binding.gesture();
            let hit = pressed
                .iter()
                .any(|(key, modifiers)| keybinding_matches(gesture, *key, *modifiers));
            if !hit {
                continue;
            }
            requests.push(match action {
                Action::GlobalPlayPause => GuiRequest::Dispatch(ApplicationCommand::Toggle),
                Action::GlobalNextTrack => GuiRequest::Dispatch(ApplicationCommand::Next),
                Action::GlobalPrevTrack => GuiRequest::Dispatch(ApplicationCommand::Previous),
                Action::SearchInputMode => {
                    self.ui.tab = crate::ui::Tab::Search;
                    GuiRequest::RefreshSettings
                }
                _ => continue,
            });
        }
        for request in requests {
            controller.send(request);
        }
    }
}

/// 核心键位（与后端无关的 `KeyGesture`）与 egui 键事件是否一致。
fn keybinding_matches(
    gesture: lx_core::keybinding::KeyGesture,
    key: egui::Key,
    pressed: egui::Modifiers,
) -> bool {
    let mut wanted = egui::Modifiers::NONE;
    if gesture.ctrl {
        wanted |= egui::Modifiers::CTRL;
        wanted |= egui::Modifiers::COMMAND;
    }
    if gesture.alt {
        wanted |= egui::Modifiers::ALT;
    }
    // Shift 只在特殊键上参与比较：核心把 `Shift+a` 归一成 `A`，
    // 打字时的 Shift 已体现在字符本身。
    if gesture.character.is_none() && gesture.shift {
        wanted |= egui::Modifiers::SHIFT;
    }
    if !pressed.matches_logically(wanted) {
        return false;
    }
    match (gesture.character, gesture.special) {
        (Some(character), _) => {
            let typed = match key {
                egui::Key::Space => Some(' '),
                egui::Key::Slash => Some('/'),
                other => other.name().chars().next(),
            };
            typed.is_some_and(|typed| typed == character || typed.eq_ignore_ascii_case(&character))
        }
        (None, Some(special)) => key.name() == special,
        _ => false,
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
        let Some(mut controller) = self.controller.take() else {
            return;
        };
        controller.apply_snapshot(snapshot);
        controller.tick_refresh();
        // 外观与键位都要读控制器快照，先处理再借用一次画界面。
        self.apply_appearance(&ctx, &controller);
        self.apply_keybindings(&ctx, &mut controller);
        if self.window_commands.take_show_settings() {
            self.ui.tab = crate::ui::Tab::Settings;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.window_commands.take_toggle() {
            let hidden = ctx.input(|input| input.viewport().visible()) == Some(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(hidden));
            if hidden {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        self.ui
            .show(ui, &mut controller, self.theme, &self.cover_results);
        controller.send(GuiRequest::ReportAppearance {
            dark: self.resolved.1,
        });
        self.controller = Some(controller);

        // 托盘是否可用由服务线程在启动时判断，这里只读结果。
        let active = crate::service::tray_active() || self.tray_active;
        // 关闭按钮 → 最小化到托盘：取消这次关闭，改成隐藏窗口。
        // 真正的退出只由托盘「退出」/ MPRIS `Quit` / 服务线程退出触发。
        if ctx.input(|input| input.viewport().close_requested())
            && active
            && !self.window_commands.quit_requested()
            && !self.service.quit_requested()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        } else if self.service.quit_requested() || self.window_commands.quit_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let _ = frame;
        // 进度条以 ~4Hz 刷新；封面加载完成时加载器自己请求重绘。
        ctx.request_repaint_after(Duration::from_millis(250));
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

#[cfg(test)]
mod tests {
    #[test]
    fn cover_runtime_can_be_built() {
        // 封面加载运行时是纯 CPU/IO，构建失败会让封面永远空着。
        let runtime = super::cover_runtime().expect("cover runtime");
        drop(runtime);
    }
}
