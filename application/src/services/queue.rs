use std::sync::{
    Arc, RwLock,
    atomic::{AtomicUsize, Ordering},
};

use lx_core::model::song::SongInfo;

#[derive(Clone)]
pub struct QueueService {
    songs: Arc<RwLock<Vec<SongInfo>>>,
    index: Arc<AtomicUsize>,
}

impl Default for QueueService {
    fn default() -> Self {
        Self::new()
    }
}

impl QueueService {
    pub fn new() -> Self {
        Self {
            songs: Arc::new(RwLock::new(Vec::new())),
            index: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn snapshot(&self) -> (Vec<SongInfo>, usize) {
        let songs = self.songs.read().unwrap_or_else(|e| e.into_inner()).clone();
        let len = songs.len();
        let index = self.index.load(Ordering::Acquire);
        (songs, index.min(len.saturating_sub(1)))
    }

    pub fn replace(&self, songs: Vec<SongInfo>, index: usize) {
        let safe = index.min(songs.len().saturating_sub(1));
        *self.songs.write().unwrap_or_else(|e| e.into_inner()) = songs;
        self.index.store(safe, Ordering::Release);
    }

    pub fn add(&self, song: SongInfo, next: bool) -> usize {
        let mut songs = self.songs.write().unwrap_or_else(|e| e.into_inner());
        if songs.is_empty() {
            songs.push(song);
            self.index.store(0, Ordering::Release);
            return 0;
        }
        let current = self.index.load(Ordering::Acquire);
        let pos = if next {
            current.saturating_add(1).min(songs.len())
        } else {
            songs.len()
        };
        songs.insert(pos, song);
        pos
    }

    pub fn remove(&self, index: usize) -> Option<SongInfo> {
        let mut songs = self.songs.write().unwrap_or_else(|e| e.into_inner());
        if index >= songs.len() {
            return None;
        }
        let removed = songs.remove(index);
        let current = self.index.load(Ordering::Acquire);
        let next = if songs.is_empty() {
            0
        } else if index < current {
            current - 1
        } else {
            current.min(songs.len() - 1)
        };
        self.index.store(next, Ordering::Release);
        Some(removed)
    }

    /// 原地移动一首歌；返回移动后的索引与是否真的移动了。
    ///
    /// 当前曲目的索引随之调整，保证「正在播放的那首」仍然对齐。
    pub fn move_song(&self, from: usize, to: usize) -> Option<usize> {
        {
            let mut songs = self.songs.write().unwrap_or_else(|e| e.into_inner());
            if from >= songs.len() || to >= songs.len() || from == to {
                return None;
            }
            let song = songs.remove(from);
            songs.insert(to, song);
        }
        let current = self.index.load(Ordering::Acquire);
        let next = if current == from {
            to
        } else if from < current && to >= current {
            current - 1
        } else if from > current && to <= current {
            current + 1
        } else {
            current
        };
        self.index.store(next, Ordering::Release);
        Some(next)
    }

    pub fn clear(&self) {
        self.replace(Vec::new(), 0);
    }

    pub fn set_index(&self, index: usize) {
        let len = self.songs.read().unwrap_or_else(|e| e.into_inner()).len();
        self.index
            .store(index.min(len.saturating_sub(1)), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::QueueService;
    use lx_core::model::song::SongInfo;
    use lx_core::model::source::SourceId;

    fn song(id: &str) -> SongInfo {
        SongInfo::new(id.into(), SourceId::Kw, id.into(), "artist".into())
    }

    #[test]
    fn add_next_and_end_keep_current_index_stable() {
        let queue = QueueService::new();
        queue.add(song("a"), false);
        queue.add(song("b"), true);
        queue.add(song("c"), false);
        let (songs, index) = queue.snapshot();
        assert_eq!(
            songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(index, 0);
    }

    #[test]
    fn moving_songs_keeps_current_track_aligned() {
        let queue = QueueService::new();
        queue.replace(vec![song("a"), song("b"), song("c")], 1);
        // 把第 0 首挪到末尾：当前曲目 "b" 前移一位。
        let index = queue.move_song(0, 2).expect("可以移动");
        let (songs, _) = queue.snapshot();
        assert_eq!(
            songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
        assert_eq!(index, 0);
        assert_eq!(songs[index].id, "b");
        // 移动当前曲目本身：索引跟随到新位置。
        let index = queue.move_song(0, 2).expect("可以移动");
        let (songs, _) = queue.snapshot();
        assert_eq!(index, 2);
        assert_eq!(songs[index].id, "b");
        // 越界或原地移动不改变任何东西。
        assert!(queue.move_song(5, 0).is_none());
        assert!(queue.move_song(1, 1).is_none());
    }

    #[test]
    fn remove_before_current_adjusts_index() {
        let queue = QueueService::new();
        queue.replace(vec![song("a"), song("b"), song("c")], 2);
        queue.remove(0);
        let (songs, index) = queue.snapshot();
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[index].id, "c");
        assert_eq!(index, 1);
    }
}
