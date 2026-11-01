//! 平台媒体会话的共享数据层：快照构建、后端启动、媒体键命令执行。
//!
//! TUI（`voicefox-app`）与桌面 GUI（`voicefox-gui`）共用这一份：
//! - Linux 走 MPRIS（[`mpris`]），注册标准 `org.mpris.MediaPlayer2` 服务，
//!   Waybar、桌面媒体键、GNOME/KDE 媒体浮层都通过它控制播放；
//! - 快照与命令是平台无关的纯数据，后端只做"协议翻译"：
//!   往桌面方向推送 [`MediaSnapshot`]，往播放器方向回传 [`MediaCommand`]。

pub mod fmt;
pub mod mode;
#[cfg(target_os = "linux")]
pub mod mpris;
#[cfg(target_os = "macos")]
pub mod nowplaying;
#[cfg(target_os = "windows")]
pub mod smtc;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lx_core::model::song::SongInfo;
use lx_core::model::source::PlayerState;
use voicefox_application::ApplicationService;

pub use mode::PlayMode;

/// 桌面媒体控件（媒体键 / 系统浮层）请求播放器执行的动作。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum MediaCommand {
    Quit,
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Previous,
    /// 相对跳转，单位微秒（负值后退）。
    SeekBy(i64),
    /// 打开并播放一个 URI（桌面文件管理器 / 拖放「用 voicefox 打开」）。
    OpenUri(String),
    SetPosition(Duration),
    SetVolume(f64),
}

/// 一次媒体状态的完整快照；后端自行对比字段决定要更新哪些协议属性。
#[derive(Debug, Clone, PartialEq)]
pub struct MediaSnapshot {
    pub playback_status: &'static str,
    pub loop_status: &'static str,
    pub shuffle: bool,
    /// 曲目标识。MPRIS 用它当 D-Bus 对象路径，兼作变更检测的令牌。
    pub track_path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub source_url: String,
    pub duration_micros: i64,
    pub position_micros: i64,
    /// 当前进度所属的连续时间线（播放代次 + 跳转序号），变化即发 Seeked。
    pub position_epoch: u64,
    pub volume: f64,
    pub can_go_next: bool,
    pub can_go_previous: bool,
}

impl Default for MediaSnapshot {
    fn default() -> Self {
        Self {
            playback_status: "Stopped",
            loop_status: "None",
            shuffle: false,
            track_path: "/org/mpris/MediaPlayer2/TrackList/NoTrack".to_string(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art_url: String::new(),
            source_url: String::new(),
            duration_micros: 0,
            position_micros: 0,
            position_epoch: 0,
            volume: 0.8,
            can_go_next: false,
            can_go_previous: false,
        }
    }
}

impl MediaSnapshot {
    /// 按 UI 无关的语义字段组装快照。
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        state: PlayerState,
        song: Option<&SongInfo>,
        position: Duration,
        duration: Duration,
        volume: u32,
        play_mode: PlayMode,
        queue_len: usize,
        position_epoch: u64,
    ) -> Self {
        let playback_status = match state {
            PlayerState::Playing | PlayerState::Loading => "Playing",
            PlayerState::Paused => "Paused",
            PlayerState::Idle | PlayerState::Stopped => "Stopped",
        };
        let loop_status = match play_mode {
            PlayMode::SingleLoop => "Track",
            PlayMode::ListLoop => "Playlist",
            _ => "None",
        };
        let shuffle = play_mode == PlayMode::Random;
        let mut snapshot = Self {
            playback_status,
            loop_status,
            shuffle,
            position_micros: micros(position),
            position_epoch,
            duration_micros: micros(duration),
            volume: f64::from(volume.min(100)) / 100.0,
            // 队列只有一首时列表循环仍可“下一首”，因此只要在队列里就报告可用
            can_go_next: queue_len > 0,
            can_go_previous: queue_len > 0,
            ..Self::default()
        };

        if let Some(song) = song {
            snapshot.track_path = track_path(song);
            snapshot.title = song.name.clone();
            snapshot.artist = song.singer.clone();
            snapshot.album = song.album_name.clone();
            snapshot.art_url = song.cover_url.as_deref().map_or_else(String::new, art_url);
            snapshot.source_url = song.file_path.as_deref().map_or_else(String::new, file_url);
            if snapshot.duration_micros == 0 {
                snapshot.duration_micros = micros(song.duration);
            }
        }

        snapshot
    }

