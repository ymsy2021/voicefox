use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use lx_core::model::song::SongInfo;
use lx_core::model::source::{Quality, SourceId};
use lx_core::traits::player::PlayerBackend;
use lx_core::traits::source::SongUrl;
use lx_source::manager::SourceManager;

#[async_trait]
pub trait PlaybackEffects: Send + Sync {
    fn existing_download(&self, song: &SongInfo) -> Option<PathBuf>;
    fn record_history(&self, song: &SongInfo);
    fn prepare_cover(&self, song: &SongInfo, show_cover: bool) -> Option<String>;
    async fn load_cover(&self, url: Option<String>) -> Result<(), String>;
    async fn cache_cover(&self, url: Option<String>) -> Option<String>;
    /// 地址体检（不发网络请求）：音源常回残缺地址（只剩域名之类），
    /// 这类地址请求只会拿到 400/403 或 HTML 首页，必须提前挡掉。
    fn cover_url_is_usable(&self, url: &str) -> bool;
    fn album_cover_notification(&self) -> bool;
    fn track_change_notification(&self) -> bool;
    fn fade_in_ms(&self) -> u64;
    fn quality(&self) -> Quality;
    fn auto_toggle(&self) -> bool;
}

/// 解析「最终可用的封面地址」，播放解析与 app 侧播放路径共用一份：
///
/// - 队列里的地址优先；播放地址自带的封面次之；
/// - 地址不合格时跨源重找一次（带 `timeout` 预算）；
/// - 都拿不到合格地址时**保留原值**，让封面面板把失败原因显示出来。
pub async fn resolve_cover_url(
    sources: &SourceManager,
    song: &SongInfo,
    from_song_url: Option<String>,
    usable: impl Fn(&str) -> bool,
    timeout: Duration,
) -> Option<String> {
    let mut resolved = song.cover_url.clone().or(from_song_url);
    if resolved.as_deref().is_some_and(&usable) {
        return resolved;
    }
    let original = resolved.take();
    // 换源请求要看得到候选来源的字段，因此传原始 song
    if let Ok(Ok(candidate)) = tokio::time::timeout(timeout, sources.get_cover_url(song)).await
        && usable(&candidate)
    {
        resolved = Some(candidate);
    } else {
        resolved = original;
    }
    resolved
}

/// 播放器事件的订阅入口（runtime 把它接进服务层）。
#[derive(Clone)]
pub struct PlaybackService {
    player: Arc<dyn PlayerBackend>,
    sources: Arc<SourceManager>,
    attempted: Arc<Mutex<HashSet<SourceId>>>,
    js_source_index: Arc<Mutex<Option<usize>>>,
    effects: Arc<dyn PlaybackEffects>,
}

impl PlaybackService {
    pub fn new(
        player: Arc<dyn PlayerBackend>,
        sources: Arc<SourceManager>,
        effects: Arc<dyn PlaybackEffects>,
    ) -> Self {
        Self {
            player,
            sources,
            attempted: Arc::new(Mutex::new(HashSet::new())),
            js_source_index: Arc::new(Mutex::new(None)),
            effects,
        }
    }

    /// 播放器后端句柄（runtime 用它订阅生命周期事件、读取进度）。
    pub fn player(&self) -> Arc<dyn PlayerBackend> {
        Arc::clone(&self.player)
    }

    pub async fn play(
        &self,
        song: &SongInfo,
        quality: Quality,
        auto_toggle: bool,
        restored_state: Option<(Duration, bool)>,
        add_history: bool,
    ) -> Result<(u64, SongInfo), String> {
        let generation = self.player.prepare();
        self.attempted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self
            .js_source_index
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let initial_cover = self.effects.prepare_cover(song, true);
        if add_history {
            self.effects.record_history(song);
        }
        let (mut resolved_song, url) = if let Some(path) = self.effects.existing_download(song) {
            (
                song.clone(),
                SongUrl {
                    url: path.to_string_lossy().into_owned(),
                    quality,
                    duration: song.duration,
                    ..SongUrl::default()
                },
            )
        } else {
            self.resolve(song.clone(), quality, auto_toggle).await?
        };
        if !self
            .player
            .play_with_headers(&url.url, generation, &url.headers)
        {
            return Err("播放器拒绝了本次播放请求".to_string());
        }
        if let Some((position, paused)) = restored_state {
            self.player.seek(position);
            if paused {
                self.player.pause();
            }
        } else if let Some(position) = resolved_song
            .extra
            .get("cue_start_ms")
            .and_then(|v| v.parse::<u64>().ok())
        {
            self.player.seek(Duration::from_millis(position));
        }
        if resolved_song.cover_url.is_none() {
            resolved_song.cover_url = url.cover_url.clone();
        }
        resolved_song.cover_url = resolve_cover_url(
            &self.sources,
            &resolved_song,
            resolved_song.cover_url.clone(),
            |url| self.effects.cover_url_is_usable(url),
            Duration::from_secs(10),
        )
        .await;
        if initial_cover != resolved_song.cover_url {
            let _ = self
                .effects
                .load_cover(resolved_song.cover_url.clone())
                .await;
        }
        let _icon = if self.effects.album_cover_notification() {
            self.effects
                .cache_cover(resolved_song.cover_url.clone())
                .await
        } else {
            None
        };
        if restored_state.is_none_or(|(_, paused)| !paused) {
            let fade_in = self.effects.fade_in_ms();
            if fade_in > 0 {
                self.player.fade_in(Duration::from_millis(fade_in));
            }
        }
        Ok((generation, resolved_song))
    }

