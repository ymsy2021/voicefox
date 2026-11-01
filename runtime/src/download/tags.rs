//! 下载完成后的收尾：写标签、嵌封面、保存歌词。
//!
//! 对应 MusicBot-Go 的 `bot/id3`（`EmbedTags` + 歌词文件写出），
//! 这里复用 voicefox 本地音乐已有的 lofty 写标签实现。

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::file::AudioFile;
use lx_core::model::lyric::LyricData;
use lx_source::local::metadata::{MetadataEdit, write_metadata};

/// 超过该体积的封面在嵌入前先压缩（与 MusicBot-Go 的 2MB 阈值一致）。
const COVER_SHRINK_THRESHOLD: usize = 2 * 1024 * 1024;
/// 压缩后的封面边长；参考实现压到 320，这里取 640 以便本地播放器显示更清晰。
const COVER_MAX_SIDE: u32 = 640;

/// 一次下载要写入的元数据。
#[derive(Debug, Clone, Default)]
pub struct DownloadMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<Vec<u8>>,
    pub lyric: Option<String>,
}

impl DownloadMetadata {
    pub fn new(title: &str, artist: &str, album: &str) -> Self {
        Self {
            title: title.trim().to_string(),
            artist: artist.trim().to_string(),
            album: album.trim().to_string(),
            ..Self::default()
        }
    }
}

/// 写入标题/歌手/专辑/歌词，并尝试嵌入封面。
///
/// 封面格式不被容器接受时（例如 WebP 封面写入 FLAC 失败）会自动降级为
/// 「不带封面」再写一次，保证标签本身仍然落地。
pub fn embed_tags(path: &Path, metadata: &DownloadMetadata) -> Result<(), String> {
    let lyrics = metadata
        .lyric
        .as_ref()
        .map(|lyric| lyric.trim())
        .filter(|lyric| !lyric.is_empty())
        .map(|lyric| lyric.to_string());

    let usable_cover = metadata
        .cover
        .as_ref()
        .filter(|cover| cover_is_usable(cover))
        .cloned();

    let mut edit = MetadataEdit {
        title: Some(metadata.title.clone()).filter(|value| !value.is_empty()),
        artist: Some(metadata.artist.clone()).filter(|value| !value.is_empty()),
        album: Some(metadata.album.clone()).filter(|value| !value.is_empty()),
        lyrics,
        cover: usable_cover.clone(),
    };

    match write_metadata(path, &edit) {
        Ok(()) => Ok(()),
        Err(error) if edit.cover.is_some() => {
            tracing::warn!(
                "embedding cover failed for {}: {error}, retrying without cover",
                path.display()
            );
            edit.cover = None;
            write_metadata(path, &edit)
        }
        Err(error) => Err(error),
    }
}

/// 把歌词（可选带翻译）写成音频同目录下的 `.lrc` 文件。
pub fn write_lyric_file(
    audio_path: &Path,
    lyric: &str,
    translation: Option<&str>,
) -> Result<PathBuf, String> {
    let merged = match translation.map(str::trim).filter(|text| !text.is_empty()) {
        Some(translation) => merge_lyric(lyric, translation),
        None => lyric.trim().to_string(),
    };
    if merged.is_empty() {
        return Err("歌词为空".to_string());
    }
    let lyric_path = audio_path.with_extension("lrc");
    std::fs::write(&lyric_path, merged).map_err(|error| error.to_string())?;
    Ok(lyric_path)
}

/// 按时间戳把翻译歌词合并到主歌词里，生成双语 LRC。
pub fn merge_lyric(lyric: &str, translation: &str) -> String {
    let translations = lx_lyric::parser::lrc::parse(translation);
    let mut merged = String::new();
    for line in lx_lyric::parser::lrc::parse(lyric) {
        merged.push_str(&format_lrc_line(line.timestamp, &line.text));
        if let Some(translated) = translations
            .iter()
            .find(|candidate| candidate.timestamp == line.timestamp)
            .filter(|candidate| !candidate.text.trim().is_empty())
        {
            merged.push_str(&format_lrc_line(line.timestamp, &translated.text));
        }
    }
    if merged.trim().is_empty() {
        // 无法解析时间轴时退化成「原文 + 译文」两段。
        format!("{}\n{}\n", lyric.trim(), translation.trim())
    } else {
        merged
    }
}

