//! 下载文件名与目录：模板渲染、非法字符清理、音频扩展名推断。
//!
//! 清理规则参考 MusicBot-Go `sanitizeFileName`：替换路径分隔符等非法字符，
//! 并按 UTF-8 字节预算截断超长文件名。

use std::path::{Path, PathBuf};

use lx_core::model::song::SongInfo;
use lx_core::model::source::Quality;

/// 单个文件名分量的最大字节数（保留扩展名的预算）。
const MAX_NAME_BYTES: usize = 180;

/// 常见音频扩展名，用于从 URL / Content-Type 推断落盘格式。
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "aac", "ogg", "opus", "wav", "wma", "ape", "aiff", "aif", "mka",
    "dff", "dsf", "wv",
];

/// 非法文件名字符统一替换成空格，并截断到字节预算内。
pub fn sanitize_filename(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '/' | '\\' | '?' | '*' | ':' | '|' | '<' | '>' | '"' | '\0' => cleaned.push(' '),
            character if character.is_control() => cleaned.push(' '),
            character => cleaned.push(character),
        }
    }

    // 折叠连续空格，避免模板留空产生一堆分隔符。
    let mut collapsed = String::with_capacity(cleaned.len());
    let mut last_was_space = false;
    for character in cleaned.chars() {
        let is_space = character == ' ';
        if is_space && last_was_space {
            continue;
        }
        collapsed.push(character);
        last_was_space = is_space;
    }

    // Windows 不允许文件名以点或空格结尾。
    let trimmed = collapsed.trim_matches(|character| character == ' ' || character == '.');
    if trimmed.is_empty() {
        return "未命名".to_string();
    }

    let extension = Path::new(trimmed)
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .filter(|extension| extension.len() <= MAX_NAME_BYTES / 2);
    match extension {
        Some(extension) => {
            let stem_len = trimmed.len().saturating_sub(extension.len());
            let stem = truncate_bytes(trimmed, stem_len.min(MAX_NAME_BYTES - extension.len()));
            format!("{}{}", stem.trim_end_matches([' ', '.']), extension)
        }
        None => truncate_bytes(trimmed, MAX_NAME_BYTES).to_string(),
    }
}

/// 按模板渲染文件名主体（不含扩展名）。
///
/// 支持 `{name}` `{singer}` `{album}` `{source}` `{quality}`；未知占位符原样保留。
pub fn render_filename(template: &str, song: &SongInfo, quality: Quality) -> String {
    let template = template.trim();
    let template = if template.is_empty() {
        "{singer} - {name}"
    } else {
        template
    };
    let mut rendered = template.to_string();
    for (placeholder, value) in [
        ("{name}", song.name.trim().to_string()),
        ("{singer}", song.singer.trim().to_string()),
        ("{album}", song.album_name.trim().to_string()),
        ("{source}", song.source.as_str().to_string()),
        ("{quality}", quality.label().to_string()),
    ] {
        rendered = rendered.replace(placeholder, &value);
    }
    // 去掉模板里因为字段为空留下的孤立分隔符，例如 "{singer} - {name}" → " - 歌名"。
    let trimmed =
        rendered.trim_matches(|character| matches!(character, '-' | '—' | '_' | '.' | ' '));
    let trimmed = trimmed.replace("  -  ", " - ").trim().to_string();
    if trimmed.is_empty() || trimmed.chars().all(|c| !c.is_alphanumeric()) {
        song.name.trim().to_string()
    } else {
        trimmed
    }
}

/// 从 URL 路径推断音频扩展名；查询参数不参与判断。
pub fn extension_from_url(url: &str) -> Option<String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let candidate = without_query
        .rsplit('/')
        .next()
        .and_then(|segment| segment.rsplit_once('.'))
        .map(|(_, extension)| extension.trim().to_ascii_lowercase())?;
    is_audio_extension(&candidate).then_some(candidate)
}

/// 按文件头魔数判断真实音频格式，避免 CDN 返回的扩展名与内容不符。
pub fn detect_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    let head = &bytes[..bytes.len().min(16)];
    if head.starts_with(b"ID3") || (head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        return Some("mp3");
    }
    if head.starts_with(b"fLaC") {
        return Some("flac");
    }
    if head.starts_with(b"OggS") {
        return Some("ogg");
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return Some("m4a");
    }
    if head.starts_with(b"RIFF") {
        return Some("wav");
    }
    if head.starts_with(b"MAC ") {
        return Some("ape");
    }
    if head.starts_with(b"wvpk") {
        return Some("wv");
    }
    if head.starts_with(b"FORM") {
        return Some("aiff");
    }
    if head.starts_with(b"wma") || head.starts_with(&[0x30, 0x26, 0xB2, 0x75]) {
        return Some("wma");
    }
    None
}

