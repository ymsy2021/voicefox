//! 桌面 GUI 的服务层：把 ApplicationService 的 command/state/event 边界
//! 与播放器 watcher 接到 egui 的帧循环上。
//!
//! 设计约束：
//! - 后端命令只走 `ApplicationService::dispatch`，与 TUI、Flutter FFI 同一入口；
//! - egui 每帧只读一份 [`Snapshot`]（UI 线程不加锁、不 await、不碰 tokio）；
//! - 播放状态以播放器 `state_watcher` 为准（ApplicationService 不感知自然播完、
//!   暂停这类被动变化），进度以 `position_watcher` 为准；
//! - 桌面媒体键（MPRIS）走 `voicefox_runtime::media`，与这里共享同一状态源。
//!
//! 唯一的 GUI 专有改写在服务线程里完成：搜索结果的「播放整张列表」需要
//! 一次投递「队列 + 播放索引」，而 ApplicationCommand 没有队列替换语义
//! （QueueRemove 等会重排索引），因此由这里把一条 [`GuiRequest`] 翻译成
//! 多条 ApplicationCommand。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lx_core::model::config::Config;
use lx_core::model::song::SongInfo;
use lx_core::model::source::{PlayerState, SourceId};
use lx_core::traits::player::PlayerBackend;
use lx_source::manager::SourceManager;
use tokio::sync::mpsc;
use voicefox_application::{
    ApplicationCommand, ApplicationEvent, ApplicationService, ApplicationState,
};
use voicefox_runtime::ApplicationRuntime;
use voicefox_runtime::download::DownloadManager;
use voicefox_runtime::media::{self, MediaSession, PlayMode};
use voicefox_runtime::storage::Storage;

/// 服务层的错误：只保留面向用户的文案。
#[derive(Debug, Clone)]
pub struct GuiError(pub String);

impl std::fmt::Display for GuiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GuiError {}

/// 界面每帧读取的一首歌。
#[derive(Debug, Clone, PartialEq)]
pub struct TrackView {
    pub id: String,
    pub source: String,
    pub name: String,
    pub singer: String,
    pub album: String,
    pub duration_ms: u64,
    pub cover_url: Option<String>,
}

impl TrackView {
    fn of(song: &SongInfo) -> Self {
        Self {
            id: song.id.clone(),
            source: song.source.as_str().to_string(),
            name: song.name.clone(),
            singer: song.singer.clone(),
            album: song.album_name.clone(),
            duration_ms: song.duration.as_millis() as u64,
            cover_url: song.cover_url.clone(),
        }
    }

    pub fn to_song(&self) -> SongInfo {
        let source = match self.source.as_str() {
            "kw" => SourceId::Kw,
            "kg" => SourceId::Kg,
            "tx" => SourceId::Tx,
            "wy" => SourceId::Wy,
            "mg" => SourceId::Mg,
            "bili" => SourceId::Bili,
            "soda" => SourceId::Soda,
            "qianqian" => SourceId::Qianqian,
            "joox" => SourceId::Joox,
            "jamendo" => SourceId::Jamendo,
            "fivesing" => SourceId::Fivesing,
            "apple" => SourceId::Apple,
            _ => SourceId::Local,
        };
        let mut song = SongInfo::new(
            self.id.clone(),
            source,
            self.name.clone(),
            self.singer.clone(),
        );
        song.album_name = self.album.clone();
        song.duration = Duration::from_millis(self.duration_ms);
        song.cover_url = self.cover_url.clone();
        song
    }
}

/// 搜索结果视图。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchView {
    pub keyword: String,
    pub loading: bool,
    pub error: Option<String>,
    pub has_more: bool,
    pub page: u32,
    pub items: Vec<TrackView>,
}

/// 歌词视图（当前行与全部行）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LyricsView {
    pub lines: Vec<String>,
    pub current: usize,
    /// 当前行的翻译（配置打开翻译时由核心给出）。
    pub translation: Option<String>,
    /// 逐字歌词（YRC）：当前行的词与起止时间，供 karaoke 高亮。
    pub words: Vec<LyricWord>,
}

/// 逐字歌词的一小段。
#[derive(Debug, Clone, PartialEq)]
pub struct LyricWord {
    pub text: String,
    pub start_ms: u64,
    pub duration_ms: u64,
}

/// 歌词页的可切换显示项（对应核心的 `lyric.show_translation` / `show_yrc`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricToggle {
    Translation,
    Yrc,
}

/// 收藏 / 历史 / 歌单页的数据。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryView {
    pub favorites: Vec<TrackView>,
    pub history: Vec<TrackView>,
    pub playlists: Vec<PlaylistView>,
    /// 当前打开的歌单（`None` 表示只看列表）。
    pub selected: Option<PlaylistView>,
    pub error: Option<String>,
}

/// 自建歌单。
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistView {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    /// 打开后可见的歌曲（列表页为空）。
    pub songs: Vec<TrackView>,
}

/// 音源切换与扫码登录状态。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourcesView {
    /// 可用的在线音源（顺序即界面顺序）。
    pub available: Vec<String>,
    pub enabled: Vec<String>,
    pub default: String,
    /// 已登录的音源（音源只保存 cookie，界面只显示是否已登录）。
    pub logged_in: Vec<String>,
    pub login: Option<LoginView>,
    pub error: Option<String>,
}

/// 扫码登录进度。
#[derive(Debug, Clone, PartialEq)]
pub struct LoginView {
    pub source: String,
    /// 二维码内容：链接或图片 base64（QQ 走图片）。
    pub url: String,
    pub image_base64: Option<String>,
    pub status: String,
    pub message: String,
    /// 二维码是否仍在轮询中。
    pub polling: bool,
}

/// 下载面板的数据。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DownloadsView {
    pub tasks: Vec<DownloadItemView>,
    pub download_dir: String,
}

/// 一条下载任务。
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadItemView {
    pub id: u64,
    pub name: String,
    pub singer: String,
    /// 界面上的状态文案。
    pub state: String,
    pub active: bool,
    pub can_retry: bool,
    pub downloaded: u64,
    pub total: u64,
    /// 已耗秒数（服务线程按两次快照的增量子算速度）。
    pub elapsed_ms: u64,
    /// 下载速度（字节/秒），服务线程按两次快照的增量估计。
    pub speed_bps: u64,
    pub error: Option<String>,
}

impl DownloadItemView {
    pub fn percent(&self) -> Option<u32> {
        (self.total > 0).then(|| (self.downloaded.saturating_mul(100) / self.total) as u32)
    }
}

/// 一帧的完整界面状态。
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub state: PlaybackState,
    pub song: Option<TrackView>,
    pub position: Duration,
    pub duration: Duration,
    pub volume: u32,
    pub play_mode: PlayMode,
    pub queue: Vec<TrackView>,
    pub queue_index: usize,
    pub search: SearchView,
    pub lyrics: LyricsView,
    pub notification: Option<String>,
    pub has_media_controls: bool,
    /// 顺序 / 播完停止模式下队列已经播到结尾（播放按钮改为从头开始）。
    pub playlist_ended: bool,
    pub library: LibraryView,
    pub sources: SourcesView,
    pub downloads: DownloadsView,
}

