//! 设置页的配置改动：走 `apply_config_change` → 写进运行时的 `Config` 并落盘。
//!
//! 用隔离的配置路径与一份假播放器，验证「界面改什么，config.toml 里就是什么」，
//! 与 TUI 共用同一份配置（`[ui].appearance` 等字段都在里面）。

use std::sync::Arc;

use lx_core::model::config::{Appearance, Config};
use lx_core::model::source::PlayerState;
use lx_core::model::source::Quality;
use lx_core::traits::player::PlayerBackend;
use voicefox_gui::service::{ConfigChange, apply_config_change};
use voicefox_runtime::ApplicationRuntime;

/// 最小假播放器：所有操作都成功但不做任何事。
struct FakePlayer;

impl PlayerBackend for FakePlayer {
    fn prepare(&self) -> u64 {
        0
    }
    fn play(&self, _url: &str, _generation: u64) -> bool {
        false
    }
    fn pause(&self) {}
    fn resume(&self) {}
    fn stop(&self) {}
    fn toggle(&self) {}
    fn seek(&self, _position: std::time::Duration) {}
    fn state_watcher(&self) -> tokio::sync::watch::Receiver<PlayerState> {
        let (sender, receiver) = tokio::sync::watch::channel(PlayerState::Stopped);
        std::mem::forget(sender);
        receiver
    }
    fn position_watcher(&self) -> tokio::sync::watch::Receiver<std::time::Duration> {
        let (sender, receiver) = tokio::sync::watch::channel(std::time::Duration::ZERO);
        std::mem::forget(sender);
        receiver
    }
    fn duration_watcher(&self) -> tokio::sync::watch::Receiver<std::time::Duration> {
        let (sender, receiver) = tokio::sync::watch::channel(std::time::Duration::ZERO);
        std::mem::forget(sender);
        receiver
    }
    fn take_event_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<lx_core::traits::player::PlayerEvent>> {
        None
    }
    fn volume(&self) -> u32 {
        100
    }
    fn set_volume(&self, _vol: u32) {}
    fn volume_up(&self, _delta: u32) {}
    fn volume_down(&self, _delta: u32) {}
}

fn runtime(tag: &str) -> (ApplicationRuntime, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("voicefox-gui-cfg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let runtime =
        ApplicationRuntime::from_parts(Config::default(), path.clone(), Arc::new(FakePlayer))
            .expect("构建运行时");
    (runtime, path)
}

#[test]
fn settings_changes_land_in_the_shared_config_file() {
    let (runtime, path) = runtime("quality");
    let apply = |change: ConfigChange| apply_config_change(&runtime, &change).expect("应用配置");

    apply(ConfigChange::Quality("flac24bit".into()));
    apply(ConfigChange::DownloadQuality("320k".into()));
    apply(ConfigChange::Volume(37));
    apply(ConfigChange::Theme("nord".into()));
    apply(ConfigChange::Appearance("light".into()));
    apply(ConfigChange::Keybinding {
        action: "GlobalNextTrack".into(),
        keys: "Ctrl+Right".into(),
    });

    // 内存里的配置已经变了。
    let memory = runtime.config_snapshot();
    assert_eq!(memory.player.quality, Quality::Flac24);
    assert_eq!(memory.download.quality, Some(Quality::High320));
    assert_eq!(memory.player.volume, 37);
    assert_eq!(memory.theme.name, "nord");
    assert_eq!(memory.ui.appearance, Appearance::Light);
    assert_eq!(
        memory
            .keybindings
            .global
            .get(&lx_core::keybinding::Action::GlobalNextTrack),
        Some(&"Ctrl+Right".to_string())
    );

    // 磁盘上的 config.toml 同步写好了（TUI 读同一份）。
    let text = std::fs::read_to_string(&path).expect("配置已落盘");
    let on_disk: Config = toml::from_str(&text).expect("配置可读回");
    assert_eq!(on_disk.player.quality, Quality::Flac24);
    assert_eq!(on_disk.ui.appearance, Appearance::Light);
    assert_eq!(on_disk.theme.name, "nord");
}

#[test]
fn invalid_theme_and_keys_are_rejected_without_writing() {
    let (runtime, path) = runtime("invalid");
    // 未知主题名：不落盘（界面会回滚到上一次的值）。
    let message = apply_config_change(&runtime, &ConfigChange::Theme("no-such-theme".into()))
        .expect("不写盘也算成功");
    assert!(message.is_empty(), "非法主题不产生说明");
    assert_eq!(
        runtime.config_snapshot().theme.name,
        Config::default().theme.name
    );

    // 非法键位：同样不写进配置。
    let message = apply_config_change(
        &runtime,
        &ConfigChange::Keybinding {
            action: "GlobalPlayPause".into(),
            keys: "NotAKey".into(),
        },
    )
    .expect("非法键位也要返回说明");
    assert!(message.contains("无效"), "应提示键位无效: {message}");
    assert_eq!(
        runtime
            .config_snapshot()
            .keybindings
            .global
            .get(&lx_core::keybinding::Action::GlobalPlayPause)
            .cloned(),
        lx_core::keybinding::KeybindingConfig::default()
            .global
            .get(&lx_core::keybinding::Action::GlobalPlayPause)
            .cloned()
    );
    let _ = path;
}

#[test]
fn download_quality_follow_clears_the_override() {
    let (runtime, _path) = runtime("follow");
    apply_config_change(&runtime, &ConfigChange::DownloadQuality("flac".into())).unwrap();
    assert_eq!(
        runtime.config_snapshot().download.quality,
        Some(Quality::Flac)
    );
    apply_config_change(&runtime, &ConfigChange::DownloadQuality("follow".into())).unwrap();
    assert_eq!(runtime.config_snapshot().download.quality, None);
}