/// 从歌词数据里挑出适合落盘与内嵌的**标准 LRC** 文本。
///
/// 优先使用 `lyric` / `raw_lrc` 里的标准 LRC；需要时把逐字歌词（`lxlyric`）
/// 转成 LRC 行。识别不出来就返回 `None`：宁可没有歌词文件，也不要写出
/// 某些音源返回的原始 JSON / 私有格式内容，那会污染 `.lrc` 与音频标签。
pub fn standard_lyric(lyric: &LyricData) -> Option<String> {
    for candidate in [Some(lyric.lyric.as_str()), lyric.raw_lrc.as_deref()]
        .into_iter()
        .flatten()
    {
        if is_lrc(candidate) {
            return Some(candidate.trim().to_string());
        }
    }
    let karaoke = lyric.lxlyric.as_deref()?;
    let lines = lx_lyric::parser::parse_karaoke(karaoke);
    let converted = lines
        .iter()
        .filter_map(|line| {
            let text = line
                .words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>();
            let text = text.trim();
            (!text.is_empty()).then(|| format_lrc_line(line.timestamp, text))
        })
        .collect::<String>();
    (!converted.trim().is_empty()).then_some(converted)
}

/// 判断文本是否是可解析出内容的标准 LRC。
fn is_lrc(text: &str) -> bool {
    !text.trim().is_empty()
        && lx_lyric::parser::lrc::parse(text)
            .iter()
            .any(|line| !line.text.trim().is_empty())
}

fn format_lrc_line(timestamp_ms: u64, text: &str) -> String {
    let minutes = timestamp_ms / 60_000;
    let seconds = (timestamp_ms % 60_000) as f64 / 1000.0;
    format!("[{minutes:02}:{seconds:05.2}]{}\n", text.trim())
}

/// 封面必须是可解码的图片，否则嵌入会失败。
fn cover_is_usable(cover: &[u8]) -> bool {
    if cover.is_empty() {
        return false;
    }
    image::ImageReader::new(Cursor::new(cover))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .is_some_and(|(width, height)| width > 0 && height > 0)
}

/// 封面过大时按比例缩小并转成 JPEG，避免把几十 MB 的原图塞进音频标签。
///
/// 参照 MusicBot-Go `resizeImg`：只在超过阈值时处理，处理失败就退回原图。
pub fn shrink_cover(cover: Vec<u8>) -> Vec<u8> {
    if cover.len() <= COVER_SHRINK_THRESHOLD {
        return cover;
    }
    match shrink_cover_inner(&cover) {
        Ok(shrunk) if !shrunk.is_empty() => shrunk,
        _ => cover,
    }
}

fn shrink_cover_inner(cover: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::ImageReader::new(Cursor::new(cover))
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?;
    let (width, height) = (image.width(), image.height());
    if width <= COVER_MAX_SIDE && height <= COVER_MAX_SIDE {
        return Err("封面尺寸已经足够小".to_string());
    }
    let scale = COVER_MAX_SIDE as f64 / width.max(height) as f64;
    let target = image::imageops::FilterType::Lanczos3;
    let resized = image.resize(
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
        target,
    );
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 85);
    encoder
        .encode_image(&resized)
        .map_err(|error| error.to_string())?;
    Ok(encoded)
}