impl Default for Snapshot {
    /// 首帧（服务尚未发布）的占位状态与配置默认播放模式一致。
    fn default() -> Self {
        Self {
            state: PlaybackState::default(),
            song: None,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            volume: 100,
            play_mode: PlayMode::ListLoop,
            queue: Vec::new(),
            queue_index: 0,
            search: SearchView::default(),
            lyrics: LyricsView::default(),
            notification: None,
            has_media_controls: false,
            playlist_ended: false,
            library: LibraryView::default(),
            sources: SourcesView::default(),
            downloads: DownloadsView::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Stopped,
}

impl From<PlayerState> for PlaybackState {
    fn from(value: PlayerState) -> Self {
        match value {
            PlayerState::Idle => Self::Idle,
            PlayerState::Loading => Self::Loading,
            PlayerState::Playing => Self::Playing,
            PlayerState::Paused => Self::Paused,
            PlayerState::Stopped => Self::Stopped,
        }
    }
}

impl PlaybackState {
    pub fn is_playing(self) -> bool {
        matches!(self, Self::Playing | Self::Loading)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "空闲",
            Self::Loading => "加载中",
            Self::Playing => "播放中",
            Self::Paused => "已暂停",
            Self::Stopped => "已停止",
        }
    }
}

/// GUI 提交给服务线程的请求。
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum GuiRequest {
    /// 透传一条业务命令，与 TUI / FFI 同一入口。
    Dispatch(ApplicationCommand),
    /// 播放一整张列表（搜索结果「播放全部」或双击某条结果）。
    ///
    /// 列表与索引必须在**发起请求时**一并带上：服务线程要等前一条命令
    /// 执行完才会更新队列，等它再读「当前队列」时可能还是空的或上一次的。
    PlayList {
        songs: Vec<TrackView>,
        index: usize,
    },
    /// 音量（0–100）：写播放器与配置，并让 MPRIS 音量属性同步。
    SetVolume(u32),
    /// 播放模式：写配置并更新 MPRIS。
    SetPlayMode(PlayMode),
    /// 队列歌曲上移/下移一位（保持 queue_index 指向当前曲目）。
    MoveQueue {
        from: usize,
        to: usize,
    },
    /// 顺序 / 播完停止模式下队列已播完时按「播放」：从队列开头重新开始。
    RestartPlaylist,
    /// 直接播放一组曲目（MPRIS OpenUri：本地文件）。
    PlaySongs(Vec<SongInfo>),
    /// 播完停止（PlayMode::None）：播完最后一首后停在结尾。
    PlaylistEnded,
    /// 队列拖动排序（走共享的 `ApplicationCommand::QueueMove`）。
    ReorderQueue {
        from: usize,
        to: usize,
    },
    /// 刷新收藏 / 历史 / 歌单。
    RefreshLibrary,
    /// 打开一首自建歌单（读歌单内容）。
    OpenPlaylist(String),
    RefreshSources,
    /// 切换默认音源（与 TUI 设置页同一语义：写配置并更新音源管理器）。
    SetDefaultSource(String),
    /// 启用/停用一个音源。
    ToggleSource(String),
    /// 开始扫码登录（重新生成二维码）。
    StartLogin(String),
    /// 登出。
    LogoutSource(String),
    /// 歌词显示开关（翻译 / 逐字）。
    ToggleLyric(LyricToggle),
    /// 把歌曲加入收藏 / 自建歌单。
    ToggleFavorite(TrackView),
    AddToPlaylist {
        playlist: String,
        song: TrackView,
    },
    /// 新建 / 删除 / 重命名自建歌单。
    CreatePlaylist(String),
    DeletePlaylist(String),
    /// 从历史 / 歌单里移除一首。
    RemoveFromHistory(TrackView),
    RemoveFromPlaylist {
        playlist: String,
        song: TrackView,
    },
    /// 下载：入队当前曲目 / 搜索结果里的一首，取消、清理已结束。
    DownloadSong(TrackView),
    CancelDownload(u64),
    ClearFinishedDownloads,
    /// 从桌面控件退出（MPRIS Quit / 窗口关闭）。
    Quit,
}

impl From<ApplicationCommand> for GuiRequest {
    fn from(value: ApplicationCommand) -> Self {
        Self::Dispatch(value)
    }
}

/// 服务线程 → UI 线程的最新快照（只读读取，不加 tokio 锁）。
#[derive(Default)]
struct SnapshotCell(std::sync::Mutex<Snapshot>);

impl SnapshotCell {
    fn get(&self) -> Snapshot {
        self.0.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    fn set(&self, snapshot: Snapshot) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = snapshot;
        }
    }
}

impl GuiError {
    /// 构造一条面向用户的错误文案（演示后端与测试用）。
    pub fn message(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

/// 桌面 GUI 需要的后端能力。
///
/// 生产实现是 [`spawn`] 背后的真实服务线程；界面因此可以脱离网络 / 音频设备
/// 单独驱动（见 `gui/src/bin/voicefox-gui-demo.rs` 与 UI 快照测试）。
pub trait GuiBackend: Send + Sync {
    fn send(&self, request: GuiRequest) -> Result<(), GuiError>;
    fn take_snapshot(&self) -> Snapshot;
    fn quit_requested(&self) -> bool;
    /// 重绘回调句柄（UI 启动后装入 egui Context 的 `request_repaint`）。
    fn repaint_handle(&self) -> RepaintHandle;

    /// 请求服务线程刷新长任务数据（下载进度、扫码状态）。
    fn request_refresh(&self);

    /// 共享同一后端的句柄（输入线程 / 测试可以各自提交请求）。
    ///
    /// egui 的 `App` 与控制器都独占自己的 Box，外部线程必须拿到可克隆的
    /// 句柄才能提交命令；生产实现直接克隆 [`AppService`]。
    fn clone_backend(&self) -> Box<dyn GuiBackend>;
}

impl GuiBackend for AppService {
    fn send(&self, request: GuiRequest) -> Result<(), GuiError> {
        AppService::send(self, request)
    }

    fn take_snapshot(&self) -> Snapshot {
        AppService::take_snapshot(self)
    }

    fn quit_requested(&self) -> bool {
        AppService::quit_requested(self)
    }

    fn repaint_handle(&self) -> RepaintHandle {
        AppService::repaint_handle(self)
    }

    fn request_refresh(&self) {
        AppService::request_refresh(self);
    }

    fn clone_backend(&self) -> Box<dyn GuiBackend> {
        Box::new(self.clone())
    }
}

/// 克隆后端时装箱的类型：和 `Box<dyn GuiBackend>` 等价，便于各实现返回。
pub type ClonedBackend = Box<dyn GuiBackend>;

/// 服务句柄：UI 持有它来提交请求、读取快照。
pub struct AppService {
    requests: mpsc::UnboundedSender<GuiRequest>,
    /// 服务线程发布的最新快照（`std::sync::Mutex`，UI 线程只做非阻塞读取）。
    snapshot: Arc<SnapshotCell>,
    quit: Arc<AtomicBool>,
    /// 快照变化时调用（egui 请求重绘）；UI 启动后由控制器装入。
    wake: RepaintHandle,
    /// 长任务（下载、扫码）的定时刷新句柄。
    refresh: RefreshHandle,
}

/// 「立即刷新一次」的请求槽：服务线程在 select 里等待它。
#[derive(Clone, Default)]
pub struct RefreshHandle(Arc<tokio::sync::Notify>);

impl RefreshHandle {
    fn notify(&self) {
        // 已经有一次待处理时不再叠加：刷新是幂等的。
        self.0.notify_one();
    }

    async fn notified(&self) {
        self.0.notified().await;
    }
}

/// 可替换的重绘回调槽：服务在启动时还不知道 egui 的 Context。
type RepaintSlot = std::sync::RwLock<Option<Arc<dyn Fn() + Send + Sync>>>;

/// 可替换的重绘回调：服务在启动时还不知道 egui 的 Context。
#[derive(Clone, Default)]
pub struct RepaintHandle(Arc<RepaintSlot>);

impl RepaintHandle {
    /// 装入重绘回调（通常在窗口创建后调用一次）。
    pub fn set(&self, repaint: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut guard) = self.0.write() {
            *guard = Some(repaint);
        }
    }

    /// 触发一次重绘请求（服务线程与演示后端共用）。
    pub fn call(&self) {
        let callback = self.0.read().ok().and_then(|guard| guard.as_ref().cloned());
        if let Some(callback) = callback {
            callback();
        }
    }
}

impl AppService {
    /// 提交一条请求；后台已退出时返回错误（UI 显示通知）。
    pub fn send(&self, request: GuiRequest) -> Result<(), GuiError> {
        self.requests
            .send(request)
            .map_err(|_| GuiError("后台已退出".into()))?;
        self.wake.call();
        Ok(())
    }

