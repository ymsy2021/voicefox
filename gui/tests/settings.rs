//! P2 设置页 / 外观 / 托盘：读写都落在已有的 `Config` 上，没有 GUI 专属配置。

use lx_core::model::config::{Appearance, UiConfig};
use lx_core::model::source::Quality;
use voicefox_application::ApplicationCommand;
use voicefox_gui::app::resolve_dark;
use voicefox_gui::service::{
    GUI_ACTIONS, parse_action, quality_from_config, quality_from_str, validate_keys,
};
use voicefox_gui::tray::{TrayAction, WindowCommands, request_for};

#[test]
fn quality_names_round_trip_through_config() {
    for quality in [
        Quality::Low128,
        Quality::High320,
        Quality::Flac,
        Quality::Flac24,
    ] {
        let name = quality_from_config(quality);
        assert_eq!(quality_from_str(&name), Some(quality), "音质名往返");
    }
    assert_eq!(
        quality_from_str("FLAC"),
        Some(Quality::Flac),
        "大小写不敏感"
    );
    assert_eq!(quality_from_str("bogus"), None, "非法音质被拒绝");
}

#[test]
fn keybinding_validation_rejects_garbage_and_accepts_real_keys() {
    for good in ["Space", "Ctrl+l", "n", "/", "F5", "Shift+Tab"] {
        assert!(validate_keys(good).is_ok(), "{good} 应被接受");
    }
    for bad in ["", "Ctrl+", "NotAKey"] {
        assert!(validate_keys(bad).is_err(), "{bad} 应被拒绝");
    }
    // 设置页列出的动作必须都能解析回来（否则改了存不进配置）。
    for action in GUI_ACTIONS {
        let debug = format!("{action:?}");
        assert_eq!(parse_action(&debug), Some(action), "{debug} 应能解析");
    }
    assert!(parse_action("NotAnAction").is_none());
}

#[test]
fn appearance_is_part_of_the_shared_config() {
    // 默认「跟随系统」，并且能序列化回 config.toml（TUI 读同一份）。
    let config = lx_core::model::config::Config::default();
    assert_eq!(config.ui.appearance, Appearance::System);
    let text = toml::to_string_pretty(&config).expect("配置可序列化");
    let parsed: lx_core::model::config::Config = toml::from_str(&text).expect("配置可反序列化");
    assert_eq!(parsed.ui.appearance, Appearance::System);
    // 旧配置没有该字段时按默认值补齐。
    let old: UiConfig = toml::from_str("enable_mouse = true").expect("旧配置可读");
    assert_eq!(old.appearance, Appearance::System);
    assert_eq!(Appearance::parse("light"), Appearance::Light);
    assert_eq!(Appearance::parse("DARK"), Appearance::Dark);
    assert_eq!(Appearance::parse("nonsense"), Appearance::System);
    assert_eq!(Appearance::Light.forced_dark(), Some(false));
    assert_eq!(Appearance::System.forced_dark(), None);
}

#[test]
fn forced_appearance_wins_over_the_desktop() {
    let ctx = egui::Context::default();
    // 「跟随系统」在测试环境拿不到系统主题：按深色处理（与历史观感一致）。
    assert!(resolve_dark(Appearance::System, &ctx));
    // 强制项无视桌面：不再看 egui 的系统主题通道。
    assert!(!resolve_dark(Appearance::Light, &ctx));
    assert!(resolve_dark(Appearance::Dark, &ctx));
}

#[test]
fn tray_menu_actions_map_to_the_same_playback_commands() {
    assert!(matches!(
        request_for(TrayAction::Next),
        Some(voicefox_gui::GuiRequest::Dispatch(ApplicationCommand::Next))
    ));
    assert!(matches!(
        request_for(TrayAction::Previous),
        Some(voicefox_gui::GuiRequest::Dispatch(
            ApplicationCommand::Previous
        ))
    ));
    assert!(matches!(
        request_for(TrayAction::TogglePlayback),
        Some(voicefox_gui::GuiRequest::Dispatch(
            ApplicationCommand::Toggle
        ))
    ));
    // 窗口类动作由 UI 线程执行，不发播放命令。
    assert!(request_for(TrayAction::ToggleWindow).is_none());
    assert!(request_for(TrayAction::Quit).is_none());
}

#[test]
fn window_commands_are_one_shot() {
    let show_settings = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let commands = WindowCommands::new(show_settings.clone());
    assert!(!commands.take_toggle());
    commands.toggle_visible();
    assert!(commands.take_toggle(), "托盘「显示/隐藏」只消费一次");
    assert!(!commands.take_toggle(), "消费之后不再触发");
    assert!(!commands.quit_requested());
    commands.request_quit();
    assert!(commands.quit_requested());
    show_settings.store(true, std::sync::atomic::Ordering::Release);
    assert!(commands.take_show_settings());
    assert!(!commands.take_show_settings());
}
