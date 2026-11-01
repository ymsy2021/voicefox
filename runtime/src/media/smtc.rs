//! Windows SMTC（System Media Transport Controls）服务。
//!
//! 让硬件媒体键、系统媒体浮层和蓝牙耳机按键能控制 voicefox。通过 souvlaki
//! 访问 SMTC，但它要求调用方提供一个真实窗口句柄且持续泵消息 —— 因此这里
//! 在专属线程上创建一个隐藏的顶层窗口，在同一线程里构造 `MediaControls`
//! 并循环：泵窗口消息 + 应用快照更新。
//!
//! 快照 / 命令的数据结构在 [`crate::media_session`]（与 Linux MPRIS 共用）。

use std::time::Duration;

use souvlaki::{MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::warn;

use crate::media_session::{MediaCommand, MediaSnapshot};

/// 消息泵轮询间隔：决定媒体键事件的响应延迟与空转开销的平衡。
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

pub fn start() -> anyhow::Result<(
    crate::media_session::MediaHandle,
    UnboundedReceiver<MediaCommand>,
)> {
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (handle, update_rx) = crate::media_session::channel();

    // SMTC 的按钮事件经由窗口消息派发，窗口与控件必须同线程；线程退出即
    // 控件失效，因此存活到进程结束，失败只记日志由用户在通知里看到。
    std::thread::Builder::new()
        .name("voicefox-smtc".to_string())
        .spawn(move || {
            if let Err(error) = run_worker(command_tx, update_rx) {
                warn!("SMTC worker exited: {error:#}");
            }
        })?;

    Ok((handle, command_rx))
}

fn run_worker(
    command_tx: tokio::sync::mpsc::UnboundedSender<MediaCommand>,
    mut update_rx: UnboundedReceiver<MediaSnapshot>,
) -> anyhow::Result<()> {
    let hwnd = create_hidden_window()?;
    let mut controls = MediaControls::new(PlatformConfig {
        dbus_name: "voicefox",
        display_name: "voicefox",
        hwnd: Some(hwnd),
    })
    .map_err(|error| anyhow::anyhow!("create media controls: {error}"))?;
    controls
        .attach(move |event| {
            if let Some(command) = crate::media_session::translate_event(event) {
                let _ = command_tx.send(command);
            }
        })
        .map_err(|error| anyhow::anyhow!("attach media controls: {error}"))?;

    // 只保留最新快照：消费不及时的时候，中间状态没有回放价值。
    let mut applied: Option<MediaSnapshot> = None;
    loop {
        pump_window_messages();
        let mut latest = None;
        while let Ok(snapshot) = update_rx.try_recv() {
            latest = Some(snapshot);
        }
        if let Some(snapshot) = latest {
            let metadata_changed = applied
                .as_ref()
                .is_none_or(|previous| previous.metadata_changed(&snapshot));
            if metadata_changed {
                apply_metadata(&mut controls, &snapshot);
            }
            if applied.as_ref() != Some(&snapshot) {
                apply_playback(&mut controls, &snapshot);
            }
            applied = Some(snapshot);
        }
        std::thread::sleep(PUMP_INTERVAL);
    }
}

fn apply_metadata(controls: &mut MediaControls, snapshot: &MediaSnapshot) {
    let duration = Duration::from_micros(snapshot.duration_micros.max(0) as u64);
    let metadata = MediaMetadata {
        title: Some(snapshot.title.as_str()),
        artist: Some(snapshot.artist.as_str()),
        album: Some(snapshot.album.as_str()),
        cover_url: Some(snapshot.art_url.as_str()).filter(|url| !url.is_empty()),
        duration: (snapshot.duration_micros > 0).then_some(duration),
    };
    if controls.set_metadata(metadata).is_err() {
        warn!("SMTC set_metadata failed");
    }
}

fn apply_playback(controls: &mut MediaControls, snapshot: &MediaSnapshot) {
    let playback = match snapshot.playback_status {
        "Playing" => MediaPlayback::Playing {
            progress: Some(MediaPosition(position(snapshot))),
        },
        "Paused" => MediaPlayback::Paused {
            progress: Some(MediaPosition(position(snapshot))),
        },
        _ => MediaPlayback::Stopped,
    };
    if controls.set_playback(playback).is_err() {
        warn!("SMTC set_playback failed");
    }
}

fn position(snapshot: &MediaSnapshot) -> Duration {
    Duration::from_micros(snapshot.position_micros.max(0) as u64)
}

/// 创建 SMTC 依附的隐藏顶层窗口。
///
/// 不用 message-only 窗口：SMTC 按钮事件在 message-only 窗口上不可靠。
/// 窗口从不显示（不调 ShowWindow），也不会出现在任务栏。
fn create_hidden_window() -> anyhow::Result<*mut std::ffi::c_void> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassW, WNDCLASSW, WS_OVERLAPPED,
    };

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let class_name = wide("voicefox_smtc");
    let title = wide("voicefox");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        if instance.is_null() {
            anyhow::bail!("GetModuleHandleW failed");
        }
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        // 重复注册失败不影响流程：目标是拿到一个能收消息的窗口。
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            anyhow::bail!("CreateWindowExW failed");
        }
        Ok(hwnd)
    }
}

/// 泵掉窗口消息，让 SMTC 的按钮事件有机会派发到 attach 的回调。
fn pump_window_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
