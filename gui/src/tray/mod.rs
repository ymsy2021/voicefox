//! 系统托盘与「最小化到托盘」。
//!
//! 托盘线程自己发请求给服务线程（不经过 UI 线程做 IO）：
//!
//! | 平台 | 实现 | 状态 |
//! |---|---|---|
//! | Linux / BSD | `ksni`（纯 Rust StatusNotifierItem，走 D-Bus，与 MPRIS 同一条路线） | 已实现 |
//! | Windows / macOS | 暂无（`tray-icon` 依赖 `libxdo`/GTK 或 Objective-C 绑定，本轮未接） | 编译通过，托盘与「最小化到托盘」不可用，关闭窗口即退出 |
//!
//! 关闭窗口不退出：主线程收到关闭事件后先隐藏窗口（`Visible(false)`），
//! 只有托盘「退出」或 MPRIS `Quit` 才真正结束进程。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::service::GuiRequest;

/// 托盘 → 服务线程的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    TogglePlayback,
    Previous,
    Next,
    /// 显示 / 隐藏窗口（由 UI 线程执行的窗口命令）。
    ToggleWindow,
    /// 打开设置页。
    OpenSettings,
    Quit,
}

/// 托盘与 UI 线程之间的窗口命令槽：托盘线程只置位，UI 帧循环消费。
#[derive(Clone, Default)]
pub struct WindowCommands {
    visible: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    show_settings: Arc<AtomicBool>,
}

impl WindowCommands {
    pub fn new(show_settings: Arc<AtomicBool>) -> Self {
        Self {
            show_settings,
            ..Default::default()
        }
    }

    pub fn toggle_visible(&self) {
        self.visible.fetch_xor(true, Ordering::AcqRel);
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::Release);
    }

    /// 取出并清除「切换显示」请求。
    pub fn take_toggle(&self) -> bool {
        self.visible.swap(false, Ordering::AcqRel)
    }

    pub fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::Acquire)
    }

    /// 取出并清除「打开设置」请求。
    pub fn take_show_settings(&self) -> bool {
        self.show_settings.swap(false, Ordering::AcqRel)
    }
}

/// 托盘动作 → 播放请求（与播放栏按钮同一条 `ApplicationCommand`）。
pub fn request_for(action: TrayAction) -> Option<GuiRequest> {
    use voicefox_application::ApplicationCommand;
    Some(match action {
        TrayAction::TogglePlayback => GuiRequest::Dispatch(ApplicationCommand::Toggle),
        TrayAction::Previous => GuiRequest::Dispatch(ApplicationCommand::Previous),
        TrayAction::Next => GuiRequest::Dispatch(ApplicationCommand::Next),
        TrayAction::ToggleWindow | TrayAction::OpenSettings | TrayAction::Quit => return None,
    })
}

/// 托盘是否可用（当前平台是否真的注册了托盘图标）。
pub const SUPPORTED: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

/// 启动托盘：返回 `true` 表示已注册。
///
/// 失败（没有 D-Bus、桌面没有 StatusNotifierWatcher）只记日志，不影响窗口。
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn start(
    send: impl Fn(GuiRequest) -> bool + Send + Sync + 'static,
    commands: WindowCommands,
) -> bool {
    linux::start(send, commands)
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn start(
    _send: impl Fn(GuiRequest) -> bool + Send + Sync + 'static,
    _commands: WindowCommands,
) -> bool {
    // Windows / macOS 的托盘本轮未接（见模块文档的平台表）。
    false
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use super::{TrayAction, WindowCommands, request_for};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    struct VoicefoxTray {
        send: Arc<dyn Fn(crate::service::GuiRequest) -> bool + Send + Sync>,
        commands: WindowCommands,
        title: String,
        playing: Arc<std::sync::atomic::AtomicBool>,
    }

    impl VoicefoxTray {
        fn dispatch(&self, action: TrayAction) {
            match action {
                TrayAction::ToggleWindow => self.commands.toggle_visible(),
                TrayAction::OpenSettings => {
                    self.commands.show_settings.store(true, Ordering::Release)
                }
                TrayAction::Quit => self.commands.request_quit(),
                other => {
                    if let Some(request) = request_for(other) {
                        let _ = (self.send)(request);
                    }
                }
            }
        }
    }

    impl ksni::Tray for VoicefoxTray {
        fn id(&self) -> String {
            "voicefox".into()
        }

        fn title(&self) -> String {
            self.title.clone()
        }

        fn icon_name(&self) -> String {
            // 主题图标缺失时大部分桌面会退回通用图标，不影响点击。
            "audio-x-generic".into()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: self.title.clone(),
                description: "Voicefox 桌面播放器".into(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.dispatch(TrayAction::ToggleWindow);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            let playing = self.playing.load(Ordering::Acquire);
            vec![
                StandardItem {
                    label: "显示 / 隐藏".into(),
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::ToggleWindow)),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: if playing {
                        "暂停".to_string()
                    } else {
                        "播放".to_string()
                    },
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::TogglePlayback)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "上一首".into(),
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::Previous)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "下一首".into(),
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::Next)),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "设置…".into(),
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::OpenSettings)),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "退出".into(),
                    activate: Box::new(|tray: &mut Self| tray.dispatch(TrayAction::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub fn start(
        send: impl Fn(crate::service::GuiRequest) -> bool + Send + Sync + 'static,
        commands: WindowCommands,
    ) -> bool {
        let playing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tray = VoicefoxTray {
            send: Arc::new(send),
            commands,
            title: crate::TITLE.to_string(),
            playing,
        };
        // 托盘必须跑在**自己的线程**上：ksni 的阻塞版内部会 `block_on`，
        // 在服务线程的 tokio runtime 里直接调用会 panic
        // （"Cannot start a runtime from within a runtime"）。
        // 这里起一个专用线程，注册结果通过 channel 回传。
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let spawned = std::thread::Builder::new()
            .name("voicefox-tray".into())
            .spawn(move || match ksni::blocking::TrayMethods::spawn(tray) {
                Ok(_handle) => {
                    let _ = ready_tx.send(Ok(()));
                    // 托盘服务在后台线程里跑；本线程保持存活直到进程退出。
                    loop {
                        std::thread::park();
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            });
        if let Err(error) = spawned {
            crate::service::debug_log(&format!("tray thread failed: {error}"));
            return false;
        }
        match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                crate::service::debug_log(&format!("tray unavailable: {error}"));
                false
            }
            Err(error) => {
                crate::service::debug_log(&format!("tray did not start: {error}"));
                false
            }
        }
    }
}
