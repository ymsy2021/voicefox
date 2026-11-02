//! 自定义键位映射系统（Phase 1）
//!
//! 设计目标：
//! - 单键映射，零学习成本
//! - 全局 + 页面级两层作用域
//! - 完全兼容现有硬编码行为（默认配置 = 当前键位）
//! - 可扩展至多键序列（Phase 2）

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

// =============================================================================
// Action 枚举：所有可绑定的动作
// =============================================================================

/// 可绑定的用户动作。
///
/// 命名约定：`<domain>_<verb>`，如 `global_quit`、`search_select_down`。
/// 这样即使不同页面有同名动作（如上下导航），也可以通过域名区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // --- 全局动作 ---
    /// 退出应用
    GlobalQuit,
    /// 播放/暂停
    GlobalPlayPause,
    /// 下一首
    GlobalNextTrack,
    /// 上一首
    GlobalPrevTrack,
    /// 切换播放模式（列表循环 / 单曲循环 / 随机）
    GlobalCycleMode,
    /// 快进 5 秒
    GlobalSeekForward,
    /// 后退 5 秒
    GlobalSeekBackward,
    /// 音量增加
    GlobalVolumeUp,
    /// 音量减少
    GlobalVolumeDown,
    /// 下一个标签页
    GlobalNextTab,
    /// 上一个标签页
    GlobalPrevTab,
    /// 返回主页面（Esc）
    GlobalGoToMain,
    /// 收藏/取消收藏当前歌曲（Ctrl+L）
    GlobalToggleFavorite,
    /// 下载当前播放中的歌曲（Ctrl+S）
    GlobalDownloadCurrent,
    /// 打开下载面板（Ctrl+O）
    GlobalDownloadsPanel,
    /// 强制重绘，并把封面重新传输给终端（Ctrl+R）
    GlobalRedraw,
    /// 把当前页面的面板布局恢复成默认比例（Ctrl+G）
    GlobalResetLayout,
    /// 打开睡眠定时器菜单（到点自动暂停）
    GlobalSleepTimer,
    /// 开关频谱可视化（覆盖内容区的柱状频谱）
    GlobalVisualizer,

    // --- 通用列表动作（多个页面共用） ---
    /// 选择上一项
    ListSelectUp,
    /// 选择下一项
    ListSelectDown,
    /// 跳到第一项
    ListSelectFirst,
    /// 跳到最后一项
    ListSelectLast,
    /// 向上翻页
    ListPageUp,
    /// 向下翻页
    ListPageDown,
    /// 激活选中项（播放 / 进入）
    ListActivate,
    /// 收藏或取消收藏当前页面选中的歌曲/歌单
    ListToggleFavorite,
    /// 返回/退出（Esc）
    ListGoBack,
    /// 添加到队列尾部
    ListAddToQueue,
    /// 添加到队列下一首
    ListAddToQueueNext,
    /// 下载当前选中的歌曲
    ListDownload,
    /// 循环切换当前列表的排序方式
    ListCycleSort,
    /// 打开当前选中项的上下文菜单（无鼠标环境下的右键替代入口）
    ListContextMenu,

    // --- 搜索页面专用 ---
    /// 进入搜索输入模式
    SearchInputMode,
    /// 开始搜索 / 播放结果（Enter 的复合语义）
    SearchStart,
    /// 切换聚合/单音源搜索
    SearchToggleAggregate,
    /// 切换上一个音源
    SearchCycleSourcePrev,
    /// 切换下一个音源
    SearchCycleSourceNext,

    // --- 本地音乐页面专用 ---
    /// 重新扫描本地音乐目录
    LocalRescan,
    /// 删除选中的本地文件（弹出确认）
    LocalDelete,
    /// 进入本地音乐过滤模式
    LocalFilter,

    // --- 历史页面专用 ---
    /// 进入历史页面过滤模式
    HistoryFilter,

    // --- 收藏页面专用 ---
    /// 进入过滤模式
    FavoritesFilter,
    /// 取消收藏
    FavoritesRemove,

    // --- 设置页面专用 ---
    /// 切换设置项的值
    SettingsToggle,
    /// 循环切换播放速度
    SettingsCyclePlaybackSpeed,
    /// 编辑音频输出设备
    SettingsEditAudioDevice,
    /// 循环切换 ReplayGain 模式
    SettingsCycleReplayGainMode,
    /// 循环切换 ReplayGain 预放大
    SettingsCycleReplayGainPreamp,
    /// 循环切换声道模式
    SettingsCycleChannelMode,
    /// 循环调整左右平衡
    SettingsCycleBalance,
    /// 切换 ReplayGain 削波保护
    SettingsToggleReplayGainClip,
    /// 循环切换自动淡入时长
    SettingsCycleFadeInDuration,
    /// 循环切换自动淡出时长
    SettingsCycleFadeOutDuration,
    /// 循环切换均衡器预设
    SettingsCycleEqualizerPreset,
    /// 对当前歌曲执行淡入
    SettingsRunFadeIn,
    /// 对当前歌曲执行淡出
    SettingsRunFadeOut,
    /// 把当前位置设为 A-B 循环起点
    SettingsSetAbLoopStart,
    /// 把当前位置设为 A-B 循环终点
    SettingsSetAbLoopEnd,
    /// 清除 A-B 循环
    SettingsClearAbLoop,
    /// 导出用户数据
    SettingsExportData,
    /// 导入用户数据
    SettingsImportData,
    /// 导入外部歌单
    SettingsImportPlaylist,
}

// =============================================================================
// KeybindingConfig：序列化配置结构
// =============================================================================