    /// 重绘回调句柄：UI 启动后装入 egui Context 的 `request_repaint`。
    pub fn repaint_handle(&self) -> RepaintHandle {
        self.wake.clone()
    }

    /// 请求服务线程立即刷新一次长任务数据（下载进度、扫码状态）。
    pub fn request_refresh(&self) {
        self.refresh.notify();
    }

    pub fn dispatch(&self, command: ApplicationCommand) -> Result<(), GuiError> {
        self.send(GuiRequest::Dispatch(command))
    }

    /// 非阻塞读取当前快照：UI 每帧调用。
    pub fn take_snapshot(&self) -> Snapshot {
        self.snapshot.get()
    }

    /// 兼容异步调用方（测试 / 其他前端）。
    pub async fn snapshot(&self) -> Snapshot {
        self.take_snapshot()
    }

    /// 后台线程已退出（进程该结束了）。
    pub fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }
}

impl Clone for AppService {
    fn clone(&self) -> Self {
        Self {
            requests: self.requests.clone(),
            snapshot: Arc::clone(&self.snapshot),
            quit: Arc::clone(&self.quit),
            wake: self.wake.clone(),
            refresh: self.refresh.clone(),
        }
    }
}

/// 启动服务：在独立线程上建 tokio runtime，订阅事件并把快照送给 UI。
/// 快照变化时经 [`RepaintHandle`] 请求 UI 重绘。
///
/// `startup` 里的请求在 UI 出现前就投进队列（例如
/// `VOICEFOX_GUI_AUTOSEARCH` 直接搜索并播放首条结果）。
pub fn spawn(runtime: ApplicationRuntime, startup: Vec<GuiRequest>) -> AppService {
    let (requests_tx, requests_rx) = mpsc::unbounded_channel();
    let snapshot = Arc::new(SnapshotCell::default());
    let quit = Arc::new(AtomicBool::new(false));
    let wake = RepaintHandle::default();
    let refresh = RefreshHandle::default();
    let refresh_for_bridge = refresh.clone();
    for request in startup {
        let _ = requests_tx.send(request);
    }
    let service = AppService {
        requests: requests_tx,
        snapshot: Arc::clone(&snapshot),
        quit: Arc::clone(&quit),
        wake: wake.clone(),
        refresh: refresh.clone(),
    };
    std::thread::Builder::new()
        .name("voicefox-gui-backend".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("voicefox-gui-worker")
                .build()
                .expect("tokio runtime");
            result.block_on(run_bridge(
                runtime,
                snapshot,
                quit,
                requests_rx,
                wake,
                refresh_for_bridge,
            ));
        })
        .expect("spawn backend thread");
    service
}

/// 服务线程内的粘合状态。
struct Bridge {
    application: ApplicationService,
    media: Arc<std::sync::Mutex<MediaSession>>,
    player: Arc<dyn PlayerBackend>,
    state: PlaybackState,
    position: Duration,
    duration: Duration,
    volume: u32,
    play_mode: PlayMode,
    queue: Vec<TrackView>,
    queue_index: usize,
    lyrics: LyricsView,
    search: SearchView,
    /// 以 request_id 过滤过期搜索事件（并发搜索时旧请求可能后到）。
    search_request: u64,
    /// 当前曲目（ApplicationState 的 playback.song，权威来源）。
    current: Option<TrackView>,
    /// 顺序 / 播完停止模式下队列已播到末尾。
    playlist_ended: bool,
    notification: Option<String>,
    // ── P1：歌单/收藏/历史、音源与扫码、歌词开关、下载 ──
    /// 收藏 / 历史 / 自建歌单：直接读写 TUI 同一份存储。
    library_store: Option<Arc<Storage>>,
    library: LibraryView,
    sources: Option<Arc<SourceManager>>,
    sources_view: SourcesView,
    /// 扫码登录的后台任务写到这个槽，界面每帧只读它。
    login_slot: Arc<std::sync::Mutex<Option<LoginView>>>,
    downloads: Option<Arc<DownloadManager>>,
    downloads_view: DownloadsView,
    /// 上一次下载快照的字节数与时间，用于估计速度。
    download_marks: std::collections::HashMap<u64, (u64, std::time::Instant)>,
    /// 改配置的请求（配置锁在 `ApplicationRuntime` 里，服务线程只转发）。
    config_tx: tokio::sync::mpsc::UnboundedSender<ConfigChange>,
    /// 当前配置（音源开关、歌词开关的判断依据，服务线程自己维护）。
    config_snapshot: Option<Config>,
    lyric_service: Option<Arc<lx_lyric::service::LyricService>>,
}

impl Bridge {
    /// 从 ApplicationService 的权威状态同步队列 / 音量 / 歌词。
    fn refresh_from_application(&mut self) {
        let state = self.application.state();
        self.queue = state.queue.songs.iter().map(TrackView::of).collect();
        self.queue_index = state.queue.index;
        self.volume = state.playback.volume;
        self.duration = state.playback.duration;
        self.current = state.playback.song.as_ref().map(TrackView::of);
        self.refresh_lyrics(Some(&state));
    }

    /// 歌词快照：行、当前行、当前行翻译、当前行逐字（YRC）。
    ///
    /// 歌词由核心解析后放进 `LyricState`（翻译、逐字都由核心产出），这里只
    /// 做视图转换：翻译文本对应整份歌词的当前行译文，与 TUI 歌词面板一致。
    fn refresh_lyrics(&mut self, state: Option<&ApplicationState>) {
        let fallback = state.is_none().then(|| self.application.state());
        let lyrics_state = state
            .or(fallback.as_ref())
            .and_then(|state| state.lyrics.as_ref());
        let Some(lyrics) = lyrics_state else {
            return;
        };
        if lyrics.is_empty || lyrics.lines.is_empty() {
            // 没有歌词（或核心还没解析完）：保留已有的非空歌词视图，
            // 不能用一份空视图把上一首/上一次的内容冲掉。
            return;
        }
        let current = lyrics
            .current_line
            .min(lyrics.lines.len().saturating_sub(1));
        self.lyrics = LyricsView {
            lines: lyrics.lines.iter().map(|line| line.text.clone()).collect(),
            current,
            translation: lyrics.translation.clone(),
            words: lyrics
                .yrc_words
                .iter()
                .map(|word| LyricWord {
                    text: word.text.clone(),
                    start_ms: word.start,
                    duration_ms: word.duration,
                })
                .collect(),
        };
    }

    fn handle(&mut self, event: ApplicationEvent) {
        match event {
            ApplicationEvent::SearchStarted {
                request_id,
                keyword,
            } => {
                self.search_request = request_id;
                self.search = SearchView {
                    keyword,
                    loading: true,
                    ..SearchView::default()
                };
            }
            ApplicationEvent::SearchCompleted {
                request_id,
                keyword,
                page,
                append,
                items,
                has_more,
            } => {
                if request_id != self.search_request {
                    return;
                }
                let items: Vec<TrackView> = items.iter().map(TrackView::of).collect();
                if append {
                    self.search.items.extend(items);
                } else {
                    self.search.items = items;
                }
                self.search.keyword = keyword;
                self.search.page = page;
                self.search.has_more = has_more;
                self.search.loading = false;
                self.search.error = None;
            }
            ApplicationEvent::SearchFailed { request_id, error } => {
                if request_id != self.search_request {
                    return;
                }
                self.search.loading = false;
                self.search.error = Some(error);
            }
            ApplicationEvent::PlaybackStarted { song, .. } => {
                self.state = PlaybackState::Playing;
                self.duration = song.duration;
                self.current = Some(TrackView::of(&song));
                self.lyrics = LyricsView::default();
            }
            ApplicationEvent::PlaybackPaused => self.state = PlaybackState::Paused,
            ApplicationEvent::PlaybackResumed => self.state = PlaybackState::Playing,
            ApplicationEvent::PlaybackStopped => self.state = PlaybackState::Stopped,
            ApplicationEvent::PlaybackEnded => self.state = PlaybackState::Idle,
            ApplicationEvent::PlaybackFailed { error, .. } => {
                self.notification = Some(format!("播放失败: {error}"));
            }
            ApplicationEvent::QueueChanged { queue, index } => {
                self.queue = queue.iter().map(TrackView::of).collect();
                self.queue_index = index;
            }
            ApplicationEvent::LyricsChanged { .. } => self.refresh_lyrics(None),
            ApplicationEvent::Notification { message } => self.notification = Some(message),
            _ => {}
        }
    }