    /// 是否有可播放的曲目（MPRIS 的 can-play 属性）。
    #[allow(dead_code)]
    pub fn can_play(&self) -> bool {
        !self.title.is_empty()
    }

    /// 曲目是否可跳转进度（MPRIS 的 can-seek 属性）。
    #[allow(dead_code)]
    pub fn can_seek(&self) -> bool {
        self.duration_micros > 0
    }

    /// 歌曲本身是否发生了变化（不含进度 / 音量这类播放态）。
    pub fn metadata_changed(&self, other: &Self) -> bool {
        self.track_path != other.track_path
            || self.title != other.title
            || self.artist != other.artist
            || self.album != other.album
            || self.art_url != other.art_url
            || self.source_url != other.source_url
            || self.duration_micros != other.duration_micros
    }

    /// 离散播放态（播放/暂停、音量、模式、按钮可用性）是否一致。
    ///
    /// 不含进度：进度只随时间推进时无需推送，MPRIS 客户端自行外推。
    fn discrete_state_matches(&self, other: &Self) -> bool {
        self.playback_status == other.playback_status
            && self.loop_status == other.loop_status
            && self.shuffle == other.shuffle
            && self.volume == other.volume
            && self.can_go_next == other.can_go_next
            && self.can_go_previous == other.can_go_previous
    }
}

/// 向后端推送快照的入口；后端内部用通道转到自己的服务线程。
#[derive(Clone)]
pub struct MediaHandle {
    update_tx: tokio::sync::mpsc::UnboundedSender<MediaSnapshot>,
}

impl MediaHandle {
    pub fn update(&self, snapshot: MediaSnapshot) {
        let _ = self.update_tx.send(snapshot);
    }
}

/// 新建一个 handle + 配套的更新接收端，供后端启动函数装配。
pub fn channel() -> (
    MediaHandle,
    tokio::sync::mpsc::UnboundedReceiver<MediaSnapshot>,
) {
    let (update_tx, update_rx) = tokio::sync::mpsc::unbounded_channel();
    (MediaHandle { update_tx }, update_rx)
}

pub fn micros(duration: Duration) -> i64 {
    duration.as_micros().min(i64::MAX as u128) as i64
}

fn track_path(song: &SongInfo) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    song.source.hash(&mut hasher);
    song.id.hash(&mut hasher);
    format!("/org/mpris/MediaPlayer2/track/{}", hasher.finish())
}

fn art_url(value: &str) -> String {
    if value.starts_with('/') {
        file_url(std::path::Path::new(value))
    } else {
        value.to_string()
    }
}

fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

/// 同步启动（Windows SMTC / macOS Now Playing 的后端不经过 tokio）。
///
/// Linux 的 MPRIS 建 D-Bus 连接是异步的，请用 [`start_media_controls`]。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn start_media_controls_sync(
    enabled: bool,
) -> (
    Option<MediaHandle>,
    Option<tokio::sync::mpsc::UnboundedReceiver<MediaCommand>>,
) {
    if !enabled {
        return (None, None);
    }
    let result = {
        #[cfg(target_os = "windows")]
        {
            smtc::start()
        }
        #[cfg(target_os = "macos")]
        {
            nowplaying::start()
        }
    };
    match result {
        Ok((handle, receiver)) => (Some(handle), Some(receiver)),
        Err(error) => {
            tracing::warn!("media controls unavailable: {error:#}");
            (None, None)
        }
    }
}

