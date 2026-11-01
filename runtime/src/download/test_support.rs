//! 仅测试使用的支撑代码：本地 HTTP 服务端与测试素材生成。

/// 测试服务端的 Range 行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    /// 正常支持 Range，返回 206。
    Supported,
    /// 完全不支持，任何请求都返回 200 全量内容。
    Unsupported,
    /// 声称支持 Range（HEAD 返回 Accept-Ranges）但实际忽略 Range 请求。
    Lying,
    /// 只接受不超过给定上限的有界 Range：HEAD 返回 405、不带 Range 的 GET
    /// 返回 403、Range 超过上限也返回 403（模拟 googlevideo）。
    /// 用于验证严格分片路径不会偷偷回退到 plain GET 或 HEAD 探测。
    BoundedOnly(usize),
}

/// 启动一个只在回环地址监听的最小 HTTP 服务端，返回音频与封面两个 URL。
///
/// 受限沙箱禁止监听端口，此时返回 `None`，调用方跳过用例。
pub async fn spawn_test_server(
    audio: Vec<u8>,
    cover: Option<Vec<u8>>,
    mode: RangeMode,
) -> Option<TestServer> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let audio = audio.clone();
            let cover = cover.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};

                let mut buffer = vec![0u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let head_only = request.starts_with("HEAD");
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                let body = if path.starts_with("/cover") {
                    cover.clone().unwrap_or_default()
                } else {
                    audio.clone()
                };
                // `/denied` 用于模拟「主地址失效」：直接 403，验证候选地址回退。
                if path.starts_with("/denied") {
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                    let _ = stream.shutdown().await;
                    return;
                }
                let range = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|line| line.split_once('='))
                    .and_then(|(_, value)| value.trim().split_once('-'))
                    .and_then(|(start, end)| {
                        Some((start.parse::<usize>().ok()?, end.parse::<usize>().ok()?))
                    });
                let response = match range {
                    // 严格分片模式：只放行不超过上限的有界 Range。
                    Some((start, end))
                        if matches!(mode, RangeMode::BoundedOnly(_)) && !body.is_empty() =>
                    {
                        let cap = match mode {
                            RangeMode::BoundedOnly(cap) => cap,
                            _ => unreachable!(),
                        };
                        if end < start || start >= body.len() || end - start + 1 > cap {
                            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                        } else {
                            let end = end.min(body.len() - 1);
                            let slice = &body[start..=end];
                            [
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                    slice.len(),
                                    body.len()
                                )
                                .into_bytes(),
                                slice.to_vec(),
                            ]
                            .concat()
                        }
                    }
                    None if matches!(mode, RangeMode::BoundedOnly(_)) => {
                        let status = if head_only {
                            "405 Method Not Allowed"
                        } else {
                            "403 Forbidden"
                        };
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .into_bytes()
                    }
                    Some((start, end)) if mode == RangeMode::Supported && !body.is_empty() => {
                        let start = start.min(body.len() - 1);
                        let end = end.min(body.len() - 1);
                        let slice = &body[start..=end];
                        [
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                slice.len(),
                                body.len()
                            )
                            .into_bytes(),
                            slice.to_vec(),
                        ]
                        .concat()
                    }
                    _ => {
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            if mode == RangeMode::Unsupported {
                                "none"
                            } else {
                                "bytes"
                            }
                        );
                        if head_only {
                            header.into_bytes()
                        } else {
                            [header.into_bytes(), body].concat()
                        }
                    }
                };
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Some(TestServer {
        audio_url: format!("http://{addr}/audio.bin"),
        cover_url: format!("http://{addr}/cover.png"),
        denied_url: format!("http://{addr}/denied.bin"),
    })
}

pub struct TestServer {
    pub audio_url: String,
    pub cover_url: String,
    /// 固定返回 403 的地址，用于验证候选地址回退。
    pub denied_url: String,
}

/// 生成可预测的伪音频内容，前四个字节是可被嗅探的 FLAC 魔数。
pub fn fake_audio(len: usize) -> Vec<u8> {
    let mut body = b"fLaC".to_vec();
    body.extend((0..len).map(|index| (index % 251) as u8));
    body
}

/// 生成一张 1x1 的合法 PNG，用于封面嵌入路径。
pub fn tiny_png() -> Vec<u8> {
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

/// 为测试创建独立目录，避免并发用例互相干扰。
pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "voicefox-download-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