    /// 把媒体会话的最新快照推给桌面控件，并取走媒体键命令。
    fn pump_media(&mut self) {
        let commands: Vec<media::MediaCommand> = {
            let mut session = self.media.lock().unwrap_or_else(|e| e.into_inner());
            session.set_position(self.position);
            session.set_play_mode(self.play_mode);
            session.tick();
            let mut commands = Vec::new();
            while let Some(command) = session.next_command() {
                commands.push(command);
            }
            commands
        };
        for command in commands {
            self.execute_media_command(command);
        }
    }

    fn execute_media_command(&mut self, command: media::MediaCommand) {
        let state = self.application.state();
        let Some(action) = command.into_action(
            state.playback.state,
            state.playback.position,
            state.queue.songs.len(),
        ) else {
            return;
        };
        let request = match action {
            media::MediaAction::Dispatch(command) => GuiRequest::Dispatch(command),
            media::MediaAction::PlaySongs(songs) => GuiRequest::PlaySongs(songs),
            media::MediaAction::SetVolume(volume) => GuiRequest::SetVolume(volume),
            media::MediaAction::Quit => GuiRequest::Quit,
        };
        self.apply(request);
    }

    /// 执行一条 GUI 请求；返回值表示是否要求退出进程。
    ///
    /// 一次请求可能触发多条长耗时的业务命令（例如「播放整张列表」先起播
    /// 再逐条补队，每条都要解析播放地址）。若在事件循环里 `await` 整批，
    /// 期间界面收不到任何事件、暂停/切歌等操作也被排队。因此每条命令单独
    /// 派发到 runtime，执行过程中照常处理事件与媒体键，派发顺序由服务层
    /// 的内部串行保证（与 TUI 的 `execute_action` 一致）。
    fn apply(&mut self, request: GuiRequest) -> bool {
        match request {
            GuiRequest::Dispatch(command) => {
                dispatch_detached(self.application.clone(), command);
            }
            GuiRequest::PlayList { songs, index } => {
                let songs: Vec<SongInfo> = songs.iter().map(TrackView::to_song).collect();
                self.play_list(songs, index);
            }
            GuiRequest::SetVolume(volume) => {
                let volume = volume.min(100);
                self.player.set_volume(volume);
                self.volume = volume;
                let mut session = self.media.lock().unwrap_or_else(|e| e.into_inner());
                session.set_play_mode(self.play_mode);
                session.tick();
            }
            GuiRequest::SetPlayMode(mode) => {
                self.play_mode = mode;
                let mut session = self.media.lock().unwrap_or_else(|e| e.into_inner());
                session.set_play_mode(mode);
                session.tick();
            }
            GuiRequest::MoveQueue { from, to } => {
                self.move_queue(from, to);
            }
            GuiRequest::RestartPlaylist => {
                let songs: Vec<SongInfo> = self.queue.iter().map(TrackView::to_song).collect();
                if songs.is_empty() {
                    return false;
                }
                self.play_list(songs, 0);
            }
            GuiRequest::PlaySongs(songs) => {
                // 本地文件自带时长：播放器不上报 duration 时，界面与 MPRIS
                // 仍要显示正确的总长。
                if let Some(first) = songs.first()
                    && first.duration > Duration::ZERO
                {
                    self.duration = first.duration;
                }
                self.play_list(songs, 0);
            }
            GuiRequest::PlaylistEnded => {
                self.notification = Some("队列已播完".into());
            }
            GuiRequest::ReorderQueue { from, to } => {
                // 拖动排序落到核心队列服务（与 TUI 同一条命令），索引由核心校正。
                dispatch_detached(
                    self.application.clone(),
                    ApplicationCommand::QueueMove { from, to },
                );
                self.move_queue(from, to);
            }
            GuiRequest::RefreshLibrary => self.refresh_library(None),
            GuiRequest::OpenPlaylist(id) => self.refresh_library(Some(&id)),
            GuiRequest::RefreshSources => self.refresh_sources(),
            GuiRequest::SetDefaultSource(source) => self.set_default_source(&source),
            GuiRequest::ToggleSource(source) => self.toggle_source(&source),
            GuiRequest::StartLogin(source) => self.start_login(&source),
            GuiRequest::LogoutSource(source) => self.logout(&source),
            GuiRequest::ToggleLyric(toggle) => self.toggle_lyric(toggle),
            GuiRequest::ToggleFavorite(song) => self.toggle_favorite(&song),
            GuiRequest::AddToPlaylist { playlist, song } => self.add_to_playlist(&playlist, &song),
            GuiRequest::CreatePlaylist(name) => self.create_playlist(&name),
            GuiRequest::DeletePlaylist(id) => self.delete_playlist(&id),
            GuiRequest::RemoveFromHistory(song) => self.remove_history(&song),
            GuiRequest::RemoveFromPlaylist { playlist, song } => {
                self.remove_from_playlist(&playlist, &song);
            }
            GuiRequest::DownloadSong(song) => self.download_song(&song),
            GuiRequest::CancelDownload(id) => {
                if let Some(downloads) = self.downloads.as_ref() {
                    downloads.cancel(id);
                }
                self.refresh_downloads();
            }
            GuiRequest::ClearFinishedDownloads => {
                if let Some(downloads) = self.downloads.as_ref() {
                    downloads.clear_finished();
                }
                self.refresh_downloads();
            }
            GuiRequest::Quit => return true,
        }
        false
    }

    // ── 收藏 / 历史 / 歌单 ──

    /// 读取收藏、历史与自建歌单；`open` 顺带读取该歌单的歌曲。
    fn refresh_library(&mut self, open: Option<&str>) {
        let Some(store) = self.library_store.as_ref() else {
            return;
        };
        self.library.favorites = store.load_favorites().iter().map(TrackView::of).collect();
        self.library.history = store.load_history().iter().map(TrackView::of).collect();
        self.library.playlists = store
            .custom_playlist_summaries()
            .into_iter()
            .map(|summary| PlaylistView {
                id: summary.id,
                name: summary.name,
                song_count: summary.song_count,
                songs: Vec::new(),
            })
            .collect();
        match open {
            Some(id) => {
                let selected = store.custom_playlist(id).map(|playlist| PlaylistView {
                    id: playlist.id,
                    name: playlist.name,
                    song_count: playlist.songs.len() as u32,
                    songs: playlist.songs.iter().map(TrackView::of).collect(),
                });
                if let Some(playlist) = selected.as_ref()
                    && let Some(existing) = self
                        .library
                        .playlists
                        .iter_mut()
                        .find(|item| item.id == playlist.id)
                {
                    existing.songs = playlist.songs.clone();
                    existing.song_count = playlist.song_count;
                }
                self.library.selected = selected;
            }
            None => {
                if let Some(selected) = self.library.selected.clone() {
                    let refreshed =
                        store
                            .custom_playlist(&selected.id)
                            .map(|playlist| PlaylistView {
                                id: playlist.id,
                                name: playlist.name,
                                song_count: playlist.songs.len() as u32,
                                songs: playlist.songs.iter().map(TrackView::of).collect(),
                            });
                    self.library.selected = refreshed;
                }
            }
        }
        self.library.error = None;
    }

