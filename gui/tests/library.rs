//! 收藏 / 历史 / 歌单：GUI 与 TUI 读写同一份存储。
//!
//! 这里直接驱动服务层的库操作（不经 egui），并用隔离的 `XDG_DATA_HOME`
//! 构造一份 `Storage`，验证「GUI 侧增删改查 = 核心存储的增删改查」。

use std::sync::Arc;

use lx_core::model::song::SongInfo;
use lx_core::model::source::SourceId;
use voicefox_runtime::storage::Storage;

fn song(id: &str, name: &str) -> SongInfo {
    let mut song = SongInfo::new(id.into(), SourceId::Kw, name.into(), "周杰伦".into());
    song.album_name = "十一月的萧邦".into();
    song
}

/// 用临时目录构造一份独立存储（不碰真实数据目录）。
fn isolated_storage(tag: &str) -> (Storage, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("voicefox-gui-lib-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 只在 egui 进程外（本测试）改环境变量：并行测试各自用独立目录。
    unsafe { std::env::set_var("XDG_DATA_HOME", &dir) };
    (Storage::new(), dir)
}

#[test]
fn gui_library_operations_write_the_shared_store() {
    let (storage, dir) = isolated_storage("favorites");
    let storage = Arc::new(storage);

    // 收藏：加 → 在 → 删。
    let night = song("118980", "夜曲");
    assert!(storage.add_favorite(&night));
    assert!(storage.is_favorite(&night));
    assert_eq!(storage.load_favorites().len(), 1);
    assert!(storage.remove_favorite(&night));
    assert!(storage.load_favorites().is_empty());

    // 历史：记录 + 移除。
    storage.add_history(&night, 50);
    assert_eq!(storage.load_history().len(), 1);
    assert!(storage.remove_history(&night));
    assert!(storage.load_history().is_empty());

    // 自建歌单：建 → 加入 → 读出 → 删除歌曲 → 删歌单。
    let playlist = storage
        .create_custom_playlist("通勤歌单")
        .expect("新建歌单");
    assert!(
        storage
            .add_song_to_custom_playlist(&playlist.id, &night)
            .unwrap()
    );
    let loaded = storage.custom_playlist(&playlist.id).expect("歌单存在");
    assert_eq!(loaded.songs.len(), 1);
    assert_eq!(loaded.songs[0].name, "夜曲");
    assert!(
        storage
            .remove_song_from_custom_playlist(&playlist.id, &night)
            .unwrap()
    );
    assert!(
        storage
            .custom_playlist_summaries()
            .iter()
            .any(|item| item.name == "通勤歌单")
    );
    storage
        .delete_custom_playlist(&playlist.id)
        .expect("删除歌单");
    assert!(storage.custom_playlist(&playlist.id).is_none());

    let _ = std::fs::remove_dir_all(dir);
}