impl Action {
    /// 动作的中文说明，用于帮助浮层与文档生成。
    pub fn label(self) -> &'static str {
        match self {
            Action::GlobalQuit => "退出应用",
            Action::GlobalPlayPause => "播放 / 暂停",
            Action::GlobalNextTrack => "下一首",
            Action::GlobalPrevTrack => "上一首",
            Action::GlobalCycleMode => "切换播放模式",
            Action::GlobalSeekForward => "快进 5 秒",
            Action::GlobalSeekBackward => "后退 5 秒",
            Action::GlobalVolumeUp => "音量增加",
            Action::GlobalVolumeDown => "音量减少",
            Action::GlobalNextTab => "下一个标签页",
            Action::GlobalPrevTab => "上一个标签页",
            Action::GlobalGoToMain => "返回主页面",
            Action::GlobalToggleFavorite => "收藏 / 取消收藏当前歌曲",
            Action::GlobalDownloadCurrent => "下载当前播放歌曲",
            Action::GlobalDownloadsPanel => "打开 / 关闭下载面板",
            Action::GlobalRedraw => "强制重绘界面",
            Action::GlobalResetLayout => "恢复当前页面默认面板布局",
            Action::GlobalSleepTimer => "睡眠定时器（到点自动暂停）",
            Action::GlobalVisualizer => "频谱可视化开关",
            Action::ListSelectUp => "选择上一项",
            Action::ListSelectDown => "选择下一项",
            Action::ListSelectFirst => "跳到第一项",
            Action::ListSelectLast => "跳到最后一项",
            Action::ListPageUp => "向上翻页",
            Action::ListPageDown => "向下翻页",
            Action::ListActivate => "激活选中项（播放 / 进入）",
            Action::ListToggleFavorite => "收藏 / 取消收藏选中歌曲",
            Action::ListGoBack => "返回 / 退出",
            Action::ListContextMenu => "打开选中项菜单",
            Action::ListAddToQueue => "添加到队列尾部",
            Action::ListAddToQueueNext => "添加到队列下一首",
            Action::ListDownload => "下载选中歌曲",
            Action::ListCycleSort => "循环切换列表排序",
            Action::SearchInputMode => "进入搜索输入模式",
            Action::SearchStart => "开始搜索 / 播放结果",
            Action::SearchToggleAggregate => "切换聚合 / 单音源搜索",
            Action::SearchCycleSourcePrev => "切换上一个音源",
            Action::SearchCycleSourceNext => "切换下一个音源",
            Action::LocalRescan => "重新扫描本地音乐目录",
            Action::LocalDelete => "删除选中的本地文件",
            Action::LocalFilter => "进入本地音乐过滤模式",
            Action::HistoryFilter => "进入历史页面过滤模式",
            Action::FavoritesFilter => "进入收藏过滤模式",
            Action::FavoritesRemove => "取消收藏",
            Action::SettingsToggle => "切换设置项的值",
            Action::SettingsCyclePlaybackSpeed => "循环切换播放速度",
            Action::SettingsEditAudioDevice => "编辑音频输出设备",
            Action::SettingsCycleReplayGainMode => "循环切换 ReplayGain 模式",
            Action::SettingsCycleReplayGainPreamp => "循环切换 ReplayGain 预放大",
            Action::SettingsCycleChannelMode => "循环切换声道模式",
            Action::SettingsCycleBalance => "循环调整左右平衡",
            Action::SettingsToggleReplayGainClip => "切换 ReplayGain 削波保护",
            Action::SettingsCycleFadeInDuration => "循环切换自动淡入时长",
            Action::SettingsCycleFadeOutDuration => "循环切换自动淡出时长",
            Action::SettingsCycleEqualizerPreset => "循环切换均衡器预设",
            Action::SettingsRunFadeIn => "对当前歌曲执行淡入",
            Action::SettingsRunFadeOut => "对当前歌曲执行淡出",
            Action::SettingsSetAbLoopStart => "设置 A-B 循环起点",
            Action::SettingsSetAbLoopEnd => "设置 A-B 循环终点",
            Action::SettingsClearAbLoop => "清除 A-B 循环",
            Action::SettingsExportData => "导出用户数据",
            Action::SettingsImportData => "导入用户数据",
            Action::SettingsImportPlaylist => "导入外部歌单",
        }
    }

    /// 动作的规范展示顺序，帮助浮层按此排序。
    pub const ALL: [Action; 63] = [
        Action::GlobalQuit,
        Action::GlobalPlayPause,
        Action::GlobalNextTrack,
        Action::GlobalPrevTrack,
        Action::GlobalCycleMode,
        Action::GlobalSeekForward,
        Action::GlobalSeekBackward,
        Action::GlobalVolumeUp,
        Action::GlobalVolumeDown,
        Action::GlobalNextTab,
        Action::GlobalPrevTab,
        Action::GlobalGoToMain,
        Action::GlobalToggleFavorite,
        Action::GlobalDownloadCurrent,
        Action::GlobalDownloadsPanel,
        Action::GlobalRedraw,
        Action::GlobalResetLayout,
        Action::GlobalSleepTimer,
        Action::GlobalVisualizer,
        Action::ListSelectUp,
        Action::ListSelectDown,
        Action::ListSelectFirst,
        Action::ListSelectLast,
        Action::ListPageUp,
        Action::ListPageDown,
        Action::ListActivate,
        Action::ListToggleFavorite,
        Action::ListGoBack,
        Action::ListAddToQueue,
        Action::ListAddToQueueNext,
        Action::ListDownload,
        Action::ListCycleSort,
        Action::ListContextMenu,
        Action::SearchInputMode,
        Action::SearchStart,
        Action::SearchToggleAggregate,
        Action::SearchCycleSourcePrev,
        Action::SearchCycleSourceNext,
        Action::LocalRescan,
        Action::LocalDelete,
        Action::LocalFilter,
        Action::HistoryFilter,
        Action::FavoritesFilter,
        Action::FavoritesRemove,
        Action::SettingsToggle,
        Action::SettingsCyclePlaybackSpeed,
        Action::SettingsEditAudioDevice,
        Action::SettingsCycleReplayGainMode,
        Action::SettingsCycleReplayGainPreamp,
        Action::SettingsCycleChannelMode,
        Action::SettingsCycleBalance,
        Action::SettingsToggleReplayGainClip,
        Action::SettingsCycleFadeInDuration,
        Action::SettingsCycleFadeOutDuration,
        Action::SettingsCycleEqualizerPreset,
        Action::SettingsRunFadeIn,
        Action::SettingsRunFadeOut,
        Action::SettingsSetAbLoopStart,
        Action::SettingsSetAbLoopEnd,
        Action::SettingsClearAbLoop,
        Action::SettingsExportData,
        Action::SettingsImportData,
        Action::SettingsImportPlaylist,
    ];
}

/// 页面标识在帮助浮层中的显示名与顺序。
pub const PAGE_ORDER: [(&str, &str); 10] = [
    ("main", "队列（主页）"),
    ("search", "搜索"),
    ("leaderboard", "排行榜"),
    ("playlists", "热门歌单"),
    ("favorites", "收藏"),
    ("history", "播放历史"),
    ("local", "本地音乐"),
    ("settings", "设置"),
    ("details", "歌手/专辑详情"),
    ("bili_login", "B 站登录"),
];

