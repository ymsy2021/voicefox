//! 演示 / 无网络用的假后端：按脚本推进搜索、播放、队列与媒体键状态。
//!
//! 用途：
//! - `voicefox-gui-demo` 在没有网络 / 音频设备的环境里演示整套界面；
//! - UI 快照测试用同一份状态脚本核对界面内容。
//!
//! 它刻意只实现 [`GuiBackend`]，不依赖 ApplicationService，因此不会真的
//! 发起网络请求或播放声音。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::service::{
    DownloadItemView, GuiBackend, GuiError, GuiRequest, LibraryView, LoginView, LyricToggle,
    LyricWord, LyricsView, PlaybackState, PlaylistView, RepaintHandle, SearchView, Snapshot,
    SourcesView, TrackView,
};
use voicefox_runtime::media::PlayMode;

/// 演示曲目（用于搜索结果的种子数据）。
const DEMO_SONGS: &[(&str, &str, &str, &str, u64)] = &[
    ("1", "周杰伦", "夜曲", "十一月的萧邦", 226_000),
    ("2", "周杰伦", "七里香", "七里香", 299_000),
    ("3", "林俊杰", "晴天", "叶惠美", 269_000),
    ("4", "五月天", "倔强", "神的孩子都在跳舞", 259_000),
    ("5", "陈奕迅", "十年", "黑白灰", 205_000),
    ("6", "Taylor Swift", "Love Story", "Fearless", 235_000),
];

/// 演示后端的状态机。
pub struct DemoBackend {
    state: Arc<Mutex<DemoState>>,
    repaint: RepaintHandle,
}

impl DemoBackend {
    /// 轻量句柄：共享同一份状态与重绘回调（不持有整台后端）。
    pub fn handle(&self) -> DemoHandle {
        DemoHandle {
            state: Arc::clone(&self.state),
            repaint: self.repaint.clone(),
        }
    }
}

/// 演示后端的分身：与 [`DemoBackend`] 共享状态。
#[derive(Clone)]
pub struct DemoHandle {
    state: Arc<Mutex<DemoState>>,
    repaint: RepaintHandle,
}

impl DemoBackend {
    /// 演示脚本用：直接给歌词设一段翻译（真实现由核心解析）。
    pub fn set_lyric_translation(&self, translation: Option<String>) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .lyrics
            .translation = translation;
    }
}

impl DemoHandle {
    /// 读当前快照（测试与演示脚本用）。
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
    }

    pub fn requests(&self) -> Vec<GuiRequest> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .requests
            .clone()
    }
}

impl GuiBackend for DemoHandle {
    fn send(&self, request: GuiRequest) -> Result<(), GuiError> {
        send_to(&self.state, &self.repaint, request)
    }

    fn take_snapshot(&self) -> Snapshot {
        self.snapshot()
    }

    fn quit_requested(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    fn repaint_handle(&self) -> RepaintHandle {
        self.repaint.clone()
    }

    fn request_refresh(&self) {
        // 演示后端的下载 / 扫码都是即时状态，不需要额外刷新。
    }

    fn clone_backend(&self) -> Box<dyn GuiBackend> {
        Box::new(self.clone())
    }
}

struct DemoState {
    snapshot: Snapshot,
    queue: Vec<TrackView>,
    closed: bool,
    requests: Vec<GuiRequest>,
}

impl DemoBackend {
    pub fn new() -> Self {
        Self::with_queue_preset(false)
    }