/// 解析下载目录：用户显式配置优先，其次 `~/Music/voicefox`，再退回 `~/Downloads/voicefox`。
pub fn resolve_download_dir(configured: &str) -> PathBuf {
    let configured = configured.trim();
    if !configured.is_empty() {
        if let Some(relative) = configured.strip_prefix("~/")
            && let Some(home) = dirs::home_dir()
        {
            return home.join(relative);
        }
        return PathBuf::from(expand_env(configured));
    }
    let base = dirs::audio_dir()
        .or_else(dirs::download_dir)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("voicefox")
}

fn expand_env(value: &str) -> String {
    if let Some(rest) = value.strip_prefix("$HOME/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().to_string();
    }
    value.to_string()
}

/// 目标文件已存在时追加 ` (2)`、` (3)`，避免覆盖已有下载。
pub fn unique_dest(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let sanitized = sanitize_filename(stem);
    let extension = extension.trim_start_matches('.');
    let mut candidate = dir.join(format!("{sanitized}.{extension}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{sanitized} ({index}).{extension}"));
        index += 1;
        if index > 999 {
            break;
        }
    }
    candidate
}

/// 归一化扩展名：容器为 mp4 但内容是音频时统一写成 m4a。
pub fn normalize_extension(extension: &str) -> String {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match extension.as_str() {
        "mp4" => "m4a".to_string(),
        "" => "mp3".to_string(),
        other => other.to_string(),
    }
}

fn is_audio_extension(extension: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&extension)
}

/// 按 UTF-8 字符边界截断字符串，避免切出半个汉字。
fn truncate_bytes(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn song(name: &str, singer: &str, album: &str) -> SongInfo {
        let mut song = SongInfo::new(
            "1".to_string(),
            lx_core::model::source::SourceId::Kw,
            name.to_string(),
            singer.to_string(),
        );
        song.album_name = album.to_string();
        song.qualities = BTreeSet::new();
        song
    }

    #[test]
    fn sanitize_replaces_illegal_characters() {
        assert_eq!(
            sanitize_filename("AC/DC: Back?in*Black"),
            "AC DC Back in Black"
        );
        assert_eq!(sanitize_filename("  ..  "), "未命名");
        assert_eq!(sanitize_filename("a   b"), "a b");
    }

    #[test]
    fn sanitize_truncates_long_names_on_char_boundaries() {
        let long = "汉".repeat(200);
        let cleaned = sanitize_filename(&long);
        assert!(cleaned.len() <= MAX_NAME_BYTES);
        assert!(cleaned.chars().all(|character| character == '汉'));
    }

    #[test]
    fn template_renders_song_fields() {
        let song = song("晴天", "周杰伦", "叶惠美");
        assert_eq!(
            render_filename("{singer} - {name}", &song, Quality::High320),
            "周杰伦 - 晴天"
        );
        assert_eq!(
            render_filename("{album} - {name} ({quality})", &song, Quality::Flac),
            "叶惠美 - 晴天 (FLAC)"
        );
    }

    #[test]
    fn template_falls_back_when_artist_is_missing() {
        let song = song("晴天", "", "");
        assert_eq!(
            render_filename("{singer} - {name}", &song, Quality::Flac),
            "晴天"
        );
        assert_eq!(render_filename("   ", &song, Quality::Flac), "晴天");
    }

    #[test]
    fn extension_is_read_from_url_without_query() {
        assert_eq!(
            extension_from_url("https://cdn.example.com/a/b/song.flac?auth=1"),
            Some("flac".to_string())
        );
        assert_eq!(extension_from_url("https://cdn.example.com/stream"), None);
    }

    #[test]
    fn magic_bytes_win_over_misleading_extension() {
        assert_eq!(detect_extension(b"fLaC\x00\x00"), Some("flac"));
        assert_eq!(detect_extension(b"ID3\x04\x00"), Some("mp3"));
        assert_eq!(detect_extension(b"OggS\x00\x02"), Some("ogg"));
        assert_eq!(detect_extension(b"\x00\x00\x00\x18ftypM4A "), Some("m4a"));
        assert_eq!(detect_extension(b"RIFF\x24\x08\x00\x00WAVE"), Some("wav"));
        assert_eq!(detect_extension(b"<html>"), None);
    }

    #[test]
    fn unique_dest_appends_suffix_instead_of_overwriting() {
        let dir = std::env::temp_dir().join(format!("voicefox-naming-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_dest(&dir, "song", "flac");
        assert_eq!(first.file_name().unwrap(), "song.flac");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_dest(&dir, "song", "flac");
        assert_eq!(second.file_name().unwrap(), "song (2).flac");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_download_dir_expands_home_and_defaults() {
        let dir = resolve_download_dir("~/music-test");
        assert!(dir.ends_with("music-test"));
        assert!(dir.is_absolute());

        let default_dir = resolve_download_dir("");
        assert!(default_dir.ends_with("voicefox"));
    }
}
