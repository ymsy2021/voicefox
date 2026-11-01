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
    GuiBackend, GuiError, GuiRequest, PlaybackState, RepaintHandle, SearchView, Snapshot, TrackView,
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
        }
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

    fn clone_backend(&self) -> Box<dyn GuiBackend> {
        Box::new(Arc::clone(self))
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
        GuiRequest::Quit => state.closed = true,
    }
    repaint.call();
    Ok(())
}