    /// `queue_preset`：预先放好 3 首歌并"正在播放"第一首，用于队列快照测试。
    pub fn with_queue_preset(queue_preset: bool) -> Self {
        let mut snapshot = Snapshot::default();
        if queue_preset {
            let queue: Vec<TrackView> = DEMO_SONGS
                .iter()
                .take(3)
                .map(|(id, singer, name, album, duration)| {
                    track(id, singer, name, album, *duration)
                })
                .collect();
            snapshot.song = queue.first().cloned();
            snapshot.state = PlaybackState::Playing;
            snapshot.duration = Duration::from_millis(226_000);
            snapshot.position = Duration::from_secs(42);
            snapshot.queue = queue;
            snapshot.queue_index = 0;
            snapshot.search.keyword = "周杰伦".into();
            snapshot.search.items = DEMO_SONGS
                .iter()
                .map(|(id, singer, name, album, duration)| {
                    track(id, singer, name, album, *duration)
                })
                .collect();
            snapshot.lyrics.lines = vec![
                "一群嗜血的蚂蚁 被腐肉所吸引".into(),
                "我面无表情 看孤独的风景".into(),
                "失去你 爱恨开始分明".into(),
            ];
            snapshot.lyrics.current = 1;
            snapshot.lyrics.translation = None;
        }
        // 演示后端没有真实音源管理器：给一份与默认配置一致的音源列表。
        snapshot.sources = SourcesView {
            available: vec![
                "kw".into(),
                "kg".into(),
                "tx".into(),
                "wy".into(),
                "mg".into(),
                "bili".into(),
                "qianqian".into(),
            ],
            enabled: vec![
                "kw".into(),
                "kg".into(),
                "tx".into(),
                "wy".into(),
                "mg".into(),
            ],
            default: "kw".into(),
            logged_in: Vec::new(),
            login: None,
            error: None,
        };
        snapshot.library = demo_library();
        snapshot.downloads.download_dir = "~/.local/share/voicefox/downloads".into();
        Self {
            state: Arc::new(Mutex::new(DemoState {
                snapshot,
                queue: Vec::new(),
                closed: false,
                requests: Vec::new(),
            })),
            repaint: RepaintHandle::default(),
        }
    }

    /// 观察界面发出过的请求（快照测试断言用）。
    pub fn requests(&self) -> Vec<GuiRequest> {
        self.handle().requests()
    }