/// 启动平台媒体控件后端；平台不支持、配置关闭或启动失败时返回 `(None, None)`。
///
/// 失败只记日志，不阻断播放。必须 await：MPRIS 建 D-Bus 连接是异步的，
/// 调用方不能在自己的 runtime 里 `block_on`。
pub async fn start_media_controls(
    enabled: bool,
) -> (
    Option<MediaHandle>,
    Option<tokio::sync::mpsc::UnboundedReceiver<MediaCommand>>,
) {
    if !enabled {
        return (None, None);
    }
    let result = {
        #[cfg(target_os = "linux")]
        {
            mpris::start().await
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(anyhow::anyhow!("当前平台暂无媒体控件后端"))
        }
    };
    match result {
        Ok((handle, receiver)) => (Some(handle), Some(receiver)),
        Err(error) => {
            tracing::warn!("media controls unavailable: {error:#}");
            (None, None)
        }
    }
}

/// Windows/macOS 共用：把 souvlaki 的控件事件翻译成播放器命令。
///
/// Linux MPRIS 走 zbus，事件在 [``mpris``] 里直接翻译。
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn translate_event(event: souvlaki::MediaControlEvent) -> Option<MediaCommand> {
    use souvlaki::{MediaControlEvent, SeekDirection};

    match event {
        MediaControlEvent::Play => Some(MediaCommand::Play),
        MediaControlEvent::Pause => Some(MediaCommand::Pause),
        MediaControlEvent::Toggle => Some(MediaCommand::Toggle),
        MediaControlEvent::Stop => Some(MediaCommand::Stop),
        MediaControlEvent::Next => Some(MediaCommand::Next),
        MediaControlEvent::Previous => Some(MediaCommand::Previous),
        MediaControlEvent::Seek(SeekDirection::Forward) => Some(MediaCommand::SeekBy(5_000_000)),
        MediaControlEvent::Seek(SeekDirection::Backward) => Some(MediaCommand::SeekBy(-5_000_000)),
        MediaControlEvent::SeekBy(direction, amount) => {
            let offset = i64::try_from(amount.as_micros()).unwrap_or(i64::MAX);
            Some(MediaCommand::SeekBy(match direction {
                SeekDirection::Forward => offset,
                SeekDirection::Backward => -offset,
            }))
        }
        MediaControlEvent::SetPosition(position) => Some(MediaCommand::SetPosition(position.0)),
        // 音量键由系统混音器处理；打开 URI 暂未接入 souvlaki 后端。
        MediaControlEvent::SetVolume(_)
        | MediaControlEvent::OpenUri(_)
        | MediaControlEvent::Raise
        | MediaControlEvent::Quit => None,
    }
}

/// 媒体键命令映射结果。
#[allow(clippy::large_enum_variant)]
pub enum MediaAction {
    Dispatch(voicefox_application::ApplicationCommand),
    /// 播放一组新曲目（OpenUri：本地文件 / 受支持的音源链接）。
    PlaySongs(Vec<SongInfo>),
    /// 音量不属于 ApplicationCommand：前端直接作用在播放器和配置上。
    SetVolume(u32),
    Quit,
}

impl MediaCommand {
    /// 结合当前播放状态与队列长度，把媒体键翻译成业务命令。
    ///
    /// `Pause` 在已暂停时、`Play` 在已在播放时返回 `None`（幂等，避免
    /// 桌面控件重复投递同一状态时打断播放）。
    pub fn into_action(
        &self,
        playback_state: PlayerState,
        position: Duration,
        queue_len: usize,
    ) -> Option<MediaAction> {
        use voicefox_application::ApplicationCommand;

        let step = |forward: bool| -> Option<MediaAction> {
            (queue_len > 0).then_some(MediaAction::Dispatch(if forward {
                ApplicationCommand::Next
            } else {
                ApplicationCommand::Previous
            }))
        };
        Some(match self {
            MediaCommand::Quit => MediaAction::Quit,
            MediaCommand::Play => MediaAction::Dispatch(match playback_state {
                PlayerState::Paused => ApplicationCommand::Resume,
                PlayerState::Idle | PlayerState::Stopped => ApplicationCommand::PlayCurrent,
                _ => return None,
            }),
            MediaCommand::Pause => {
                if playback_state == PlayerState::Paused {
                    return None;
                }
                MediaAction::Dispatch(ApplicationCommand::Pause)
            }
            MediaCommand::Toggle => MediaAction::Dispatch(ApplicationCommand::Toggle),
            MediaCommand::Stop => MediaAction::Dispatch(ApplicationCommand::Stop),
            MediaCommand::Next => return step(true),
            MediaCommand::Previous => return step(false),
            MediaCommand::SeekBy(offset) => {
                let current = micros(position);
                let target = if *offset >= 0 {
                    current.saturating_add(*offset)
                } else {
                    current.saturating_sub(offset.unsigned_abs() as i64)
                };
                MediaAction::Dispatch(ApplicationCommand::Seek(Duration::from_micros(
                    target.max(0) as u64,
                )))
            }
            MediaCommand::SetPosition(position) => {
                MediaAction::Dispatch(ApplicationCommand::Seek(*position))
            }
            MediaCommand::OpenUri(uri) => MediaAction::PlaySongs(vec![song_from_uri(uri)?]),
            MediaCommand::SetVolume(volume) => {
                MediaAction::SetVolume((volume.clamp(0.0, 1.0) * 100.0).round() as u32)
            }
        })
    }
}

