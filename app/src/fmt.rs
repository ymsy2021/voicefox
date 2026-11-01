//! 时间/字节格式化助手。
//!
//! 实现与 GUI 共用 `voicefox_runtime::media::fmt`，这里只做 re-export，
//! 保持 TUI 内部 `crate::fmt::*` 的调用点不变。

pub use voicefox_runtime::media::fmt::{format_bytes, format_duration};
