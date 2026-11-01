# Voicefox Flutter frontend

Flutter is intentionally not bootstrapped against TUI internals. The Rust ABI is exposed through `voicefox-ffi` and `flutter_rust_bridge`.

Phase 3 starts with the stable command/state/event boundary. Native playback bootstrap remains outside the Flutter ABI so desktop can keep libmpv while Android/iOS select their native backends.

## Generation

Use `flutter_rust_bridge_codegen generate` from the repository root after installing the matching 2.x codegen tool. Generated Dart bindings belong under `flutter/lib/src/bridge/` and are not handwritten.

## 桌面 GUI 走的是 egui，不是这条 Flutter 线

`gui/`（`voicefox-gui`，egui/eframe）是当前可交付的桌面窗口：直接用 Rust 接
`voicefox-application` 与 `voicefox-runtime`，不需要额外一套 ABI 桥或 Flutter 工具链。
它和 TUI 共用同一份 `ApplicationService` 命令、`voicefox-runtime` 的媒体会话
（MPRIS）、主题取色与配置，因此不会出现「两套前端逻辑分叉」。

Flutter 线（本目录）保持原状作为 Phase 3 的 ABI 研究线：这里约定的
command / state / event 边界没有改动，`ffi/` 生成的绑定也没有变化。详见
[`../docs/GUI.md`](../docs/GUI.md)。