/// 校验下载得到的文件确实是完整音频。
///
/// 对照 MusicBot-Go 的 `VerifyFullAudio`：时长与目录元数据偏差过大就判为
/// 不完整音频。区别是这里不依赖 ffprobe —— 用 lofty 解析文件头，
/// 解析不了（少数冷门编码）时只告警，不误删用户的文件。
pub fn validate_audio(path: &Path, expected: Duration) -> Result<(), String> {
    let mut head = [0u8; 16];
    let read = std::fs::File::open(path)
        .and_then(|mut file| {
            use std::io::Read;
            file.read(&mut head)
        })
        .map_err(|error| error.to_string())?;
    if crate::download::naming::detect_extension(&head[..read]).is_none() {
        return Err("下载内容不是音频文件".to_string());
    }

    let tagged = match lofty::read_from_path(path) {
        Ok(tagged) => tagged,
        Err(error) => {
            tracing::warn!("audio validation skipped for {}: {error}", path.display());
            return Ok(());
        }
    };
    let actual = tagged.properties().duration();
    if expected.is_zero() || actual.is_zero() {
        return Ok(());
    }

    // 容差与参考实现一致：短的那侧最多 3 秒或 5%，长的那侧再多给 1.5 秒，
    // 因为音源经常按整秒上报时长，且解码尾包会有 padding。
    let tolerance = Duration::from_secs(3).min(expected / 20);
    let longer = Duration::from_millis(1500).max(tolerance);
    if actual + tolerance < expected || actual > expected + longer {
        return Err(format!(
            "音频时长 {:.1}s 与音源信息 {:.1}s 相差过大",
            actual.as_secs_f64(),
            expected.as_secs_f64()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一张 1x1 的合法 PNG。
    fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png);
        image::ImageEncoder::write_image(
            encoder,
            &[244, 67, 54, 255],
            1,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        png
    }

    #[test]
    fn bilingual_lyric_keeps_matching_timestamps() {
        let lyric = "[00:01.00]第一行\n[00:05.50]第二行\n";
        let translation = "[00:01.00]line one\n[00:05.50]line two\n";

        let merged = merge_lyric(lyric, translation);

        assert_eq!(
            merged,
            "[00:01.00]第一行\n[00:01.00]line one\n[00:05.50]第二行\n[00:05.50]line two\n"
        );
    }

    #[test]
    fn unparseable_lyrics_fall_back_to_plain_concat() {
        let merged = merge_lyric("纯文本歌词", "plain translation");
        assert!(merged.contains("纯文本歌词"));
        assert!(merged.contains("plain translation"));
    }

    #[test]
    fn invalid_cover_bytes_are_rejected() {
        assert!(!cover_is_usable(b"not an image"));
        assert!(!cover_is_usable(b""));
    }

    #[test]
    fn standard_lrc_passes_through() {
        let lyric = LyricData {
            lyric: "[00:01.00]第一行\n[00:05.50]第二行\n".to_string(),
            ..LyricData::default()
        };

        let text = standard_lyric(&lyric).unwrap();

        assert!(text.contains("第一行"));
    }

    #[test]
    fn karaoke_lyric_is_converted_to_lrc() {
        let lyric = LyricData {
            lyric: String::new(),
            lxlyric: Some("[00:01.00]<0,500>字<500,500>幕\n".to_string()),
            ..LyricData::default()
        };

        let text = standard_lyric(&lyric).unwrap();

        assert_eq!(text, "[00:01.00]字幕\n");
    }

    #[test]
    fn private_formats_are_not_written_as_lrc() {
        // 某些音源会返回私有 JSON / 原始格式，直接写进 .lrc 会污染文件。
        let lyric = LyricData {
            lyric: r#"{"t":-1,"c":[{"tx":"作词: "},{"tx":"Someone"}]}"#.to_string(),
            ..LyricData::default()
        };

        assert_eq!(standard_lyric(&lyric), None);
    }

    #[test]
    fn non_audio_downloads_are_rejected() {
        let dir = std::env::temp_dir().join(format!("voicefox-validate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let html = dir.join("error.mp3");
        std::fs::write(&html, b"<html><body>404</body></html>").unwrap();

        let error = validate_audio(&html, Duration::from_secs(200)).unwrap_err();

        assert!(error.contains("不是音频"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_with_unknown_container_is_kept_with_a_warning() {
        let dir = std::env::temp_dir().join(format!("voicefox-validate2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 魔数像 FLAC，但 lofty 解析不了：属于「冷门/损坏」边界，
        // 这里选择保留文件而不是误删。
        let path = dir.join("song.flac");
        std::fs::write(&path, b"fLaC\x00\x00\x00\x00junk").unwrap();

        assert!(validate_audio(&path, Duration::from_secs(200)).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_covers_are_left_untouched() {
        let small = tiny_png();
        assert_eq!(shrink_cover(small.clone()), small);
    }

    #[test]
    fn lyric_file_is_written_next_to_audio() {
        let dir = std::env::temp_dir().join(format!("voicefox-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("song.mp3");
        std::fs::write(&audio, b"x").unwrap();

        let path = write_lyric_file(&audio, "[00:01.00]hi\n", Some("[00:01.00]你好\n")).unwrap();

        assert_eq!(path.file_name().unwrap(), "song.lrc");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("你好"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