/// 键位配置根结构。
///
/// TOML 示例：
/// ```toml
/// [keybindings.global]
/// quit = "q"
/// play_pause = "Space"
/// next_track = "n"
/// prev_track = "b"
///
/// [keybindings.pages.search]
/// select_up = "k"
/// select_down = "j"
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct KeybindingConfig {
    /// 全局键位映射：动作 -> 键位字符串
    #[serde(default = "default_global_bindings")]
    pub global: HashMap<Action, String>,
    /// 页面级键位映射：页面名 -> (动作 -> 键位字符串)
    #[serde(default = "default_page_bindings")]
    pub pages: HashMap<String, HashMap<Action, String>>,
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        Self {
            global: default_global_bindings(),
            pages: default_page_bindings(),
        }
    }
}

impl KeybindingConfig {
    /// 反查某动作当前绑定的键位展示串（页面级优先，全局兜底）。
    ///
    /// 查找顺序与 [`KeybindingResolver::resolve`] 一致。返回配置里的原始
    /// 键位串（如 `"Ctrl+o"`、`"Space"`），供底栏、菜单等处的提示直接展示
    /// ——此前这些提示是硬编码的，用户改键后会失真。动作没有绑定时返回
    /// `None`，调用方应整体省略提示，而不是显示"未绑定"。
    pub fn key_hint(&self, page: Option<&str>, action: Action) -> Option<&str> {
        if let Some(page) = page
            && let Some(key) = self.pages.get(page).and_then(|map| map.get(&action))
        {
            return Some(key.as_str());
        }
        self.global.get(&action).map(String::as_str)
    }
}

/// 设置页的"逐行动作"绑定：这些动作已经删除（设置项改为 `Enter` 激活）。
///
/// 保留这份清单有两个用途：迁移时把旧配置里的残留绑定清掉，以及测试里断言
/// 设置页的默认键位不再包含任何逐行快捷键。
pub const SETTINGS_ROW_ACTIONS: [Action; 19] = [
    Action::SettingsToggle,
    Action::SettingsCyclePlaybackSpeed,
    Action::SettingsEditAudioDevice,
    Action::SettingsCycleReplayGainMode,
    Action::SettingsCycleReplayGainPreamp,
    Action::SettingsCycleChannelMode,
    Action::SettingsCycleBalance,
    Action::SettingsToggleReplayGainClip,
    Action::SettingsCycleFadeInDuration,
    Action::SettingsCycleFadeOutDuration,
    Action::SettingsCycleEqualizerPreset,
    Action::SettingsRunFadeIn,
    Action::SettingsRunFadeOut,
    Action::SettingsSetAbLoopStart,
    Action::SettingsSetAbLoopEnd,
    Action::SettingsClearAbLoop,
    Action::SettingsExportData,
    Action::SettingsImportData,
    Action::SettingsImportPlaylist,
];

/// 旧版设置页键位的迁移。
///
/// 两件事：
/// 1. **删除**已经消失的逐行动作绑定（`SETTINGS_ROW_ACTIONS`）。这些动作不再被
///    设置页处理，留在配置里只会变成"按下去没反应"的死键 —— 旧版把裸数字改成
///    `F1` 之类的做法在动作本身被删掉之后已经没有意义。
/// 2. 仍然存在的键位（列表导航 / 返回）如果被裸数字占用，迁回默认绑定。
///    裸数字键在设置页始终保留给侧边栏；用户自己改过的组合键不受影响。
pub fn migrate_legacy_settings_bindings(config: &mut KeybindingConfig) -> bool {
    let Some(settings) = config.pages.get_mut("settings") else {
        return false;
    };
    let defaults = default_page_bindings()
        .remove("settings")
        .expect("default settings bindings must exist");

    let mut changed = false;
    let stale: Vec<Action> = settings
        .keys()
        .copied()
        .filter(|action| SETTINGS_ROW_ACTIONS.contains(action))
        .collect();
    for action in stale {
        settings.remove(&action);
        changed = true;
    }

    for (action, binding) in settings.iter_mut() {
        let is_bare_digit = binding.len() == 1 && binding.as_bytes()[0].is_ascii_digit();
        if is_bare_digit
            && let Some(default) = defaults.get(action)
            && binding != default
        {
            binding.clone_from(default);
            changed = true;
        }
    }
    changed
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PartialKeybindingConfig {
    global: HashMap<Action, String>,
    pages: HashMap<String, HashMap<Action, String>>,
}

impl<'de> Deserialize<'de> for KeybindingConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let partial = PartialKeybindingConfig::deserialize(deserializer)?;
        let mut config = Self::default();
        config.global.extend(partial.global);
        for (page, bindings) in partial.pages {
            config.pages.entry(page).or_default().extend(bindings);
        }
        Ok(config)
    }
}

/// 默认全局键位（与现有硬编码完全一致）
fn default_global_bindings() -> HashMap<Action, String> {
    let mut m = HashMap::new();
    m.insert(Action::GlobalQuit, "q".to_string());
    m.insert(Action::GlobalPlayPause, "Space".to_string());
    m.insert(Action::GlobalNextTrack, "n".to_string());
    m.insert(Action::GlobalPrevTrack, "b".to_string());
    m.insert(Action::GlobalCycleMode, "m".to_string());
    // The application gates these global actions to the queue page; search
    // reuses the bracket keys for source switching.
    m.insert(Action::GlobalSeekForward, "]".to_string());
    m.insert(Action::GlobalSeekBackward, "[".to_string());
    m.insert(Action::GlobalVolumeUp, ".".to_string());
    m.insert(Action::GlobalVolumeDown, ",".to_string());
    m.insert(Action::GlobalNextTab, "Tab".to_string());
    m.insert(Action::GlobalPrevTab, "Shift+Tab".to_string());
    m.insert(Action::GlobalGoToMain, "Esc".to_string());
    m.insert(Action::GlobalToggleFavorite, "Ctrl+l".to_string());
    // Ctrl+S / Ctrl+O 不与页面内的小写 s / o 冲突，也不占用列表翻页键。
    m.insert(Action::GlobalDownloadCurrent, "Ctrl+s".to_string());
    m.insert(Action::GlobalDownloadsPanel, "Ctrl+o".to_string());
    m.insert(Action::GlobalRedraw, "Ctrl+r".to_string());
    // 面板布局复位：与强制重绘同源（都只影响界面），但用另一个键区分。
    // 不用 Ctrl+Shift+R：多数终端把 Ctrl+Shift+<字母> 报成和 Ctrl+<字母> 同样的
    // 字节，拿不到 SHIFT 修饰位，按下去会变成强制重绘。
    m.insert(Action::GlobalResetLayout, "Ctrl+g".to_string());
    // 睡眠定时器：单键 mnemonic（t = timer），小写 t 尚未被任何页面占用。
    m.insert(Action::GlobalSleepTimer, "t".to_string());
    // 频谱可视化：w = wave（波形/频谱），小写 w 尚未被占用。
    m.insert(Action::GlobalVisualizer, "w".to_string());
    m
}

