//! 下载：实现与桌面 GUI 共用 `voicefox_runtime::download`，这里只做 re-export，
//! 保持 TUI 内部 `crate::download::*` 的调用点不变。

pub use voicefox_runtime::download::*;
