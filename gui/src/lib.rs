//! Voicefox 桌面 GUI（egui）。
//!
//! 复用现有 Rust 核心：`voicefox-application` 的 command/state/event 边界、
//! `voicefox-runtime` 的播放器 / 音源 / 歌词 / MPRIS 媒体会话。
//! TUI（`voicefox-app`）保留，两者共用同一份核心，功能不会分叉。

pub mod app;
pub mod controller;
pub mod cover;
pub mod demo;
pub mod service;
pub mod theme;
pub mod tray;
pub mod ui;

pub use app::{install_cjk_font, resolve_dark, run};
pub use controller::VoicefoxGuiController;
pub use service::{
    AppService, GuiBackend, GuiError, GuiRequest, PlaybackState, RepaintHandle, SearchView,
    Snapshot, TrackView,
};

/// 窗口标题。
pub const TITLE: &str = "Voicefox";