/// 把 MPRIS `OpenUri` 的 URI 解析成本地曲目。
///
/// 只接受本地文件（`file://` 或绝对路径）：网络链接的解析需要音源层，
/// 不属于媒体控件的职责。文件名不猜歌手，交给后续的标签读取。
pub fn song_from_uri(uri: &str) -> Option<SongInfo> {
    let trimmed = uri.trim();
    let path = trimmed.strip_prefix("file://").map_or_else(
        || std::path::PathBuf::from(trimmed),
        std::path::PathBuf::from,
    );
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let mut song = SongInfo::new(
        path.to_string_lossy().into_owned(),
        lx_core::model::source::SourceId::Local,
        name,
        String::new(),
    );
    song.file_path = Some(path.clone());
    song.file_ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase());
    Some(song)
}

/// 菜单里的循环顺序：列表循环 → 单曲循环 → 随机 → 顺序 → 播完停止。
pub fn cycle_play_mode(current: PlayMode) -> PlayMode {
    current.next_mode()
}

/// 共享的媒体状态源：把 ApplicationService 的当前状态同步成 MPRIS 快照，
/// 并把桌面媒体键回传成业务命令。
///
/// GUI 侧另外回报播放进度（ApplicationService 不跟踪进度），通过
/// [`MediaSession::set_position`] 注入；跳转通过 [`MediaSession::mark_seek`]。
pub struct MediaSession {
    application: ApplicationService,
    handle: Option<MediaHandle>,
    commands: Option<tokio::sync::mpsc::UnboundedReceiver<MediaCommand>>,
    mode: PlayMode,
    last: Option<MediaSnapshot>,
    last_sent: Option<MediaSnapshot>,
    position: Duration,
    generation: u64,
    seek_epoch: AtomicU64,
    /// 下一次 tick 必须推送（跳转 / 模式变化等离散事件）。
    force: bool,
}

impl MediaSession {
    pub fn new(
        application: ApplicationService,
        mode: PlayMode,
        handle: Option<MediaHandle>,
        commands: Option<tokio::sync::mpsc::UnboundedReceiver<MediaCommand>>,
    ) -> Self {
        Self {
            application,
            handle,
            commands,
            mode,
            last: None,
            last_sent: None,
            position: Duration::ZERO,
            generation: 0,
            seek_epoch: AtomicU64::new(0),
            force: true,
        }
    }

    /// 媒体控件后端起好后挂上来（MPRIS 需要异步启动，因此不在构造时做）。
    pub fn attach(
        &mut self,
        handle: Option<MediaHandle>,
        commands: Option<tokio::sync::mpsc::UnboundedReceiver<MediaCommand>>,
    ) {
        self.handle = handle;
        self.commands = commands;
        self.force = true;
    }

    /// 回报当前播放进度（由拥有进度源的界面层或播放器 watcher 提供）。
    pub fn set_position(&mut self, position: Duration) {
        self.position = position;
    }

    /// 用户跳转后调用：推进进度纪元，让 MPRIS 发出 Seeked。
    pub fn mark_seek(&mut self, position: Duration) {
        self.position = position;
        self.seek_epoch.fetch_add(1, Ordering::Relaxed);
        self.force = true;
    }