/// 默认页面级键位（与现有硬编码完全一致）
fn default_page_bindings() -> HashMap<String, HashMap<Action, String>> {
    let mut pages = HashMap::new();

    // --- 搜索页面 ---
    let mut search = HashMap::new();
    search.insert(Action::SearchInputMode, "i".to_string());
    search.insert(Action::SearchStart, "Enter".to_string());
    search.insert(Action::SearchToggleAggregate, "v".to_string());
    search.insert(Action::ListSelectUp, "k".to_string());
    search.insert(Action::ListSelectDown, "j".to_string());
    search.insert(Action::ListSelectFirst, "g".to_string());
    search.insert(Action::ListSelectLast, "G".to_string());
    search.insert(Action::ListPageUp, "Ctrl+u".to_string());
    search.insert(Action::ListPageDown, "Ctrl+d".to_string());
    search.insert(Action::ListActivate, "l".to_string());
    search.insert(Action::ListToggleFavorite, "f".to_string());
    search.insert(Action::ListAddToQueue, "a".to_string());
    search.insert(Action::ListAddToQueueNext, "A".to_string());
    search.insert(Action::ListDownload, "D".to_string());
    search.insert(Action::SearchCycleSourcePrev, "Left".to_string());
    search.insert(Action::SearchCycleSourceNext, "Right".to_string());
    search.insert(Action::ListGoBack, "Esc".to_string());
    search.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("search".to_string(), search);

    // --- 主页（队列） ---
    let mut main = HashMap::new();
    main.insert(Action::ListSelectUp, "Up".to_string());
    main.insert(Action::ListSelectDown, "Down".to_string());
    main.insert(Action::ListSelectFirst, "g".to_string());
    main.insert(Action::ListSelectLast, "G".to_string());
    main.insert(Action::ListPageUp, "Ctrl+u".to_string());
    main.insert(Action::ListPageDown, "Ctrl+d".to_string());
    main.insert(Action::ListActivate, "Enter".to_string());
    main.insert(Action::ListToggleFavorite, "f".to_string());
    // 队列页的 D 已经用于「清空队列」，下载沿用 Ctrl+S 的语义，落在选中的队列项上。
    main.insert(Action::ListDownload, "Ctrl+s".to_string());
    main.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("main".to_string(), main);

    // --- 排行榜 ---
    let mut leaderboard = HashMap::new();
    leaderboard.insert(Action::ListSelectUp, "k".to_string());
    leaderboard.insert(Action::ListSelectDown, "j".to_string());
    leaderboard.insert(Action::ListSelectFirst, "g".to_string());
    leaderboard.insert(Action::ListSelectLast, "G".to_string());
    leaderboard.insert(Action::ListPageUp, "Ctrl+u".to_string());
    leaderboard.insert(Action::ListPageDown, "Ctrl+d".to_string());
    leaderboard.insert(Action::ListActivate, "Enter".to_string());
    leaderboard.insert(Action::ListToggleFavorite, "f".to_string());
    leaderboard.insert(Action::ListAddToQueue, "a".to_string());
    leaderboard.insert(Action::ListAddToQueueNext, "A".to_string());
    leaderboard.insert(Action::ListDownload, "D".to_string());
    leaderboard.insert(Action::SearchCycleSourcePrev, "Left".to_string());
    leaderboard.insert(Action::SearchCycleSourceNext, "Right".to_string());
    leaderboard.insert(Action::ListGoBack, "Esc".to_string());
    leaderboard.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("leaderboard".to_string(), leaderboard);

    // --- 歌单 ---
    let mut playlists = HashMap::new();
    playlists.insert(Action::ListSelectUp, "k".to_string());
    playlists.insert(Action::ListSelectDown, "j".to_string());
    playlists.insert(Action::ListSelectFirst, "g".to_string());
    playlists.insert(Action::ListSelectLast, "G".to_string());
    playlists.insert(Action::ListPageUp, "Ctrl+u".to_string());
    playlists.insert(Action::ListPageDown, "Ctrl+d".to_string());
    playlists.insert(Action::ListActivate, "Enter".to_string());
    playlists.insert(Action::ListToggleFavorite, "f".to_string());
    playlists.insert(Action::ListAddToQueue, "a".to_string());
    playlists.insert(Action::ListAddToQueueNext, "A".to_string());
    playlists.insert(Action::ListDownload, "D".to_string());
    playlists.insert(Action::SearchCycleSourcePrev, "Left".to_string());
    playlists.insert(Action::SearchCycleSourceNext, "Right".to_string());
    playlists.insert(Action::ListGoBack, "Esc".to_string());
    playlists.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("playlists".to_string(), playlists);

    // --- 收藏 ---
    let mut favorites = HashMap::new();
    favorites.insert(Action::FavoritesFilter, "/".to_string());
    favorites.insert(Action::ListSelectUp, "k".to_string());
    favorites.insert(Action::ListSelectDown, "j".to_string());
    favorites.insert(Action::ListSelectFirst, "g".to_string());
    favorites.insert(Action::ListSelectLast, "G".to_string());
    favorites.insert(Action::ListPageUp, "Ctrl+u".to_string());
    favorites.insert(Action::ListPageDown, "Ctrl+d".to_string());
    favorites.insert(Action::ListActivate, "Enter".to_string());
    favorites.insert(Action::ListToggleFavorite, "f".to_string());
    favorites.insert(Action::ListAddToQueue, "a".to_string());
    favorites.insert(Action::ListAddToQueueNext, "A".to_string());
    favorites.insert(Action::ListCycleSort, "s".to_string());
    favorites.insert(Action::FavoritesRemove, "d".to_string());
    favorites.insert(Action::ListDownload, "D".to_string());
    favorites.insert(Action::ListGoBack, "Esc".to_string());
    favorites.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("favorites".to_string(), favorites);

    // --- 历史 ---
    let mut history = HashMap::new();
    history.insert(Action::ListSelectUp, "k".to_string());
    history.insert(Action::ListSelectDown, "j".to_string());
    history.insert(Action::ListSelectFirst, "g".to_string());
    history.insert(Action::ListSelectLast, "G".to_string());
    history.insert(Action::ListPageUp, "Ctrl+u".to_string());
    history.insert(Action::ListPageDown, "Ctrl+d".to_string());
    history.insert(Action::ListActivate, "Enter".to_string());
    history.insert(Action::ListToggleFavorite, "f".to_string());
    history.insert(Action::ListAddToQueue, "a".to_string());
    history.insert(Action::ListAddToQueueNext, "A".to_string());
    history.insert(Action::ListDownload, "D".to_string());
    history.insert(Action::ListCycleSort, "s".to_string());
    history.insert(Action::HistoryFilter, "/".to_string());
    history.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("history".to_string(), history);

    // --- 本地音乐 ---
    let mut local = HashMap::new();
    local.insert(Action::ListSelectUp, "k".to_string());
    local.insert(Action::ListSelectDown, "j".to_string());
    local.insert(Action::ListSelectFirst, "g".to_string());
    local.insert(Action::ListSelectLast, "G".to_string());
    local.insert(Action::ListPageUp, "Ctrl+u".to_string());
    local.insert(Action::ListPageDown, "Ctrl+d".to_string());
    local.insert(Action::ListActivate, "Enter".to_string());
    local.insert(Action::ListToggleFavorite, "f".to_string());
    local.insert(Action::ListAddToQueue, "a".to_string());
    local.insert(Action::ListAddToQueueNext, "A".to_string());
    local.insert(Action::ListCycleSort, "s".to_string());
    local.insert(Action::LocalRescan, "r".to_string());
    local.insert(Action::LocalDelete, "d".to_string());
    local.insert(Action::LocalFilter, "/".to_string());
    local.insert(Action::ListContextMenu, "x".to_string());
    pages.insert("local".to_string(), local);

    // --- 设置 ---
    //
    // 设置页只保留"分类 / 导航 / 激活"三类键：设置项本身一律靠 `Enter`
    // （`Space` 对开关与取值行等价）或鼠标点击激活，因此这里**只有**列表导航
    // 与返回，不再有任何逐行动作的默认绑定。`Action::Settings*` 那些变体仍然
    // 存在（用户配置里的旧绑定会被保留，但设置页不再拦截它们，见
    // `app/src/pages/settings.rs` 的 `settings_action_is_page_owned`）。
    let mut settings = HashMap::new();
    settings.insert(Action::ListSelectUp, "k".to_string());
    settings.insert(Action::ListSelectDown, "j".to_string());
    settings.insert(Action::ListGoBack, "Esc".to_string());
    pages.insert("settings".to_string(), settings);

    // --- B站登录 ---
    let mut bili_login = HashMap::new();
    bili_login.insert(Action::ListGoBack, "Esc".to_string());
    pages.insert("bili_login".to_string(), bili_login);

    // --- 歌手/专辑详情 ---
    let mut details = HashMap::new();
    details.insert(Action::ListSelectUp, "k".to_string());
    details.insert(Action::ListSelectDown, "j".to_string());
    details.insert(Action::ListSelectFirst, "g".to_string());
    details.insert(Action::ListSelectLast, "G".to_string());
    details.insert(Action::ListPageUp, "Ctrl+u".to_string());
    details.insert(Action::ListPageDown, "Ctrl+d".to_string());
    details.insert(Action::ListActivate, "Enter".to_string());
    details.insert(Action::ListToggleFavorite, "f".to_string());
    details.insert(Action::ListAddToQueue, "a".to_string());
    details.insert(Action::ListAddToQueueNext, "A".to_string());
    details.insert(Action::ListDownload, "D".to_string());
    details.insert(Action::ListGoBack, "Esc".to_string());
    pages.insert("details".to_string(), details);

    pages
}

