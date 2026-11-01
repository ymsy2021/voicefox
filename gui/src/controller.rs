//! GUI 控制器：服务句柄 + UI 侧临时状态（搜索框内容、状态栏文案）。
//!
//! 每帧由 [`crate::app`] 先 `apply_snapshot` 再交给绘制层；控制器自身
//! 不做 IO，也不阻塞。

use std::sync::Arc;

use eframe::egui;

use crate::service::{GuiBackend, GuiRequest, RepaintHandle, Snapshot};

/// 解析 [`VoicefoxGuiController::listen_input`] 的一行命令。
pub fn parse_input(
    line: &str,
    search_items: impl FnOnce() -> Vec<crate::service::TrackView>,
) -> Option<GuiRequest> {
    use voicefox_application::ApplicationCommand;

    let line = line.trim();
    let (verb, argument) = line.split_once(':').map_or((line, ""), |(verb, argument)| {
        (verb.trim(), argument.trim())
    });
    match verb {
        "quit" => Some(GuiRequest::Quit),
        "search" if !argument.is_empty() => {
            Some(GuiRequest::Dispatch(ApplicationCommand::Search {
                keyword: argument.to_string(),
                source: None,
            }))
        }
        // 序号从 1 开始；列表取自当前搜索结果（由调用方在外部补全）。
        "play" => argument
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .map(|index| GuiRequest::PlayList {
                songs: search_items(),
                index,
            }),
        "toggle" => Some(GuiRequest::Dispatch(ApplicationCommand::Toggle)),
        "next" => Some(GuiRequest::Dispatch(ApplicationCommand::Next)),
        "previous" => Some(GuiRequest::Dispatch(ApplicationCommand::Previous)),
        _ => None,
    }
}

pub struct VoicefoxGuiController {
    backend: Box<dyn GuiBackend>,
    snapshot: Snapshot,
    search_input: String,
    status: Option<String>,
    /// 上一条已经展示过的通知（避免同一通知每秒重刷状态栏）。
    seen_notification: Option<String>,
}

impl VoicefoxGuiController {
    /// 绑定真实服务线程（生产路径）。
    pub fn new(ctx: &egui::Context, service: crate::service::AppService) -> Self {
        Self::with_backend(ctx, Box::new(service))
    }

    /// 绑定任意后端实现（演示、UI 快照测试）。
    pub fn with_backend(ctx: &egui::Context, backend: Box<dyn GuiBackend>) -> Self {
        // 服务线程在快照变化时请求重绘：UI 不必空转轮询。
        let target = ctx.clone();
        let repaint: RepaintHandle = backend.repaint_handle();
        repaint.set(Arc::new(move || target.request_repaint()));
        let initial = backend.take_snapshot();
        let search_input = initial.search.keyword.clone();
        Self {
            backend,
            snapshot: initial,
            search_input,
            status: None,
            seen_notification: None,
        }
    }

    pub fn backend(&self) -> &dyn GuiBackend {
        self.backend.as_ref()
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// 用服务线程发布的最新快照刷新界面状态。
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        if snapshot.notification.is_some() && snapshot.notification != self.seen_notification {
            self.seen_notification = snapshot.notification.clone();
            self.status = snapshot.notification.clone();
        }
        self.search_input = snapshot.search.keyword.clone();
        self.snapshot = snapshot;
    }

    pub fn search_input_mut(&mut self) -> &mut String {
        &mut self.search_input
    }

    /// 监听输入命令的线程：每行一条命令立即提交，便于外部驱动界面
    /// （`VOICEFOX_GUI_INPUT`）。支持 `search:<关键词>`、`play:<序号>`
    /// （序号从 1 开始，按当前搜索结果）与 `quit`。
    pub fn listen_input(&mut self, stream: std::fs::File) {
        log_line("监听输入命令文件");
        let handle = self.backend.repaint_handle();
        let backend = self.backend.clone_backend();
        std::thread::Builder::new()
            .name("voicefox-gui-input".into())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(stream);
                let mut line = String::new();
                loop {
                    line.clear();
                    match std::io::BufRead::read_line(&mut reader, &mut line) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let backend_for_items = backend.clone_backend();
                    let Some(request) =
                        parse_input(&line, || backend_for_items.take_snapshot().search.items)
                    else {
                        log_line(&format!("忽略未知输入命令: {:?}", line.trim_end()));
                        continue;
                    };
                    log_line(&format!("输入命令: {} → {request:?}", line.trim_end()));
                    let _ = backend.send(request);
                    handle.call();
                }
            })
            .ok();
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// 提交一条请求；失败时把错误留在状态栏。
    pub fn send(&mut self, request: GuiRequest) {
        if let Err(error) = self.backend.send(request) {
            self.set_status(error.to_string());
        }
    }
}
/// 输入线程的诊断日志。GUI 常常从桌面会话启动（标准错误不可见），
/// 因此把命令回显写到 `VOICEFOX_GUI_LOG` 指定的文件里。
fn log_line(message: &str) {
    let Ok(path) = std::env::var("VOICEFOX_GUI_LOG") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{message}");
    }
}