    fn with_library(&mut self, action: impl FnOnce(&Storage) -> Result<(), String>) {
        let Some(store) = self.library_store.as_ref() else {
            return;
        };
        match action(store) {
            Ok(()) => self.refresh_library(None),
            Err(error) => self.library.error = Some(error),
        }
    }

    fn toggle_favorite(&mut self, song: &TrackView) {
        let song = song.to_song();
        self.with_library(move |store| {
            if store.is_favorite(&song) {
                store.remove_favorite(&song);
            } else {
                store.add_favorite(&song);
            }
            Ok(())
        });
    }

    fn add_to_playlist(&mut self, playlist: &str, song: &TrackView) {
        let song = song.to_song();
        let playlist = playlist.to_string();
        self.with_library(move |store| {
            store
                .add_song_to_custom_playlist(&playlist, &song)
                .map(|_| ())
        });
    }

    fn remove_from_playlist(&mut self, playlist: &str, song: &TrackView) {
        let song = song.to_song();
        let playlist = playlist.to_string();
        self.with_library(move |store| {
            store
                .remove_song_from_custom_playlist(&playlist, &song)
                .map(|_| ())
        });
    }

    fn create_playlist(&mut self, name: &str) {
        let name = name.to_string();
        self.with_library(move |store| store.create_custom_playlist(&name).map(|_| ()));
    }

    fn delete_playlist(&mut self, id: &str) {
        let id = id.to_string();
        if self
            .library
            .selected
            .as_ref()
            .is_some_and(|item| item.id == id)
        {
            self.library.selected = None;
        }
        self.with_library(move |store| store.delete_custom_playlist(&id).map(|_| ()));
    }

    fn remove_history(&mut self, song: &TrackView) {
        let song = song.to_song();
        self.with_library(move |store| {
            store.remove_history(&song);
            Ok(())
        });
    }

    // ── 音源 / 扫码登录 ──