// =============================================================================
// KeyBinding：运行时解析后的键位表示
// =============================================================================

/// 解析后的键位，可直接与 `KeyEvent` 匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

/// 不依赖终端后端的键位描述：桌面 GUI 用它与 egui 的按键事件比对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyGesture {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// 字符键为 `Some(c)`（已按核心规则大写归一），特殊键为 `None`。
    pub character: Option<char>,
    /// 特殊键名（`F5`、`Enter`、`Space`…），字符键为 `None`。
    pub special: Option<&'static str>,
}

impl KeyBinding {
    /// 转换成与后端无关的描述（GUI 侧匹配用）。
    pub fn gesture(&self) -> KeyGesture {
        let character = match self.code {
            KeyCode::Char(character) => Some(character),
            _ => None,
        };
        let special = Some(match self.code {
            KeyCode::Enter => "Enter",
            KeyCode::Esc => "Esc",
            KeyCode::Tab => "Tab",
            KeyCode::Backspace => "Backspace",
            KeyCode::Delete => "Delete",
            KeyCode::Home => "Home",
            KeyCode::End => "End",
            KeyCode::PageUp => "PageUp",
            KeyCode::PageDown => "PageDown",
            KeyCode::Up => "Up",
            KeyCode::Down => "Down",
            KeyCode::Left => "Left",
            KeyCode::Right => "Right",
            KeyCode::F(number) => match number {
                1 => "F1",
                2 => "F2",
                3 => "F3",
                4 => "F4",
                5 => "F5",
                6 => "F6",
                7 => "F7",
                8 => "F8",
                9 => "F9",
                10 => "F10",
                11 => "F11",
                12 => "F12",
                _ => "F",
            },
            _ => {
                return KeyGesture {
                    ctrl: self.modifiers.contains(KeyModifiers::CONTROL),
                    alt: self.modifiers.contains(KeyModifiers::ALT),
                    shift: self.modifiers.contains(KeyModifiers::SHIFT),
                    character,
                    special: None,
                };
            }
        });
        KeyGesture {
            ctrl: self.modifiers.contains(KeyModifiers::CONTROL),
            alt: self.modifiers.contains(KeyModifiers::ALT),
            shift: self.modifiers.contains(KeyModifiers::SHIFT),
            character,
            special,
        }
    }

    /// 从 `KeyEvent` 创建
    pub fn from_event(event: KeyEvent) -> Self {
        normalize_char_binding(event.modifiers, event.code)
    }

    /// 匹配一个 `KeyEvent`（忽略 kind 和 state）
    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }
}