    /// 开始把演示状态推到界面上（每 100ms 一次，播放进度随之推进）。
    pub fn drive(self: &Arc<Self>) {
        let this = Arc::clone(self);
        std::thread::Builder::new()
            .name("voicefox-demo".into())
            .spawn(move || {
                loop {
                    {
                        let Ok(mut state) = this.state.lock() else {
                            return;
                        };
                        if state.closed {
                            return;
                        }
                        if state.snapshot.state.is_playing() {
                            state.snapshot.position += Duration::from_millis(100);
                            if state.snapshot.duration > Duration::ZERO
                                && state.snapshot.position >= state.snapshot.duration
                            {
                                state.snapshot.position = Duration::ZERO;
                            }
                        }
                    }
                    this.repaint.call();
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .ok();
    }
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn track(id: &str, singer: &str, name: &str, album: &str, duration_ms: u64) -> TrackView {
    TrackView {
        id: id.into(),
        source: "demo".into(),
        name: name.into(),
        singer: singer.into(),
        album: album.into(),
        duration_ms,
        cover_url: None,
    }
}

impl GuiBackend for DemoBackend {
    fn send(&self, request: GuiRequest) -> Result<(), GuiError> {
        send_to(&self.state, &self.repaint, request)
    }

    fn take_snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot
            .clone()
    }

    fn quit_requested(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    fn repaint_handle(&self) -> RepaintHandle {
        self.repaint.clone()
    }

    fn request_refresh(&self) {
        // 演示后端的下载 / 扫码都是即时状态，不需要额外刷新。
    }

    fn clone_backend(&self) -> Box<dyn GuiBackend> {
        Box::new(self.handle())
    }
}

/// `Arc<DemoBackend>` 直接可用作后端：演示程序与 UI 快照测试都持有共享的
/// 后端实例（既能驱动界面，也能读取请求记录）。
impl GuiBackend for std::sync::Arc<DemoBackend> {
    fn send(&self, request: GuiRequest) -> Result<(), GuiError> {
        DemoBackend::send(self, request)
    }

    fn take_snapshot(&self) -> Snapshot {
        DemoBackend::take_snapshot(self)
    }

    fn quit_requested(&self) -> bool {
        DemoBackend::quit_requested(self)
    }

    fn repaint_handle(&self) -> RepaintHandle {
        DemoBackend::repaint_handle(self)
    }

    fn request_refresh(&self) {
        // 演示后端的下载 / 扫码都是即时状态，不需要额外刷新。
    }

    fn clone_backend(&self) -> Box<dyn GuiBackend> {
        Box::new(Arc::clone(self))
    }
}

/// 原地移动一首歌并把当前曲目的索引校正到新位置（与 `QueueService::move_song` 同规则）。
fn move_track(queue: &mut Vec<TrackView>, current: usize, from: usize, to: usize) -> Option<usize> {
    if from >= queue.len() || to >= queue.len() || from == to {
        return None;
    }
    let song = queue.remove(from);
    queue.insert(to, song);
    let next = if current == from {
        to
    } else if from < current && to >= current {
        current - 1
    } else if from > current && to <= current {
        current + 1
    } else {
        current
    };
    Some(next)
}

/// 演示歌词：三行 + 第二行逐字 + 当前行译文占位。
fn demo_lyrics() -> LyricsView {
    LyricsView {
        lines: vec![
            "一群嗜血的蚂蚁 被腐肉所吸引".into(),
            "我面无表情 看孤独的风景".into(),
            "失去你 爱恨开始分明".into(),
        ],
        current: 1,
        translation: None,
        words: vec![
            LyricWord {
                text: "我面无表情".into(),
                start_ms: 4_000,
                duration_ms: 1_500,
            },
            LyricWord {
                text: " 看孤独的风景".into(),
                start_ms: 5_500,
                duration_ms: 1_500,
            },
        ],
    }
}

/// 演示用的收藏 / 历史 / 歌单数据。
fn demo_library() -> LibraryView {
    let track = |id: &str, singer: &str, name: &str, album: &str, duration: u64| TrackView {
        id: id.into(),
        source: "demo".into(),
        name: name.into(),
        singer: singer.into(),
        album: album.into(),
        duration_ms: duration,
        cover_url: None,
    };
    let songs = vec![
        track("1", "周杰伦", "夜曲", "十一月的萧邦", 226_000),
        track("2", "周杰伦", "七里香", "七里香", 299_000),
        track("3", "林俊杰", "晴天", "叶惠美", 269_000),
    ];
    LibraryView {
        favorites: songs.clone(),
        history: songs.iter().take(2).cloned().collect(),
        playlists: vec![PlaylistView {
            id: "demo-1".into(),
            name: "通勤歌单".into(),
            song_count: 2,
            songs: songs.iter().take(2).cloned().collect(),
        }],
        selected: None,
        error: None,
    }
}

fn start(snapshot: &mut Snapshot, song: TrackView, index: usize) {
    snapshot.duration = Duration::from_millis(song.duration_ms);
    snapshot.position = Duration::ZERO;
    snapshot.queue_index = index;
    snapshot.song = Some(song);
    snapshot.state = PlaybackState::Playing;
    snapshot.play_mode = if snapshot.play_mode == PlayMode::None {
        PlayMode::ListLoop
    } else {
        snapshot.play_mode
    };
}
/// 演示后端与它的分身共用的请求处理。
fn send_to(
    state: &Mutex<DemoState>,
    repaint: &RepaintHandle,
    request: GuiRequest,
) -> Result<(), GuiError> {
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    state.requests.push(request.clone());
    match request {
        GuiRequest::Dispatch(command) => {
            use voicefox_application::ApplicationCommand as Cmd;
            match command {
                Cmd::Search { keyword, .. } => {
                    state.snapshot.search = SearchView {
                        keyword: keyword.clone(),
                        loading: true,
                        ..SearchView::default()
                    };
                    // 立即给结果（真实实现是异步多音源检索）。
                    let items: Vec<TrackView> = DEMO_SONGS
                        .iter()
                        .filter(|(_, singer, name, _, _)| {
                            name.contains(&keyword) || singer.contains(&keyword)
                        })
                        .map(|(id, singer, name, album, duration)| {
                            track(id, singer, name, album, *duration)
                        })
                        .collect();
                    let items = if items.is_empty() {
                        DEMO_SONGS
                            .iter()
                            .map(|(id, singer, name, album, duration)| {
                                track(id, singer, name, album, *duration)
                            })
                            .collect()
                    } else {
                        items
                    };
                    state.snapshot.search.loading = false;
                    state.snapshot.search.items = items;
                    state.snapshot.search.has_more = true;
                    state.snapshot.search.page = 1;
                }
                Cmd::Toggle => {
                    state.snapshot.state = if state.snapshot.state.is_playing() {
                        PlaybackState::Paused
                    } else if state.snapshot.song.is_some() {
                        PlaybackState::Playing
                    } else if let Some(first) = state.snapshot.queue.first().cloned() {
                        start(&mut state.snapshot, first, 0);
                        PlaybackState::Playing
                    } else {
                        state.snapshot.state
                    };
                }
                Cmd::Pause => state.snapshot.state = PlaybackState::Paused,
                Cmd::Resume => state.snapshot.state = PlaybackState::Playing,
                Cmd::Stop => state.snapshot.state = PlaybackState::Stopped,
                Cmd::Next => {
                    let len = state.snapshot.queue.len();
                    if len > 0 {
                        let index = (state.snapshot.queue_index + 1) % len;
                        let song = state.snapshot.queue[index].clone();
                        start(&mut state.snapshot, song, index);
                    }
                }
                Cmd::Previous => {
                    let len = state.snapshot.queue.len();
                    if len > 0 {
                        let index = (state.snapshot.queue_index + len - 1) % len;
                        let song = state.snapshot.queue[index].clone();
                        start(&mut state.snapshot, song, index);
                    }
                }
                Cmd::Seek(position) => state.snapshot.position = position,
                Cmd::QueueAdd { song, .. } => {
                    let view = TrackView {
                        id: song.id.clone(),
                        source: song.source.as_str().to_string(),
                        name: song.name.clone(),
                        singer: song.singer.clone(),
                        album: song.album_name.clone(),
                        duration_ms: song.duration.as_millis() as u64,
                        cover_url: song.cover_url.clone(),
                    };
                    state.snapshot.queue.push(view);
                    if state.snapshot.song.is_none() {
                        let first = state.snapshot.queue[0].clone();
                        start(&mut state.snapshot, first, 0);
                    }
                }
                Cmd::QueueRemove { index } => {
                    if index < state.snapshot.queue.len() {
                        state.snapshot.queue.remove(index);
                        if state.snapshot.queue_index >= state.snapshot.queue.len()
                            && !state.snapshot.queue.is_empty()
                        {
                            state.snapshot.queue_index = state.snapshot.queue.len() - 1;
                        }
                    }
                }
                Cmd::QueueClear => {
                    state.snapshot.queue.clear();
                    state.snapshot.queue_index = 0;
                    state.snapshot.song = None;
                    state.snapshot.state = PlaybackState::Idle;
                    state.snapshot.position = Duration::ZERO;
                    state.snapshot.duration = Duration::ZERO;
                }
                other => {
                    state.snapshot.notification = Some(format!("演示模式未模拟该命令: {other:?}"));
                }
            }
        }
        GuiRequest::PlayList { songs, index } => {
            // 与真实服务一致：请求自带的列表成为队列，播放第 index 首。
            // 真实实现把目标放到队首再补队，演示直接保持原序便于观察。
            if songs.is_empty() {
                return Ok(());
            }
            let index = index.min(songs.len() - 1);
            let song = songs[index].clone();
            state.queue = songs;
            state.snapshot.queue = state.queue.clone();
            start(&mut state.snapshot, song, index);
            // 真实链路里新曲目的歌词由核心重新加载；演示直接换上示例歌词。
            state.snapshot.lyrics = demo_lyrics();
        }
        GuiRequest::SetVolume(volume) => {
            state.snapshot.volume = volume.min(100);
        }
        GuiRequest::SetPlayMode(mode) => {
            state.snapshot.play_mode = mode;
        }
        GuiRequest::MoveQueue { from, to } => {
            let queue = &mut state.snapshot.queue;
            if from < queue.len() && to < queue.len() && from != to {
                let song = queue.remove(from);
                queue.insert(to, song);
                state.snapshot.queue_index = if state.snapshot.queue_index == from {
                    to
                } else {
                    state.snapshot.queue_index
                };
            }
        }
        GuiRequest::PlaySongs(songs) => {
            state.queue = songs
                .iter()
                .map(|song| TrackView {
                    id: song.id.clone(),
                    source: song.source.as_str().to_string(),
                    name: song.name.clone(),
                    singer: song.singer.clone(),
                    album: song.album_name.clone(),
                    duration_ms: song.duration.as_millis() as u64,
                    cover_url: song.cover_url.clone(),
                })
                .collect();
            state.snapshot.queue = state.queue.clone();
            if let Some(first) = state.snapshot.queue.first().cloned() {
                start(&mut state.snapshot, first, 0);
            }
        }
        GuiRequest::RestartPlaylist => {
            if let Some(first) = state.snapshot.queue.first().cloned() {
                start(&mut state.snapshot, first, 0);
            }
        }
        GuiRequest::PlaylistEnded => {
            state.snapshot.notification = Some("队列已播完".into());
        }
        // P1 的页面在演示后端里用本地示例数据模拟（不访问网络）。
        GuiRequest::ReorderQueue { from, to } => {
            // 与核心 `QueueMove` 一致：当前曲目的索引随移动校正。
            let current = state.snapshot.queue_index;
            if let Some(index) = move_track(&mut state.snapshot.queue, current, from, to) {
                state.snapshot.queue_index = index;
            }
        }
        GuiRequest::RefreshLibrary | GuiRequest::OpenPlaylist(_) => {
            state.snapshot.library = demo_library();
        }
        GuiRequest::ToggleFavorite(song) => {
            let library = state.snapshot.library.clone();
            let already = library.favorites.iter().any(|item| item.id == song.id);
            let favorites = if already {
                library
                    .favorites
                    .into_iter()
                    .filter(|item| item.id != song.id)
                    .collect()
            } else {
                let mut favorites = library.favorites;
                favorites.insert(0, song);
                favorites
            };
            state.snapshot.library.favorites = favorites;
        }
        GuiRequest::AddToPlaylist { playlist, song } => {
            let mut library = state.snapshot.library.clone();
            if let Some(target) = library
                .playlists
                .iter_mut()
                .find(|item| item.id == playlist)
            {
                target.songs.push(song);
                target.song_count += 1;
            }
            state.snapshot.library = library;
        }
        GuiRequest::RemoveFromPlaylist { playlist, song } => {
            let mut library = state.snapshot.library.clone();
            if let Some(target) = library
                .playlists
                .iter_mut()
                .find(|item| item.id == playlist)
            {
                target.songs.retain(|item| item.id != song.id);
                target.song_count = target.songs.len() as u32;
            }
            state.snapshot.library = library;
        }
        GuiRequest::RemoveFromHistory(song) => {
            state
                .snapshot
                .library
                .history
                .retain(|item| item.id != song.id);
        }
        GuiRequest::CreatePlaylist(name) => {
            let mut library = state.snapshot.library.clone();
            let id = format!("demo-{}", library.playlists.len() + 1);
            library.playlists.push(PlaylistView {
                id,
                name,
                song_count: 0,
                songs: Vec::new(),
            });
            state.snapshot.library = library;
        }
        GuiRequest::DeletePlaylist(id) => {
            state
                .snapshot
                .library
                .playlists
                .retain(|item| item.id != id);
        }
        GuiRequest::RefreshSources => {}
        GuiRequest::SetDefaultSource(source) => {
            state.snapshot.sources.default = source;
        }
        GuiRequest::ToggleSource(source) => {
            let sources = &mut state.snapshot.sources;
            if sources.enabled.contains(&source) {
                sources.enabled.retain(|item| *item != source);
            } else {
                sources.enabled.push(source);
            }
        }
        GuiRequest::StartLogin(source) => {
            state.snapshot.sources.login = Some(LoginView {
                source,
                url: "https://example.com/demo-qr".into(),
                image_base64: None,
                status: "waiting".into(),
                message: "演示模式：请用对应 App 扫码".into(),
                polling: true,
            });
        }
        GuiRequest::LogoutSource(source) => {
            state
                .snapshot
                .sources
                .logged_in
                .retain(|item| *item != source);
            state.snapshot.sources.login = None;
        }
        GuiRequest::ToggleLyric(toggle) => match toggle {
            LyricToggle::Translation => {
                // 播放中，翻译始终由核心给出；这里演示「当前行译文」内容。
                state.snapshot.lyrics.translation = Some("（演示）翻译行".into());
            }
            LyricToggle::Yrc => {
                state.snapshot.lyrics.words = if state.snapshot.lyrics.words.is_empty() {
                    vec![LyricWord {
                        text: "演示逐字".into(),
                        start_ms: 0,
                        duration_ms: 800,
                    }]
                } else {
                    Vec::new()
                };
            }
        },
        GuiRequest::DownloadSong(song) => {
            let downloads = &mut state.snapshot.downloads;
            let id = downloads.tasks.len() as u64 + 1;
            downloads.tasks.push(DownloadItemView {
                id,
                name: song.name,
                singer: song.singer,
                state: "下载中".into(),
                active: true,
                can_retry: false,
                downloaded: 4096,
                total: 8192,
                elapsed_ms: 1000,
                speed_bps: 4096,
                error: None,
            });
            downloads.download_dir = "~/.local/share/voicefox/downloads".into();
        }
        GuiRequest::CancelDownload(id) => {
            if let Some(task) = state
                .snapshot
                .downloads
                .tasks
                .iter_mut()
                .find(|task| task.id == id)
            {
                task.state = "已取消".into();
                task.active = false;
                task.can_retry = true;
            }
        }
        GuiRequest::ClearFinishedDownloads => {
            state.snapshot.downloads.tasks.retain(|task| task.active);
        }
        GuiRequest::Quit => state.closed = true,
    }
    repaint.call();
    Ok(())
}
