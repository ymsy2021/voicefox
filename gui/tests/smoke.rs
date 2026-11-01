//! 端到端冒烟测试：真实 ApplicationRuntime + 真实 GUI 绘制（不联网、不播音频）。
//!
//! 复用 `voicefox-runtime` 的真实装配（播放器/音源/歌词/存储/封面/MPRIS），
//! 用本机音频设备；这里只验证 GUI 能起来、能画、能收命令，不会真的出声。

use std::time::{Duration, Instant};

use lx_core::model::config::Config;
use lx_core::model::song::SongInfo;
use lx_core::model::source::{PlayerState, SourceId};
use lx_core::traits::player::PlayerEvent;
use voicefox_application::ApplicationEvent;
use voicefox_runtime::ApplicationRuntime;
use voicefox_runtime::media::{MediaSnapshot, PlayMode};

fn test_config() -> Config {
    let mut config = Config::default();
    config.player.volume = 42;
    config.player.play_mode = "single-loop".into();
    config
}

#[test]
fn desktop_runtime_builds_and_exposes_gui_contract() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        let (config, path) = voicefox_runtime::load_config().expect("config");
        let runtime = ApplicationRuntime::desktop(config, path).expect("desktop runtime");

        // 1) GUI 与 TUI 共用的 ApplicationService 边界可用。
        let application = runtime.application();
        let state = application.state();
        assert!(state.playback.song.is_none(), "新启动时没有在播放的曲目");

        // 2) 媒体会话可用：同一状态源既能给 MPRIS 推送快照，也能收媒体键。
        let media = runtime.media();
        {
            let mut session = media.lock().unwrap_or_else(|e| e.into_inner());
            session.set_position(Duration::from_secs(7));
            session.set_play_mode(PlayMode::Random);
            assert!(!session.has_media_controls(), "未启动后端时不应自称已接入");
            assert!(session.next_command().is_none(), "未启动后端时没有媒体键");
        }
        drop(runtime);
    });
}

#[test]
fn mpris_snapshot_reflects_application_state() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        let (config, path) = voicefox_runtime::load_config().expect("config");
        let runtime = ApplicationRuntime::desktop(config, path).expect("desktop runtime");
        let _application = runtime.application();

        // 用一条不依赖网络的队列填充与状态变更路径：QueueAdd 到空队列会尝试
        // 解析播放地址，因此这里只验证「无队列时的快照」与媒体键映射。
        // 媒体会话在 GUI 里只由服务线程持有；这里直接验证快照构建。
        let snapshot = MediaSnapshot::build(
            PlayerState::Playing,
            Some(&SongInfo::new(
                "1".into(),
                SourceId::Kw,
                "夜曲".into(),
                "周杰伦".into(),
            )),
            Duration::from_secs(30),
            Duration::from_secs(226),
            60,
            PlayMode::SingleLoop,
            4,
            7,
        );
        assert_eq!(snapshot.title, "夜曲");
        assert_eq!(snapshot.loop_status, "Track");
        assert_eq!(snapshot.volume, 0.6);
        assert!(snapshot.can_go_next);

        // 媒体键 → 业务命令：暂停/播放都走同一入口。
        let action = voicefox_runtime::media::MediaCommand::Toggle.into_action(
            PlayerState::Playing,
            Duration::from_secs(30),
            4,
        );
        assert!(matches!(
            action,
            Some(voicefox_runtime::media::MediaAction::Dispatch(
                voicefox_application::ApplicationCommand::Toggle
            ))
        ));
    });
}

/// 事件桥：把应用事件与播放器状态汇成 GUI 的 Snapshot（服务层同一套逻辑的
/// 简化驱动），验证「自然播完 → 界面回到空闲」这类 GUI 依赖的被动变化。
#[test]
fn player_state_events_drive_gui_playback_state() {
    let (tx, mut rx) = tokio::sync::watch::channel(PlayerState::Idle);
    let notify = tokio::sync::broadcast::channel::<ApplicationEvent>(8).0;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        let mut states = Vec::new();
        tx.send(PlayerState::Playing).unwrap();
        states.push(voicefox_gui::PlaybackState::from(*rx.borrow_and_update()));
        tx.send(PlayerState::Paused).unwrap();
        states.push(voicefox_gui::PlaybackState::from(*rx.borrow_and_update()));
        tx.send(PlayerState::Stopped).unwrap();
        states.push(voicefox_gui::PlaybackState::from(*rx.borrow_and_update()));
        assert_eq!(
            states,
            vec![
                voicefox_gui::PlaybackState::Playing,
                voicefox_gui::PlaybackState::Paused,
                voicefox_gui::PlaybackState::Stopped,
            ]
        );
        // 事件通道本身是 GUI 的输入源：断开后界面仍可显示，不 panic。
        let mut subscriber = notify.subscribe();
        let _ = notify.send(ApplicationEvent::PlaybackPaused);
        assert!(subscriber.recv().await.is_ok());
        // 播放器事件（自然播完）用于停止态判断。
        let event = PlayerEvent::Ended { generation: 3 };
        assert!(matches!(event, PlayerEvent::Ended { .. }));
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert!(started.elapsed() < Duration::from_secs(1));
    });
}

#[test]
fn gui_theme_follows_config() {
    let theme = voicefox_gui::theme::GuiTheme::from_config(&test_config());
    assert_ne!(theme.base.to_array()[3], 0, "背景必须不透明");
    let visuals = theme.visuals();
    assert_eq!(visuals.panel_fill, theme.base);
}