// =============================================================================
// 解析器：字符串 -> KeyBinding
// =============================================================================

/// 解析键位描述字符串，如 `"Ctrl+l"`、`"Space"`、`"Shift+Tab"`。
///
/// 支持的修饰符前缀：`Ctrl+`, `Shift+`, `Alt+`, `Ctrl+Shift+` 等组合。
/// 支持的特殊键名：
/// - `Space`, `Tab`, `BackTab`, `Enter`, `Esc`, `Backspace`
/// - `Up`, `Down`, `Left`, `Right`
/// - `Home`, `End`, `PageUp`, `PageDown`
/// - `Insert`, `Delete`
/// - `F1` ~ `F12`
pub fn parse_keybinding(desc: &str) -> Option<KeyBinding> {
    let desc = desc.trim();
    if desc.is_empty() {
        return None;
    }

    // 解析修饰符
    let mut modifiers = KeyModifiers::NONE;
    let mut remaining = desc;

    loop {
        if let Some(rest) = remaining.strip_prefix("Ctrl+") {
            modifiers |= KeyModifiers::CONTROL;
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix("Shift+") {
            modifiers |= KeyModifiers::SHIFT;
            remaining = rest;
        } else if let Some(_rest) = remaining.strip_prefix("Alt+") {
            modifiers |= KeyModifiers::ALT;
            remaining = _rest;
        } else {
            break;
        }
    }

    // 特殊处理：Shift+Tab 在 crossterm 中报告为 BackTab + SHIFT
    if modifiers == KeyModifiers::SHIFT && remaining == "Tab" {
        return Some(KeyBinding {
            modifiers: KeyModifiers::SHIFT,
            code: KeyCode::BackTab,
        });
    }

    // 解析键码
    let code = parse_keycode(remaining)?;

    Some(normalize_char_binding(modifiers, code))
}

fn normalize_char_binding(mut modifiers: KeyModifiers, mut code: KeyCode) -> KeyBinding {
    if let KeyCode::Char(character) = code
        && modifiers.contains(KeyModifiers::SHIFT)
    {
        modifiers.remove(KeyModifiers::SHIFT);
        code = KeyCode::Char(if character.is_ascii_lowercase() {
            character.to_ascii_uppercase()
        } else {
            character
        });
    }
    KeyBinding { modifiers, code }
}

fn parse_keycode(s: &str) -> Option<KeyCode> {
    match s {
        "Space" | " " => Some(KeyCode::Char(' ')),
        "Tab" => Some(KeyCode::Tab),
        "BackTab" => Some(KeyCode::BackTab),
        "Enter" | "Return" => Some(KeyCode::Enter),
        "Esc" | "Escape" => Some(KeyCode::Esc),
        "Backspace" => Some(KeyCode::Backspace),
        "Up" => Some(KeyCode::Up),
        "Down" => Some(KeyCode::Down),
        "Left" => Some(KeyCode::Left),
        "Right" => Some(KeyCode::Right),
        "Home" => Some(KeyCode::Home),
        "End" => Some(KeyCode::End),
        "PageUp" => Some(KeyCode::PageUp),
        "PageDown" => Some(KeyCode::PageDown),
        "Insert" => Some(KeyCode::Insert),
        "Delete" => Some(KeyCode::Delete),
        "CapsLock" => Some(KeyCode::CapsLock),
        "Null" => Some(KeyCode::Null),
        _ => {
            // 尝试解析 F1-F12
            if s.len() >= 2
                && s.starts_with('F')
                && let Ok(n) = s[1..].parse::<u8>()
                && (1..=12).contains(&n)
            {
                return Some(KeyCode::F(n));
            }
            // 尝试解析单个字符
            if s.len() == 1 {
                let c = s.chars().next().unwrap();
                return Some(KeyCode::Char(c));
            }
            None
        }
    }
}

// =============================================================================
// KeybindingResolver：运行时快速查表
// =============================================================================

/// 运行时键位解析器。
///
/// 把配置中的字符串键位预解析为 `HashMap<KeyBinding, Action>`，
/// 实现 O(1) 的事件到动作查找。
pub struct KeybindingResolver {
    global: HashMap<KeyBinding, Action>,
    pages: HashMap<String, HashMap<KeyBinding, Action>>,
}

impl KeybindingResolver {
    /// 从配置创建解析器
    pub fn from_config(config: &KeybindingConfig) -> Self {
        let mut global = HashMap::new();
        for (action, key_str) in &config.global {
            match parse_keybinding(key_str) {
                Some(binding) => {
                    if let Some(previous) = global.insert(binding, *action) {
                        tracing::warn!(
                            "全局快捷键 {key_str} 同时绑定了 {previous:?} 和 {action:?}，保留后者"
                        );
                    }
                }
                None => tracing::warn!("无法识别的全局快捷键配置: {action:?} = {key_str}"),
            }
        }

        let mut pages = HashMap::new();
        for (page_name, page_bindings) in &config.pages {
            let mut page_map = HashMap::new();
            for (action, key_str) in page_bindings {
                match parse_keybinding(key_str) {
                    Some(binding) => {
                        if let Some(previous) = page_map.insert(binding, *action) {
                            tracing::warn!(
                                "页面 {page_name} 快捷键 {key_str} 同时绑定了 {previous:?} 和 {action:?}，保留后者"
                            );
                        }
                    }
                    None => tracing::warn!(
                        "无法识别的页面快捷键配置: {page_name}/{action:?} = {key_str}"
                    ),
                }
            }
            pages.insert(page_name.clone(), page_map);
        }

        Self { global, pages }
    }

    /// 解析全局键位事件
    pub fn resolve_global(&self, event: &KeyEvent) -> Option<Action> {
        let binding = KeyBinding::from_event(*event);
        self.global.get(&binding).copied()
    }

    /// 解析页面级键位事件
    pub fn resolve_page(&self, page: &str, event: &KeyEvent) -> Option<Action> {
        let binding = KeyBinding::from_event(*event);
        self.pages
            .get(page)
            .and_then(|map| map.get(&binding).copied())
    }

    /// 同时查询全局和页面级（页面级优先）
    pub fn resolve(&self, page: &str, event: &KeyEvent) -> Option<Action> {
        self.resolve_page(page, event)
            .or_else(|| self.resolve_global(event))
    }
}

// =============================================================================
// 工具函数
// =============================================================================

/// 生成 Colemak 布局预设配置。
///
/// 可作为 `config.toml` 中 `[keybindings]` 节的参考示例。
pub fn colemak_preset() -> KeybindingConfig {
    let mut config = KeybindingConfig::default();
    config
        .global
        .insert(Action::GlobalNextTrack, "k".to_string());
    config
        .global
        .insert(Action::GlobalPrevTrack, "h".to_string());

    for page in [
        "search",
        "main",
        "leaderboard",
        "playlists",
        "favorites",
        "history",
        "local",
        "settings",
        "details",
    ] {
        if let Some(bindings) = config.pages.get_mut(page) {
            bindings.insert(Action::ListSelectUp, "e".to_string());
            bindings.insert(Action::ListSelectDown, "n".to_string());
        }
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hint_prefers_the_page_binding_and_falls_back_to_global() {
        let mut config = KeybindingConfig::default();
        // 全局绑定兜底。
        assert_eq!(
            config.key_hint(Some("main"), Action::GlobalPlayPause),
            config
                .global
                .get(&Action::GlobalPlayPause)
                .map(String::as_str)
        );
        // 页面级优先。
        config
            .pages
            .get_mut("main")
            .expect("默认表含 main 页")
            .insert(Action::ListCycleSort, "S".to_string());
        assert_eq!(
            config.key_hint(Some("main"), Action::ListCycleSort),
            Some("S")
        );
        assert_ne!(
            config.key_hint(Some("local"), Action::ListCycleSort),
            Some("S")
        );
        // 用户删除绑定后返回 None，提示应整体省略。
        config.global.remove(&Action::GlobalPlayPause);
        if let Some(page) = config.pages.get_mut("main") {
            page.remove(&Action::GlobalPlayPause);
        }
        assert_eq!(config.key_hint(Some("main"), Action::GlobalPlayPause), None);
    }

    /// 面板布局复位必须有一键入口，而且不能和强制重绘撞键。
    ///
    /// 这条绑定以前只存在于表头右键菜单里，无鼠标环境（SSH / tmux 键盘流）
    /// 根本按不到；`Ctrl+Shift+R` 也不能用——多数终端把 `Ctrl+Shift+<字母>`
    /// 报成与 `Ctrl+<字母>` 相同的字节，拿不到 SHIFT 修饰位。
    #[test]
    fn resetting_the_pane_layout_has_its_own_keyboard_binding() {
        let config = KeybindingConfig::default();
        let reset = config
            .global
            .get(&Action::GlobalResetLayout)
            .expect("布局复位应有默认全局键位");
        let redraw = config
            .global
            .get(&Action::GlobalRedraw)
            .expect("强制重绘应有默认全局键位");
        assert_ne!(reset, redraw, "复位布局不能和强制重绘共用同一个键");
        assert_eq!(reset, "Ctrl+g");

        // 展示名与动作清单都要跟上（帮助浮层按 ALL 顺序渲染）。
        assert!(Action::ALL.contains(&Action::GlobalResetLayout));
        assert_eq!(
            Action::GlobalResetLayout.label(),
            "恢复当前页面默认面板布局"
        );

        // 这个键真的能解析出"Ctrl+G"，不会被别的绑定抢先匹配
        let resolver = KeybindingResolver::from_config(&config);
        let event = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert!(
            resolver
                .resolve_global(&event)
                .is_some_and(|action| action == Action::GlobalResetLayout),
            "Ctrl+G 应解析为布局复位"
        );
    }

    /// 无鼠标环境下的右键替代入口：所有歌曲列表页都必须绑上 `ListContextMenu`，
    /// 否则"清空历史/删除本地/查看歌手/播放控制"这些只在菜单里的功能不可达。
    #[test]
    fn song_list_pages_bind_a_keyboard_context_menu_entry() {
        let config = KeybindingConfig::default();
        for page in [
            "main",
            "search",
            "leaderboard",
            "playlists",
            "favorites",
            "history",
            "local",
        ] {
            let bindings = config
                .pages
                .get(page)
                .unwrap_or_else(|| panic!("{page} 应有默认页键位"));
            assert_eq!(
                bindings.get(&Action::ListContextMenu).map(String::as_str),
                Some("x"),
                "{page} 应把打开上下文菜单绑到 x"
            );
        }
        // 设置页没有歌曲列表，不该占用这个键
        assert!(
            !config
                .pages
                .get("settings")
                .is_some_and(|b| b.contains_key(&Action::ListContextMenu))
        );
    }

    #[test]
    fn parse_simple_char() {
        let b = parse_keybinding("q").unwrap();
        assert_eq!(b.code, KeyCode::Char('q'));
        assert_eq!(b.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_ctrl_combo() {
        let b = parse_keybinding("Ctrl+l").unwrap();
        assert_eq!(b.code, KeyCode::Char('l'));
        assert_eq!(b.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_tab() {
        let b = parse_keybinding("Shift+Tab").unwrap();
        assert_eq!(b.code, KeyCode::BackTab);
        assert_eq!(b.modifiers, KeyModifiers::SHIFT);
    }

    #[test]
    fn parse_space() {
        let b = parse_keybinding("Space").unwrap();
        assert_eq!(b.code, KeyCode::Char(' '));
        assert_eq!(b.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn parse_f_key() {
        let b = parse_keybinding("F5").unwrap();
        assert_eq!(b.code, KeyCode::F(5));
    }

    #[test]
    fn resolver_lookup() {
        let mut config = KeybindingConfig::default();
        config.global.insert(Action::GlobalQuit, "q".to_string());

        let resolver = KeybindingResolver::from_config(&config);
        let event = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(resolver.resolve_global(&event), Some(Action::GlobalQuit));
    }

    #[test]
    fn resolver_page_priority() {
        let mut config = KeybindingConfig::default();
        config
            .global
            .insert(Action::ListSelectDown, "j".to_string());

        let mut search = HashMap::new();
        search.insert(Action::ListSelectDown, "n".to_string());
        config.pages.insert("search".to_string(), search);

        let resolver = KeybindingResolver::from_config(&config);
        let event = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(
            resolver.resolve("search", &event),
            Some(Action::ListSelectDown)
        );
    }

    #[test]
    fn default_config_contains_all_default_scopes() {
        let config = KeybindingConfig::default();

        // 下载相关的默认键位：全局 Ctrl+S/Ctrl+O，列表页 D。
        assert_eq!(
            config.global.get(&Action::GlobalDownloadCurrent),
            Some(&"Ctrl+s".to_string())
        );
        assert_eq!(
            config.global.get(&Action::GlobalDownloadsPanel),
            Some(&"Ctrl+o".to_string())
        );
        for page in [
            "search",
            "leaderboard",
            "playlists",
            "favorites",
            "history",
            "details",
        ] {
            assert_eq!(
                config
                    .pages
                    .get(page)
                    .and_then(|bindings| bindings.get(&Action::ListDownload)),
                Some(&"D".to_string()),
                "{page} 的下载键位应为 D"
            );
        }
        // 队列页的 D 已用于清空队列，下载走 Ctrl+S。
        assert_eq!(
            config
                .pages
                .get("main")
                .and_then(|bindings| bindings.get(&Action::ListDownload)),
            Some(&"Ctrl+s".to_string())
        );

        assert_eq!(
            config.global.get(&Action::GlobalQuit),
            Some(&"q".to_string())
        );
        assert_eq!(
            config
                .pages
                .get("local")
                .and_then(|page| page.get(&Action::LocalRescan)),
            Some(&"r".to_string())
        );
        for page in ["favorites", "history", "local"] {
            assert_eq!(
                config
                    .pages
                    .get(page)
                    .and_then(|bindings| bindings.get(&Action::ListCycleSort)),
                Some(&"s".to_string())
            );
        }
    }

    #[test]
    fn list_favorite_action_defaults_to_f_on_song_pages() {
        let config = KeybindingConfig::default();
        for page in [
            "main",
            "search",
            "leaderboard",
            "playlists",
            "favorites",
            "history",
            "local",
        ] {
            assert_eq!(
                config
                    .pages
                    .get(page)
                    .and_then(|bindings| bindings.get(&Action::ListToggleFavorite)),
                Some(&"f".to_string())
            );
        }
        assert_eq!(
            config.global.get(&Action::GlobalToggleFavorite),
            Some(&"Ctrl+l".to_string())
        );
    }

    /// 设置页的默认键位只剩"列表导航 + 返回"：逐行动作快捷键已全部删除。
    #[test]
    fn settings_defaults_only_keep_navigation_keys() {
        let config = KeybindingConfig::default();
        let settings = config.pages.get("settings").unwrap();
        assert_eq!(
            settings.get(&Action::ListSelectUp).map(String::as_str),
            Some("k")
        );
        assert_eq!(
            settings.get(&Action::ListSelectDown).map(String::as_str),
            Some("j")
        );
        assert_eq!(
            settings.get(&Action::ListGoBack).map(String::as_str),
            Some("Esc")
        );
        assert_eq!(
            settings.len(),
            3,
            "设置页默认键位只剩导航（实际 {settings:?}）"
        );

        // 任何一个 `Settings*` 逐行动作都不许再有默认绑定
        for action in SETTINGS_ROW_ACTIONS {
            assert!(
                settings.get(&action).is_none(),
                "{action:?} 的行内快捷键已经删除，不该再有默认绑定"
            );
        }
    }

    #[test]
    fn migrates_legacy_settings_keys_without_overwriting_modified_combinations() {
        let mut config = KeybindingConfig::default();
        let settings = config.pages.get_mut("settings").unwrap();
        // 逐行动作已经从默认表里删除：它们在旧配置里的残留绑定会被清掉
        // （留着只会是"按下去没反应"的死键），无论用户当初把它绑到什么键上。
        settings.insert(Action::SettingsCyclePlaybackSpeed, "1".to_string());
        settings.insert(Action::SettingsRunFadeIn, "F".to_string());
        settings.insert(Action::SettingsImportData, "Ctrl+7".to_string());
        // 裸数字占用了仍然存在的导航键 → 迁回默认
        settings.insert(Action::ListSelectUp, "8".to_string());

        assert!(migrate_legacy_settings_bindings(&mut config));
        let settings = config.pages.get("settings").unwrap();
        for action in SETTINGS_ROW_ACTIONS {
            assert!(
                settings.get(&action).is_none(),
                "{action:?} 已经删除，旧绑定必须被清掉"
            );
        }
        assert_eq!(
            settings.get(&Action::ListSelectUp).map(String::as_str),
            Some("k"),
            "仍在使用的导航键从裸数字迁回默认"
        );
        assert!(!migrate_legacy_settings_bindings(&mut config));
    }

    #[test]
    fn partial_config_keeps_unspecified_defaults() {
        let config: KeybindingConfig = serde_json::from_value(serde_json::json!({
            "global": {
                "global_quit": "Ctrl+q"
            },
            "pages": {
                "local": {
                    "list_select_up": "e"
                }
            }
        }))
        .unwrap();

        assert_eq!(
            config.global.get(&Action::GlobalQuit),
            Some(&"Ctrl+q".to_string())
        );
        assert_eq!(
            config.global.get(&Action::GlobalPlayPause),
            Some(&"Space".to_string())
        );
        assert_eq!(
            config
                .pages
                .get("local")
                .and_then(|page| page.get(&Action::ListSelectUp)),
            Some(&"e".to_string())
        );
        assert_eq!(
            config
                .pages
                .get("local")
                .and_then(|page| page.get(&Action::ListSelectDown)),
            Some(&"j".to_string())
        );
        assert!(config.pages.contains_key("search"));
    }

    #[test]
    fn uppercase_binding_matches_kitty_shift_event() {
        let mut config = KeybindingConfig::default();
        config
            .pages
            .get_mut("local")
            .unwrap()
            .insert(Action::ListSelectLast, "G".to_string());
        let resolver = KeybindingResolver::from_config(&config);
        let event = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);

        assert_eq!(
            resolver.resolve_page("local", &event),
            Some(Action::ListSelectLast)
        );
    }

    #[test]
    fn colemak_preset_keeps_defaults_and_updates_every_list_page() {
        let config = colemak_preset();

        assert_eq!(
            config.global.get(&Action::GlobalPlayPause),
            Some(&"Space".to_string())
        );
        for page in [
            "search",
            "main",
            "leaderboard",
            "playlists",
            "favorites",
            "history",
            "local",
            "settings",
        ] {
            let bindings = config.pages.get(page).unwrap();
            assert_eq!(bindings.get(&Action::ListSelectUp), Some(&"e".to_string()));
            assert_eq!(
                bindings.get(&Action::ListSelectDown),
                Some(&"n".to_string())
            );
        }
        assert_eq!(
            config
                .pages
                .get("local")
                .and_then(|page| page.get(&Action::LocalRescan)),
            Some(&"r".to_string())
        );
    }
}