    pub fn play_mode(&self) -> PlayMode {
        self.mode
    }

    pub fn set_play_mode(&mut self, mode: PlayMode) {
        self.mode = mode;
        self.force = true;
    }

    /// 取走一条桌面媒体控件（媒体键）命令。
    pub fn next_command(&mut self) -> Option<MediaCommand> {
        self.commands.as_mut()?.try_recv().ok()
    }

    pub fn has_media_controls(&self) -> bool {
        self.handle.is_some()
    }

    /// 用 ApplicationService 的当前状态刷新快照；有变化时推送给桌面控件。
    ///
    /// 返回本次是否真的推送了（GUI 以固定频率调用即可）。
    pub fn tick(&mut self) -> bool {
        let Some(handle) = self.handle.as_ref() else {
            return false;
        };
        let state = self.application.state();
        let playback = &state.playback;
        if playback.generation != self.generation {
            self.generation = playback.generation;
            self.position = Duration::ZERO;
            self.force = true;
        }
        // 进度不能超过时长；但 ApplicationState 的时长可能还没同步（服务层
        // 不跟踪播放器的 duration watcher），为 0 时不能把进度削成 0。
        if playback.duration > Duration::ZERO {
            self.position = self.position.min(playback.duration);
        }
        let snapshot = MediaSnapshot::build(
            playback.state,
            playback.song.as_ref(),
            self.position,
            playback.duration,
            playback.volume,
            self.mode,
            state.queue.songs.len(),
            self.generation
                .wrapping_mul(1_000_003)
                .wrapping_add(self.seek_epoch.load(Ordering::Relaxed)),
        );
        let changed = self
            .last_sent
            .as_ref()
            .is_none_or(|previous| !previous.discrete_state_matches(&snapshot));
        self.last = Some(snapshot.clone());
        if !(self.force || changed || self.last_sent.is_none()) {
            return false;
        }
        self.force = false;
        handle.update(snapshot.clone());
        self.last_sent = Some(snapshot);
        true
    }

    /// 当前快照（测试用）。
    pub fn snapshot(&self) -> Option<&MediaSnapshot> {
        self.last.as_ref()
    }
}

/// 把一条媒体键命令执行到 ApplicationService 上，并刷新媒体快照。
///
/// 返回 `true` 表示用户从桌面控件请求退出播放器，调用方应结束进程。
pub async fn execute_media_action(
    application: &ApplicationService,
    media: &mut MediaSession,
    command: &MediaCommand,
    on_volume: impl FnOnce(u32),
) -> bool {
    let state = application.state();
    let action = command.into_action(
        state.playback.state,
        media_reported_position(media, &state.playback.position),
        state.queue.songs.len(),
    );
    match action {
        Some(MediaAction::Dispatch(command)) => {
            if let Err(error) = application.dispatch(command).await {
                tracing::warn!("media key command failed: {error}");
            }
            media.tick();
            false
        }
        Some(MediaAction::SetVolume(volume)) => {
            on_volume(volume);
            media.tick();
            false
        }
        Some(MediaAction::PlaySongs(songs)) => {
            if let Err(error) = application
                .dispatch(voicefox_application::ApplicationCommand::Play { songs, index: 0 })
                .await
            {
                tracing::warn!("OpenUri 播放失败: {error}");
            }
            media.tick();
            false
        }
        Some(MediaAction::Quit) => true,
        None => false,
    }
}

fn media_reported_position(media: &MediaSession, fallback: &Duration) -> Duration {
    media.snapshot().map_or(*fallback, |snapshot| {
        Duration::from_micros(snapshot.position_micros.max(0) as u64)
    })
}