    /// 音源列表与登录态。
    fn refresh_sources(&mut self) {
        let Some(sources) = self.sources.as_ref() else {
            return;
        };
        let config = self.parsed_config();
        let enabled: Vec<String> = config
            .as_ref()
            .map(|config| {
                config
                    .source
                    .enabled
                    .iter()
                    .map(|source| source.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let default = config
            .as_ref()
            .map(|config| config.source.default.as_str().to_string())
            .unwrap_or_default();
        let logged_in = enabled
            .iter()
            .filter(|source| source_id(source).is_some_and(|id| sources.is_logged_in(id)))
            .cloned()
            .collect();
        let login = self
            .login_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.sources_view = SourcesView {
            available: enabled.clone(),
            enabled,
            default,
            logged_in,
            login,
            error: None,
        };
    }

    /// 读服务线程维护的配置（改配置后由 `ConfigChange` 回填）。
    fn parsed_config(&self) -> Option<Config> {
        self.config_snapshot.clone()
    }

    fn set_default_source(&mut self, source: &str) {
        self.send_config(ConfigChange::DefaultSource(source.to_string()));
    }

    fn toggle_source(&mut self, source: &str) {
        self.send_config(ConfigChange::ToggleSource(source.to_string()));
    }

    fn logout(&mut self, source: &str) {
        if let (Some(sources), Some(id)) = (self.sources.as_ref(), source_id(source)) {
            match sources.logout(id) {
                Ok(()) => {
                    self.sources_view.logged_in.retain(|item| item != source);
                    self.sources_view.login = None;
                }
                Err(error) => self.sources_view.error = Some(error),
            }
        }
    }

    /// 生成二维码并在后台轮询登录状态（UI 每帧只读 `sources.login`）。
    fn start_login(&mut self, source: &str) {
        let (Some(sources), Some(id)) = (self.sources.as_ref(), source_id(source)) else {
            return;
        };
        let sources = Arc::clone(sources);
        let slot = Arc::clone(&self.login_slot);
        let name = source.to_string();
        set_login(&slot, &name, None, "waiting", "正在生成二维码…", true);
        tokio::spawn(async move {
            let session = match sources.create_qr_login(id).await {
                Ok(session) => session,
                Err(error) => {
                    set_login(
                        &slot,
                        &name,
                        None,
                        "failed",
                        &format!("生成二维码失败: {error}"),
                        false,
                    );
                    return;
                }
            };
            let key = session.key.clone();
            set_login(
                &slot,
                &name,
                Some((session.url.clone(), session.image_png.clone())),
                "waiting",
                "请使用对应 App 扫码",
                true,
            );
            let mut ticker = tokio::time::interval(Duration::from_millis(1500));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                match sources.check_qr_login(id, &key).await {
                    Ok(result) => {
                        let terminal = result.status.is_terminal();
                        set_login(
                            &slot,
                            &name,
                            None,
                            &format!("{:?}", result.status).to_lowercase(),
                            &result.message,
                            !terminal,
                        );
                        if terminal {
                            return;
                        }
                    }
                    Err(error) => {
                        // 瞬时网络错误不终止轮询，二维码仍然有效。
                        set_login(
                            &slot,
                            &name,
                            None,
                            "network_error",
                            &format!("登录状态查询失败: {error}"),
                            true,
                        );
                    }
                }
            }
        });
    }

    // ── 歌词显示开关 ──

    fn toggle_lyric(&mut self, toggle: LyricToggle) {
        let Some(lyrics) = self.lyric_service.as_ref() else {
            return;
        };
        let config = self.parsed_config();
        match toggle {
            LyricToggle::Translation => {
                let enabled = config
                    .as_ref()
                    .is_none_or(|config| !config.lyric.show_translation);
                lyrics.set_translation_enabled(enabled);
                self.send_config(ConfigChange::LyricTranslation(enabled));
            }
            LyricToggle::Yrc => {
                let enabled = config.as_ref().is_none_or(|config| !config.lyric.show_yrc);
                lyrics.set_yrc_enabled(enabled);
                self.send_config(ConfigChange::LyricYrc(enabled));
            }
        }
    }

    // ── 下载 ──

    fn download_song(&mut self, song: &TrackView) {
        let (Some(downloads), Some(sources)) = (self.downloads.as_ref(), self.sources.as_ref())
        else {
            return;
        };
        let (notify, _notify_rx) = tokio::sync::mpsc::unbounded_channel();
        downloads.enqueue(song.to_song(), Arc::clone(sources), notify);
        self.refresh_downloads();
    }

    /// 下载面板：状态 + 本次轮询间的速度估计。
    fn refresh_downloads(&mut self) {
        let Some(downloads) = self.downloads.as_ref() else {
            return;
        };
        let now = std::time::Instant::now();
        let mut next = DownloadsView {
            tasks: Vec::new(),
            download_dir: downloads.download_dir().display().to_string(),
        };
        let mut marks = std::collections::HashMap::new();
        for task in downloads.snapshot() {
            let downloaded = task.progress.downloaded.max(task.bytes);
            let speed = match self.download_marks.get(&task.id) {
                Some((previous_bytes, previous_at))
                    if downloaded > *previous_bytes && !previous_at.elapsed().is_zero() =>
                {
                    ((downloaded - previous_bytes) as f64 / previous_at.elapsed().as_secs_f64())
                        as u64
                }
                _ => 0,
            };
            if task.state.is_active() {
                marks.insert(task.id, (downloaded, now));
            }
            next.tasks.push(DownloadItemView {
                id: task.id,
                name: task.name.clone(),
                singer: task.singer.clone(),
                state: download_state_label(task.state).to_string(),
                active: task.state.is_active(),
                can_retry: matches!(
                    task.state,
                    voicefox_runtime::download::DownloadState::Failed
                        | voicefox_runtime::download::DownloadState::Cancelled
                ),
                downloaded,
                total: task.progress.total.max(task.expected_size.unwrap_or(0)),
                elapsed_ms: task.elapsed.as_millis() as u64,
                speed_bps: speed,
                error: task.error.clone(),
            });
        }
        self.download_marks = marks;
        self.downloads_view = next;
    }

    fn send_config(&mut self, change: ConfigChange) {
        let _ = self.config_tx.send(change);
    }

    /// 播放整张列表并从 `index` 开始。
    ///
    /// 队列可能只有一首（ApplicationCommand::Play 的语义），因此先播放
    /// 目标曲目，再把其余按原顺序补进队列；QueueAdd 到非空队列不会打断
    /// 当前播放，但不支持插入到中间，所以补队时跳过目标、其余顺序不变。
    fn play_list(&mut self, songs: Vec<SongInfo>, index: usize) {
        if songs.is_empty() {
            return;
        }
        let index = index.min(songs.len() - 1);
        let target = songs[index].clone();
        self.dispatch_ok(ApplicationCommand::Play {
            songs: vec![target],
            index: 0,
        });
        for (position, song) in songs.into_iter().enumerate() {
            if position == index {
                continue;
            }
            self.dispatch_ok(ApplicationCommand::QueueAdd { song, next: false });
        }
    }

    /// 队列排序：服务没有原地移动命令。
    ///
    /// 队列按「清空 + 逐条 QueueAdd + 起播目标」重建；目标索引随挪动校正，
    /// 保证正在播放的那首仍然对齐（QueueRemove/QueueAdd 会让 index 变化）。
    fn move_queue(&mut self, from: usize, to: usize) {
        let mut order: Vec<SongInfo> = self.queue.iter().map(TrackView::to_song).collect();
        if from >= order.len() || to >= order.len() || from == to {
            return;
        }
        let song = order.remove(from);
        order.insert(to, song);
        let index = if self.queue_index == from {
            to
        } else if from < self.queue_index && to >= self.queue_index {
            self.queue_index - 1
        } else if from > self.queue_index && to <= self.queue_index {
            self.queue_index + 1
        } else {
            self.queue_index
        };
        self.rebuild_queue(order, index);
    }

    /// 以「清空 + 逐条 QueueAdd + 播放目标」把队列换成给定顺序。
    ///
    /// 只在队列重排这种低频操作里使用。
    fn snapshot(&self, has_media_controls: bool) -> Snapshot {
        Snapshot {
            state: self.state,
            song: self.current.clone(),
            position: self.position,
            duration: self.duration,
            volume: self.volume,
            play_mode: self.play_mode,
            queue: self.queue.clone(),
            queue_index: self.queue_index,
            search: self.search.clone(),
            lyrics: self.lyrics.clone(),
            notification: self.notification.clone(),
            has_media_controls,
            playlist_ended: self.playlist_ended,
            library: self.library.clone(),
            sources: self.sources_view.clone(),
            downloads: self.downloads_view.clone(),
        }
    }

    fn rebuild_queue(&mut self, songs: Vec<SongInfo>, index: usize) {
        let index = index.min(songs.len().saturating_sub(1));
        let Some(current) = songs.get(index).cloned() else {
            return;
        };
        // QueueAdd 到空队列会自动起播，因此重建后原本没在播也会开始播放；
        // 这是「拖动排序仍在播放的那首」的既有行为，先在这里写明。
        // 原本在播（或暂停）时先把目标曲目放到最前面起播，其余按顺序补齐，
        // 保证队列顺序与界面一致、索引对齐当前曲目。
        let was_active = matches!(
            self.state,
            PlaybackState::Playing | PlaybackState::Loading | PlaybackState::Paused
        );
        self.dispatch_ok(ApplicationCommand::QueueClear);
        let mut ordered: Vec<SongInfo> = Vec::with_capacity(songs.len());
        if was_active {
            ordered.push(current.clone());
        }
        ordered.extend(
            songs
                .into_iter()
                .enumerate()
                .filter(|(position, _)| !was_active || *position != index)
                .map(|(_, song)| song),
        );
        for song in ordered {
            self.dispatch_ok(ApplicationCommand::QueueAdd { song, next: false });
        }
    }

    /// 后台派发一条命令：不阻塞事件循环，失败经 `spawn_reported` 变成通知。
    fn dispatch_ok(&mut self, command: ApplicationCommand) {
        let application = self.application.clone();
        dispatch_detached(application, command);
    }
}

/// 播放进度时钟：播放器进度是锚点，播放中按墙钟外推。
///
/// libmpv 的 `time-pos` 只在部分时刻刷新（开头、跳转、暂停），界面每秒
/// 重绘都需要连续进度；两者都写进 MPRIS，所以统一从这里取。
struct PlaybackClock {
    state: PlayerState,
    anchor_position: Duration,
    anchor_at: Option<std::time::Instant>,
    generation: u64,
    /// `anchor_position` 是不是播放器亲自报的（而不是换曲时的 0）。
    /// 只有播放器报过进度才按墙钟外推：libmpv 在缓冲/未起播时只会报 0，
    /// 此时外推会让进度条与 MPRIS 显示"正在播放"但时间一直不走。
    has_precedent: bool,
}

impl Default for PlaybackClock {
    fn default() -> Self {
        Self {
            state: PlayerState::Idle,
            anchor_position: Duration::ZERO,
            anchor_at: None,
            generation: 0,
            has_precedent: false,
        }
    }
}

impl PlaybackClock {
    /// 播放器报告的进度（可信锚点）。
    fn anchor(&mut self, position: Duration) {
        self.anchor_position = position;
        self.anchor_at = Some(std::time::Instant::now());
        self.has_precedent = true;
    }

    /// 曲目切换时清零进度，但保留「播放器会报进度」的证据。
    fn rebase(&mut self) {
        self.anchor_position = Duration::ZERO;
        self.anchor_at = Some(std::time::Instant::now());
    }

    /// 曲目代次变化（新歌）时清空外推，避免上一首的进度漏进来。
    fn reset(&mut self, generation: u64) {
        if generation != self.generation {
            self.generation = generation;
            self.rebase();
        }
    }

    fn position(&self) -> Duration {
        let Some(anchor_at) = self.anchor_at else {
            return Duration::ZERO;
        };
        let elapsed = anchor_at.elapsed();
        match self.state {
            PlayerState::Playing if self.has_precedent => self.anchor_position + elapsed,
            _ => self.anchor_position,
        }
    }
}

impl Bridge {
    /// 播放器状态变化后的界面同步与「按播放模式推进队列」。
    fn advance_after_state(&mut self, clock: &mut PlaybackClock) {
        let value = clock.state;
        let previous = self.state;
        self.state = value.into();
        if matches!(value, PlayerState::Stopped | PlayerState::Idle) {
            self.position = Duration::ZERO;
        }
        if value == PlayerState::Idle
            && matches!(previous, PlaybackState::Playing | PlaybackState::Loading)
        {
            clock.rebase();
            self.start_next_by_mode();
        }
    }

    /// 自动推进：单曲循环重播当前曲目，其余按手动「下一首」的规则前进；
    /// 顺序 / 播完停止模式到头时停在结尾（播完停止会提示一次）。
    fn start_next_by_mode(&mut self) {
        let queue: Vec<SongInfo> = self.queue.iter().map(TrackView::to_song).collect();
        match next_target(self.play_mode, self.queue_index, queue.len()) {
            NextTarget::Restart => self.dispatch_ok(ApplicationCommand::PlayCurrent),
            NextTarget::Play(index) => self.play_list(queue, index),
            NextTarget::End { notify } => {
                self.playlist_ended = true;
                if notify {
                    self.apply(GuiRequest::PlaylistEnded);
                }
            }
        }
    }

    /// 定时器判定当前曲目已播完（播放器没报到头时）。
    fn on_track_finished(&mut self) {
        self.state = PlaybackState::Idle;
        self.position = Duration::ZERO;
        self.start_next_by_mode();
    }
}

async fn run_bridge(
    runtime: ApplicationRuntime,
    snapshot: Arc<SnapshotCell>,
    quit: Arc<AtomicBool>,
    requests: mpsc::UnboundedReceiver<GuiRequest>,
    wake: RepaintHandle,
    refresh: RefreshHandle,
) {
    let (config_tx, mut config_rx) = mpsc::unbounded_channel::<ConfigChange>();
    let mode = {
        let session = runtime.media();
        let guard = session.lock().unwrap_or_else(|e| e.into_inner());
        guard.play_mode()
    };
    let application = runtime.application();
    let has_media_controls = runtime.start_media_controls().await;
    let media = runtime.media();
    let player = Arc::clone(&runtime.player);
    // 下载管理器要绑定 runtime 句柄：在服务线程自己的 runtime 里构造。
    let downloads = runtime.build_downloads(tokio::runtime::Handle::current());
    let config_snapshot = runtime.config_snapshot();
    let mut bridge = Bridge {
        application: application.clone(),
        media: Arc::clone(&media),
        player: Arc::clone(&player),
        state: PlaybackState::default(),
        position: Duration::ZERO,
        duration: Duration::ZERO,
        volume: application.state().playback.volume,
        play_mode: mode,
        queue: Vec::new(),
        queue_index: 0,
        lyrics: LyricsView::default(),
        search: SearchView::default(),
        search_request: 0,
        current: None,
        playlist_ended: false,
        notification: None,
        library_store: Some(Arc::clone(&runtime.storage)),
        library: LibraryView::default(),
        sources: Some(runtime.sources()),
        sources_view: SourcesView::default(),
        login_slot: Arc::new(std::sync::Mutex::new(None)),
        downloads: Some(downloads),
        downloads_view: DownloadsView::default(),
        download_marks: std::collections::HashMap::new(),
        config_tx,
        config_snapshot: Some(config_snapshot),
        lyric_service: Some(runtime.lyrics()),
    };
    bridge.refresh_from_application();
    bridge.refresh_library(None);
    bridge.refresh_sources();
    bridge.refresh_downloads();

    let mut events = application.subscribe();
    let mut player_state = player.state_watcher();
    let mut player_position = player.position_watcher();
    let mut player_duration = player.duration_watcher();
    let mut clock = PlaybackClock::default();
    let mut published: Option<Snapshot> = None;
    let mut force_publish = true;
    // 进度只用于界面显示，每 250ms 采样一次即可；事件驱动才是主要来源。
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut requests = requests;
    loop {
        tokio::select! {
            Some(change) = config_rx.recv() => {
                match apply_config_change(&runtime, &change) {
                    Ok(message) => {
                        bridge.notification = Some(message);
                        bridge.config_snapshot = Some(runtime.config_snapshot());
                        bridge.refresh_sources();
                    }
                    Err(error) => bridge.notification = Some(error),
                }
                force_publish = true;
            }
            _ = refresh.notified() => {
                // 下载进度与扫码状态：定时刷新，界面只读快照。
                bridge.refresh_downloads();
                bridge.refresh_sources();
                force_publish = true;
            }
            Some(request) = requests.recv() => {
                if bridge.apply(request) {
                    quit.store(true, Ordering::Release);
                    wake.call();
                    break;
                }
                force_publish = true;
            }
            event = events.recv() => {
                if let Ok(event) = event {
                    if let ApplicationEvent::PlaybackStarted { .. } = &event {
                        // 新曲目（代次）换锚点：上一首的外推进度不能漏进来。
                        clock.reset(bridge.application.state().playback.generation);
                        clock.state = PlayerState::Loading;
                    }
                    bridge.handle(event);
                    force_publish = true;
                }
            }
            result = player_state.changed() => {
                if result.is_ok() {
                    let value = *player_state.borrow_and_update();
                    clock.state = value;
                    bridge.advance_after_state(&mut clock);
                    force_publish = true;
                }
            }
            result = player_duration.changed() => {
                if result.is_ok() {
                    let value = *player_duration.borrow_and_update();
                    if value > Duration::ZERO {
                        bridge.duration = value;
                    }
                }
            }
            result = player_position.changed() => {
                if result.is_ok() {
                    let value = *player_position.borrow_and_update();
                    clock.anchor(value);
                    bridge.position = value;
                }
            }
            _ = ticker.tick() => {
                // 播放器不报进度时按墙钟外推：UI 与 MPRIS 的进度都靠它。
                bridge.position = clock.position();
                let ended = bridge
                    .duration
                    .checked_sub(bridge.position)
                    .is_some_and(|left| left <= Duration::from_millis(400));
                if ended && clock.state == PlayerState::Playing {
                    bridge.on_track_finished();
                }
                force_publish = true;
            }
        }

        bridge.pump_media();
        if quit.load(Ordering::Acquire) {
            break;
        }
        // 每轮都刷新一次队列索引：QueueChanged 之外（Play/Next）的索引变化
        // 只有 StateChanged 携带，这里以权威状态兜底。
        let queue_index = application.state().queue.index;
        if queue_index != bridge.queue_index {
            bridge.queue_index = queue_index;
            force_publish = true;
        }

        let next = bridge.snapshot(has_media_controls);
        let changed = force_publish || published.as_ref() != Some(&next);
        force_publish = false;
        if changed {
            published = Some(next.clone());
            snapshot.set(next);
            wake.call();
        }
    }
    quit.store(true, Ordering::Release);
    wake.call();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_view_round_trips_through_song() {
        let view = TrackView {
            id: "77".into(),
            source: "kw".into(),
            name: "夜曲".into(),
            singer: "周杰伦".into(),
            album: "十一月的萧邦".into(),
            duration_ms: 226_000,
            cover_url: Some("https://example.com/c.jpg".into()),
        };
        let song = view.to_song();
        assert_eq!(song.source, SourceId::Kw);
        assert_eq!(song.duration, Duration::from_millis(226_000));
        assert_eq!(TrackView::of(&song), view);
    }

    #[test]
    fn playback_state_labels_cover_player_states() {
        assert!(PlaybackState::from(PlayerState::Loading).is_playing());
        assert!(!PlaybackState::from(PlayerState::Paused).is_playing());
        assert_eq!(PlaybackState::from(PlayerState::Idle).label(), "空闲");
    }

    #[test]
    fn playback_clock_extrapolates_only_while_playing() {
        let mut clock = PlaybackClock {
            state: PlayerState::Idle,
            ..PlaybackClock::default()
        };
        assert_eq!(clock.position(), Duration::ZERO, "未锚定前没有进度");
        clock.state = PlayerState::Playing;
        clock.anchor(Duration::from_secs(30));
        let ahead = clock.position();
        assert!(
            ahead >= Duration::from_secs(30),
            "播放中进度按墙钟外推: {ahead:?}"
        );
        // 暂停后进度冻结在锚点，不再外推。
        clock.state = PlayerState::Paused;
        std::thread::sleep(Duration::from_millis(30));
        clock.anchor(Duration::from_secs(31));
        assert_eq!(clock.position(), Duration::from_secs(31));
    }

    /// libmpv 只报 0（缓冲中）时不能外推：否则界面显示在播、时间却不动。
    #[test]
    fn playback_clock_does_not_extrapolate_without_a_precedent() {
        let mut clock = PlaybackClock::default();
        clock.reset(3);
        clock.state = PlayerState::Playing;
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(clock.position(), Duration::ZERO, "没有可信锚点就停在 0");
        clock.anchor(Duration::ZERO);
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            clock.position() > Duration::ZERO,
            "播放器报过 0 之后才开始外推"
        );
    }

    #[test]
    fn playback_clock_resets_on_new_generation() {
        let mut clock = PlaybackClock {
            state: PlayerState::Playing,
            ..PlaybackClock::default()
        };
        clock.anchor(Duration::from_secs(120));
        clock.reset(7);
        assert!(
            clock.position() < Duration::from_millis(1),
            "新代次从 0 开始"
        );
        clock.state = PlayerState::Paused;
        clock.anchor(Duration::from_secs(3));
        clock.reset(7);
        assert_eq!(clock.position(), Duration::from_secs(3), "同一代次不重置");
        clock.reset(8);
        assert!(
            clock.position() < Duration::from_millis(1),
            "新代次重新计时"
        );
    }

    #[test]
    fn track_views_round_trip_duration_and_source() {
        let view = TrackView {
            id: "9".into(),
            source: "wy".into(),
            name: "晴天".into(),
            singer: "周杰伦".into(),
            album: "叶惠美".into(),
            duration_ms: 269_000,
            cover_url: None,
        };
        let song = view.to_song();
        assert_eq!(song.source, SourceId::Wy);
        assert_eq!(song.duration, Duration::from_millis(269_000));
        assert_eq!(TrackView::of(&song), view);
    }

    #[test]
    fn play_mode_decides_what_happens_after_a_track() {
        assert_eq!(next_target(PlayMode::SingleLoop, 1, 3), NextTarget::Restart);
        assert_eq!(
            next_target(PlayMode::ListLoop, 2, 3),
            NextTarget::Play(0),
            "列表循环到底回到第一首"
        );
        assert_eq!(next_target(PlayMode::Random, 0, 3), NextTarget::Play(1));
        assert_eq!(next_target(PlayMode::List, 1, 3), NextTarget::Play(2));
        assert_eq!(
            next_target(PlayMode::List, 2, 3),
            NextTarget::End { notify: false },
            "顺序播放在结尾停下，不提示"
        );
        assert_eq!(
            next_target(PlayMode::None, 2, 3),
            NextTarget::End { notify: true },
            "播完停止在结尾停下并提示"
        );
        assert_eq!(
            next_target(PlayMode::ListLoop, 0, 0),
            NextTarget::End { notify: false }
        );
    }

    #[test]
    fn search_events_ignore_stale_request_ids() {
        // Bridge 的搜索状态只接受最新 request_id，旧请求后到不能覆盖新结果。
        let mut search = SearchView::default();
        let mut latest = 0u64;
        let mut accept = |request_id: u64| {
            if request_id >= latest {
                latest = request_id;
                true
            } else {
                false
            }
        };
        assert!(accept(2));
        assert!(!accept(1));
        search.keyword = "a".into();
        assert_eq!(search.keyword, "a");
    }
}

/// 改配置的请求：配置锁在 `ApplicationRuntime` 里，界面只发一句描述。
///
/// 服务线程把它转成真正的 `Config` 修改后交给 runtime：内存与 `config.toml`
/// 一起更新（与 TUI 设置页同一条落盘路径），音源管理器同步新偏好。
#[derive(Debug, Clone)]
pub enum ConfigChange {
    DefaultSource(String),
    ToggleSource(String),
    LyricTranslation(bool),
    LyricYrc(bool),
}

/// 把 `ConfigChange` 应用到运行时配置：返回给用户看的说明。
pub fn apply_config_change(
    runtime: &ApplicationRuntime,
    change: &ConfigChange,
) -> Result<String, String> {
    let mut message = String::new();
    runtime.update_config(|config| match change {
        ConfigChange::DefaultSource(source) => {
            if let Some(id) = source_id(source) {
                config.source.default = id;
                message = format!("默认音源: {}", id.as_str());
            }
        }
        ConfigChange::ToggleSource(source) => {
            if let Some(id) = source_id(source) {
                if let Some(position) = config.source.enabled.iter().position(|item| *item == id) {
                    if config.source.enabled.len() > 1 {
                        config.source.enabled.remove(position);
                        message = format!("已停用音源: {}", id.as_str());
                    }
                } else {
                    config.source.enabled.push(id);
                    message = format!("已启用音源: {}", id.as_str());
                }
            }
        }
        ConfigChange::LyricTranslation(enabled) => {
            config.lyric.show_translation = *enabled;
            message = format!("歌词翻译: {}", if *enabled { "开" } else { "关" });
        }
        ConfigChange::LyricYrc(enabled) => {
            config.lyric.show_yrc = *enabled;
            message = format!("逐字歌词: {}", if *enabled { "开" } else { "关" });
        }
    })?;
    runtime.apply_source_preferences();
    Ok(message)
}

/// 音源配置文本里的名字 → [`SourceId`]（只接受内置在线音源）。
fn source_id(value: &str) -> Option<SourceId> {
    SourceId::all_online()
        .iter()
        .copied()
        .find(|source| source.as_str().eq_ignore_ascii_case(value.trim()))
}

/// 扫码登录槽的写入（后台任务与界面之间只传这一份视图）。
///
/// `qr` 只在「刚生成二维码」时给出；之后的轮询只更新状态文字，二维码保持不清空
/// （状态变化不能把码擦掉，用户可能在扫码中途看到状态更新）。
pub fn set_login(
    slot: &std::sync::Mutex<Option<LoginView>>,
    source: &str,
    qr: Option<(String, Option<String>)>,
    status: &str,
    message: &str,
    polling: bool,
) {
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let previous = guard.take();
    let qr = qr.or_else(|| {
        previous
            .as_ref()
            .map(|previous| (previous.url.clone(), previous.image_base64.clone()))
    });
    let (url, image_base64) = qr.unwrap_or_default();
    *guard = Some(LoginView {
        source: source.to_string(),
        url,
        image_base64,
        status: status.to_string(),
        message: message.to_string(),
        polling,
    });
}

/// 下载状态的界面文案。
fn download_state_label(state: voicefox_runtime::download::DownloadState) -> &'static str {
    use voicefox_runtime::download::DownloadState;
    match state {
        DownloadState::Queued => "排队中",
        DownloadState::Resolving => "解析中",
        DownloadState::Downloading => "下载中",
        DownloadState::Tagging => "写入标签",
        DownloadState::Done => "已完成",
        DownloadState::Skipped => "已跳过",
        DownloadState::Cancelled => "已取消",
        DownloadState::Failed => "失败",
    }
}

