# 桌面窗口（GUI）

`voicefox-gui` 是 voicefox 的桌面前端：一个可直接启动的窗口，给不用终端的用户
提供搜索、播放、队列与系统媒体键。它与 TUI 共用同一份核心，播放后端仍是 libmpv。

## 启动

```bash
./scripts/fetch-gui-assets.sh                # 准备中文字体（构建必需，与 libmpv 同类原生依赖）
cargo run --release -p voicefox-gui          # 连真实音源与播放器
cargo run -p voicefox-gui --bin voicefox-gui-demo   # 假后端演示，不需要网络/音频设备
```

字体：egui 自带字体没有 CJK 字形，构建时把 Noto Sans CJK（SIL OFL 1.1）嵌进二进制。
`scripts/fetch-gui-assets.sh` 优先软链系统里已有的一份，没有再下载；与 libmpv 一样，
字体不随仓库分发。缺了这一步 `cargo build -p voicefox-gui` 会直接报错，不会悄悄产出方块界面。

首次启动读取 `~/.config/voicefox/config.toml`（与 TUI 同一份配置）。没有配置文件时
按默认配置运行；配置里的音量、音质、播放模式、主题（`[theme]`）都会被 GUI 采用。

系统依赖：除 `libmpv` 外还需要窗口系统开发库。Debian/Ubuntu：

```bash
sudo apt install libwayland-dev libxkbcommon-dev libx11-dev libxcb1-dev libgl1-mesa-dev
```

## 能力范围（本版交付）

| 能力 | 状态 |
|---|---|
| 可启动窗口 | ✅ 1200×760 起，可缩放；嵌入 Noto Sans CJK，中文不缺字 |
| 搜索 | ✅ 多音源聚合检索、加载更多、右键菜单（播放 / 下一首播放 / 加入队列） |
| 播放 | ✅ 双击结果即播放并铺满整张结果列表，播放/暂停/上一首/下一首、拖动进度、音量 |
| 队列 | ✅ 双击播放、上移/下移排序、移除、清空 |
| 歌词 | ✅ 歌词页与正在播放页的当前行（LRC/KRC/QRC/YRC 由核心解析） |
| 系统媒体键 | ✅ Linux MPRIS（媒体键、Waybar、GNOME/KDE 媒体浮层），进度与元数据同步；`OpenUri` 可直接播放本地文件 |
| 播放模式 | ✅ 列表循环 / 单曲循环 / 随机 / 顺序 / 播完停止，停在结尾后可一键重新播放 |
| 主题 | ✅ 跟随 `[theme]` 配置与主题库具名主题，与 TUI 同一份取色 |
| TUI | ✅ 保留并共用核心；`cargo run` 行为不变 |

尚未包含：收藏/历史/歌单页面、扫码登录与音源切换、下载面板、系统托盘、Windows SMTC 与
macOS Now Playing 的 GUI 接入（TUI 侧仍保留这些平台后端）。这些是后续迭代。

发布资产：`.github/workflows/release.yml` 目前只打包 TUI 二进制（`voicefox`）。是否把
`voicefox-gui` 一起放进各平台压缩包、以及 Linux 的 `libmpv-dev`/窗口库打包取舍，属于发布流程
决策，留待维护者确认后再改。

## 架构

```
                   ┌───────────────────────┐
   TUI (app/) ─────┤ voicefox-application   ├───── GUI (gui/)
                   │  command / state /     │
                   │  event 边界            │
                   └──────────┬────────────┘
                              │
                   ┌──────────▼────────────┐
                   │ voicefox-runtime      │
                   │  播放器 / 音源 / 歌词  │
                   │  media（MPRIS 会话）   │
                   │  ui::theme（取色）     │
                   └───────────────────────┘
```

- **命令只有一条路**：GUI 与 TUI 都通过 `ApplicationService::dispatch`（GUI 侧是
  `ApplicationCommand`）。UI 从不直接调用播放器或音源。
- **事件驱动**：GUI 服务线程订阅 `ApplicationEventStream`、播放器状态与进度 watcher，
  汇总成一份 `Snapshot` 交给 UI；egui 每帧只读快照，不做 IO、不 await。
- **媒体键共用**：MPRIS 服务、快照构建、媒体键 → 业务命令的映射都在
  `voicefox_runtime::media`，TUI 与 GUI 走同一份实现。
- **非阻塞**：业务命令派发到服务线程的 runtime 执行，等待解析播放地址期间界面照常
  响应（暂停、切歌、媒体键都即时生效）。
- **播放生命周期**：`ApplicationService::bind_player_events` 把播放器的被动状态
  （缓冲、暂停/恢复、停止、出错）同步进 state；曲目播完后由前端层按自己的播放模式推进，
  服务层只记录真相。

## 开发与验收

```bash
cargo test -p voicefox-gui          # 单元 + 冒烟 + 剧本测试
cargo clippy --workspace --all-targets -- -D warnings -A clippy::double_must_use
```

### 界面快照

`gui/tests/storyboard.rs` 用真实绘制 + 演示后端走一遍主要状态（空搜索 → 结果 →
播放 → 队列 → 拖动排序 → 暂停/歌词），并断言界面文字。设 `VOICEFOX_GUI_SCREENSHOT=<目录>`
时每一步还会输出 PNG：

```bash
VOICEFOX_GUI_SCREENSHOT=/tmp/shots cargo test -p voicefox-gui --test storyboard storyboard_renders -- --nocapture
```

### 真实应用截图

`voicefox-gui` 在设了 `VOICEFOX_GUI_SCREENSHOT=<目录>` 时会用 egui 的截图接口每 0.5 秒
把窗口写一张 PNG（`latest.png` 为最新）。另有两个调试用环境变量：

- `VOICEFOX_GUI_AUTOSEARCH=<关键词>`：窗口打开后立即搜索；
- `VOICEFOX_GUI_INPUT=<文件>`：逐行读取命令，`search:<词>` / `play:<序号>`（序号按当前
  搜索结果，从 1 开始）/ `toggle` / `next` / `previous` / `quit`，用于没有键盘注入工具的
  环境；
- `VOICEFOX_GUI_LOG=<文件>`：输入线程与派发失败的诊断日志。

这些只用于本地验收，不影响正常使用。

## 已知限制

- Windows SMTC / macOS Now Playing 目前只有 TUI 侧接入；GUI 在这些平台上仍可播放，
  但系统媒体键暂不可用。
- 歌词每行按核心当前行高亮；逐字歌词（YRC 词级高亮）尚未在 GUI 渲染。
- 播放列表里只有一首时没有"下一首"可补，重复播放依赖播放模式。
- 队列排序通过重建队列实现（核心没有原地移动命令），正在播放的曲目会按重建后的索引
  重新对齐。
## 验收记录

- 界面截图：`gui/tests/storyboard.rs`（演示后端，无网络）与真实应用截图（`VOICEFOX_GUI_SCREENSHOT`）。
  本仓库不随提交附带截图文件，复现命令见上文。
- TUI 不变性：同一份二进制启动后首屏的终端输出与改动前 `main` 逐字节相同
  （160×46 终端、空闲状态、无队列）。
- 真实链路：真实音源搜索（约 200 条结果）、播放、MPRIS 报告 `Playing` 与元数据/进度、
  D-Bus `PlayPause` / `Next` / `OpenUri` 在开发机上验证通过。