/// 后台任务：把 `player.position_watcher()` 的进度回报给 [`MediaSession`]。
///
/// GUI 等不做查询接口的前端用它把真实播放进度送进 MPRIS。
pub fn spawn_position_reporter(
    media: std::sync::Arc<std::sync::Mutex<MediaSession>>,
    player: std::sync::Arc<dyn lx_core::traits::player::PlayerBackend>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut watcher = player.position_watcher();
        loop {
            if watcher.changed().await.is_err() {
                break;
            }
            let position = *watcher.borrow_and_update();
            let Ok(mut guard) = media.lock() else {
                break;
            };
            guard.set_position(position);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{MediaAction, MediaCommand, MediaSnapshot, PlayMode, art_url, track_path};
    use lx_core::model::song::SongInfo;
    use lx_core::model::source::{PlayerState, SourceId};
    use std::time::Duration;
    use voicefox_application::ApplicationCommand;

    #[test]
    fn snapshot_exposes_track_metadata() {
        let mut song = SongInfo::new(
            "42".to_string(),
            SourceId::Bili,
            "Song".to_string(),
            "Artist".to_string(),
        );
        song.album_name = "Album".to_string();
        song.duration = Duration::from_secs(90);
        song.cover_url = Some("https://example.com/cover.jpg".to_string());
        let snapshot = MediaSnapshot::build(
            PlayerState::Playing,
            Some(&song),
            Duration::from_secs(3),
            Duration::ZERO,
            75,
            PlayMode::SingleLoop,
            3,
            0,
        );

        assert_eq!(snapshot.playback_status, "Playing");
        assert_eq!(snapshot.loop_status, "Track");
        assert_eq!(snapshot.duration_micros, 90_000_000);
        assert_eq!(snapshot.position_micros, 3_000_000);
        assert_eq!(snapshot.track_path, track_path(&song));
        assert!(snapshot.can_go_next);
    }

    #[test]
    fn local_cover_path_is_exposed_as_file_uri() {
        assert_eq!(art_url("/tmp/cover.jpg"), "file:///tmp/cover.jpg");
    }

    #[test]
    fn media_keys_map_to_application_commands() {
        let toggle = MediaCommand::Toggle.into_action(PlayerState::Playing, Duration::ZERO, 1);
        assert!(matches!(
            toggle,
            Some(MediaAction::Dispatch(ApplicationCommand::Toggle))
        ));
        let play = MediaCommand::Play.into_action(PlayerState::Paused, Duration::ZERO, 1);
        assert!(matches!(
            play,
            Some(MediaAction::Dispatch(ApplicationCommand::Resume))
        ));
        // 已在播放时再收到 Play 不打断播放。
        assert!(
            MediaCommand::Play
                .into_action(PlayerState::Playing, Duration::ZERO, 1)
                .is_none()
        );
        // 队列为空时切歌无意义。
        assert!(
            MediaCommand::Next
                .into_action(PlayerState::Playing, Duration::ZERO, 0)
                .is_none()
        );
    }

    #[test]
    fn seek_by_moves_position_within_bounds() {
        let action = MediaCommand::SeekBy(-5_000_000).into_action(
            PlayerState::Playing,
            Duration::from_secs(2),
            3,
        );
        assert!(matches!(
            action,
            Some(MediaAction::Dispatch(ApplicationCommand::Seek(d))) if d == Duration::ZERO
        ));
    }

    #[test]
    fn open_uri_accepts_local_files_only() {
        let dir = std::env::temp_dir();
        let file = dir.join("voicefox-open-uri-probe.mp3");
        std::fs::write(&file, b"not really audio").unwrap();
        let uri = format!("file://{}", file.display());
        let song = super::song_from_uri(&uri).expect("本地文件应能打开");
        assert_eq!(song.source, lx_core::model::source::SourceId::Local);
        assert_eq!(song.file_path.as_deref(), Some(file.as_path()));
        assert_eq!(song.file_ext.as_deref(), Some("mp3"));
        assert!(super::song_from_uri("https://example.com/a.mp3").is_none());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn play_mode_cycles_through_every_configurable_mode() {
        let mut mode = PlayMode::ListLoop;
        let mut values = Vec::new();
        for _ in 0..5 {
            values.push(mode.as_config());
            mode = super::cycle_play_mode(mode);
        }
        assert_eq!(
            values,
            vec!["list-loop", "single-loop", "random", "list", "none"]
        );
    }
}