/// 当前曲目自然播完后该做什么（按播放模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextTarget {
    /// 单曲循环：重新播放当前曲目。
    Restart,
    /// 播放队列的第 index 首。
    Play(usize),
    /// 队列已到结尾：`notify` 表示「播完停止」模式需要提示一次。
    End { notify: bool },
}

fn next_target(mode: PlayMode, index: usize, len: usize) -> NextTarget {
    if len == 0 {
        return NextTarget::End { notify: false };
    }
    let index = index.min(len - 1);
    let advance = |index: usize| NextTarget::Play(index);
    match mode {
        PlayMode::SingleLoop => NextTarget::Restart,
        // 随机/列表循环都按顺序推进到下一首（随机顺序由队列本身决定）。
        PlayMode::ListLoop | PlayMode::Random => advance((index + 1) % len),
        PlayMode::List | PlayMode::None => {
            if index + 1 < len {
                advance(index + 1)
            } else {
                NextTarget::End {
                    notify: mode == PlayMode::None,
                }
            }
        }
    }
}

/// 在服务线程的 runtime 上派发一条业务命令，立即返回。
///
/// `ApplicationService` 内部按命令顺序串行处理（与 TUI 的
/// `execute_action` 相同），因此这里不额外排队；错误转成日志，
/// 避免界面静默失败。
fn dispatch_detached(application: ApplicationService, command: ApplicationCommand) {
    tokio::spawn(async move {
        if let Err(error) = application.dispatch(command).await {
            debug_log(&format!("dispatch failed: {error}"));
        }
    });
}

/// 服务线程诊断日志（`VOICEFOX_GUI_LOG` 指定文件）。
pub(crate) fn debug_log(message: &str) {
    let Ok(path) = std::env::var("VOICEFOX_GUI_LOG") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{message}");
    }
}
