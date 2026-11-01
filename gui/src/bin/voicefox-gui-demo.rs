//! 演示程序：用假后端跑真实的 GUI 绘制，不需要网络、音频设备或登录。
//!
//! `cargo run -p voicefox-gui --bin voicefox-gui-demo` 打开可交互的演示窗口；
//! 无窗口的逐帧截图由 `tests/storyboard.rs` 负责（那里能用 egui_kittest）。

use std::sync::Arc;

use voicefox_gui::VoicefoxGuiController;
use voicefox_gui::demo::DemoBackend;
use voicefox_gui::theme::GuiTheme;
use voicefox_gui::ui::UiState;

fn main() {
    interactive();
}

fn interactive() {
    let backend = Arc::new(DemoBackend::new());
    backend.drive();
    let theme = GuiTheme::from_config(&lx_core::model::config::Config::default());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Voicefox 演示模式")
            .with_app_id("voicefox-demo")
            .with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };
    let app = InteractiveApp {
        controller: None,
        backend: Some(Arc::clone(&backend)),
        ui: UiState::default(),
        theme,
        cover_results: Arc::new(std::sync::Mutex::new(Vec::new())),
    };
    if let Err(error) = eframe::run_native(
        "Voicefox 演示模式",
        options,
        Box::new(move |cc| Ok(Box::new(app.startup(cc)))),
    ) {
        eprintln!("演示窗口启动失败: {error}");
    }
}

struct InteractiveApp {
    controller: Option<VoicefoxGuiController>,
    backend: Option<Arc<DemoBackend>>,
    ui: UiState,
    theme: GuiTheme,
    cover_results: voicefox_gui::cover::CoverResults,
}

impl InteractiveApp {
    fn startup(mut self, cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(backend) = self.backend.take() {
            self.controller = Some(VoicefoxGuiController::with_backend(
                &cc.egui_ctx,
                Box::new(backend),
            ));
        }
        self
    }
}

impl eframe::App for InteractiveApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let Some(controller) = self.controller.as_mut() else {
            return;
        };
        let snapshot = controller.backend().take_snapshot();
        controller.apply_snapshot(snapshot);
        let theme = self.theme;
        self.ui.show_in(ui, controller, theme, &self.cover_results);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