    pub async fn expand_bili_parts(
        &self,
        mut songs: Vec<SongInfo>,
        index: usize,
    ) -> (Vec<SongInfo>, usize, Option<String>) {
        let Some(song) = songs.get(index).cloned() else {
            return (songs, index, None);
        };
        if song.source != SourceId::Bili
            || song.extra.contains_key("page")
            || song.extra.contains_key("bili_parts_checked")
        {
            return (songs, index, None);
        }
        match tokio::time::timeout(
            Duration::from_secs(15),
            self.sources.bili_source().video_parts(&song),
        )
        .await
        {
            Ok(Ok(parts)) if !parts.is_empty() => {
                let count = parts.len();
                songs.splice(index..=index, parts);
                (songs, index, Some(format!("已展开 {} 个分 P", count)))
            }
            Ok(Ok(_)) => {
                if let Some(song) = songs.get_mut(index) {
                    song.extra
                        .insert("bili_parts_checked".into(), "true".into());
                }
                (songs, index, None)
            }
            Ok(Err(error)) => {
                if let Some(song) = songs.get_mut(index) {
                    song.extra
                        .insert("bili_parts_checked".into(), "true".into());
                }
                (
                    songs,
                    index,
                    Some(format!("分 P 解析失败，将播放默认分 P: {error}")),
                )
            }
            Err(_) => {
                if let Some(song) = songs.get_mut(index) {
                    song.extra
                        .insert("bili_parts_checked".into(), "true".into());
                }
                (songs, index, Some("分 P 解析超时，将播放默认分 P".into()))
            }
        }
    }

    async fn resolve(
        &self,
        song: SongInfo,
        quality: Quality,
        auto_toggle: bool,
    ) -> Result<(SongInfo, SongUrl), String> {
        let js_index = self
            .js_source_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or(0);
        match self
            .sources
            .get_song_url_from_js_index(&song, quality, js_index)
            .await
        {
            Ok((url, index)) => {
                *self
                    .js_source_index
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = index;
                Ok((song, url))
            }
            Err(direct_error) if auto_toggle => {
                self.attempted
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(song.source);
                for candidate in self.sources.find_music(&song).await {
                    if !self
                        .attempted
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(candidate.source)
                    {
                        continue;
                    }
                    match self
                        .sources
                        .get_song_url_from_js_index(&candidate, quality, 0)
                        .await
                    {
                        Ok((url, index)) => {
                            *self
                                .js_source_index
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = index;
                            return Ok((candidate, url));
                        }
                        Err(_error) => {}
                    }
                }
                Err(format!(
                    "获取播放地址失败，换源后仍不可用: {}",
                    direct_error
                ))
            }
            Err(error) => Err(format!("获取播放地址失败: {}", error)),
        }
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.resume();
    }

    pub fn toggle(&self) {
        self.player.toggle();
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn seek(&self, position: Duration) {
        self.player.seek(position);
    }

    pub fn set_volume(&self, volume: u32) {
        self.player.set_volume(volume.min(100));
    }

    pub fn volume(&self) -> u32 {
        self.player.volume()
    }

    pub fn quality(&self) -> Quality {
        self.effects.quality()
    }

    pub fn auto_toggle(&self) -> bool {
        self.effects.auto_toggle()
    }

    pub fn source_index(&self) -> Option<usize> {
        *self
            .js_source_index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub async fn url_for(&self, song: &SongInfo, quality: Quality) -> Result<SongUrl, String> {
        self.sources
            .get_song_url_from_js_index(song, quality, 0)
            .await
            .map(|(url, _)| url)
            .map_err(|e| e.to_string())
    }
}
