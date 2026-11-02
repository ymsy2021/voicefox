#![allow(dead_code)]

//! 主题解析：配置里的 `[theme]` 槽位（或主题库具名主题）→ 实际渲染色。
//!
//! 放在 `voicefox-runtime` 而不是 TUI 里，是为了让桌面 GUI 也能按同一份
//! 配置取色：两套前端共用同一主题来源，不会漂移。TUI 专有的部分（封面
//! 主色混合）留在 `voicefox-app::theme` 的 shim 里。

use lx_core::model::config::ThemeConfig;
use ratatui::style::Color;
use ratatui_themes::{ThemeName, ThemePalette};

/// 三原色通道（0–255）。GUI 侧换算成自己的颜色类型；TUI 侧再包成 ratatui 色。
pub type Rgb = [u8; 3];

/// 默认皮肤：使用 `[theme]` 里手工调好的槽位（与历史版本观感一致）。
pub const SKIN_VOICEFOX: &str = "voicefox";

/// 可选的界面主题名：`voicefox` + 主题库的全部主题，顺序即循环顺序。
pub fn skin_names() -> Vec<String> {
    let mut names = vec![SKIN_VOICEFOX.to_string()];
    names.extend(ThemeName::all().iter().map(|name| skin_key(*name)));
    names
}

/// 主题在配置里的名字。
///
/// 直接用主题库的 serde 表现（kebab-case，如 `tokyo-night`），
/// 而不是自己按显示名拼，避免两边命名规则漂移。
fn skin_key(name: ThemeName) -> String {
    serde_json::to_value(name)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 主题名的可读标签（设置页显示用）。
pub fn skin_label(name: &str) -> String {
    match library_theme(name) {
        Some(theme) => theme.display_name().to_string(),
        None => "Voicefox".to_string(),
    }
}

/// 循环到下一个主题名。
pub fn next_skin_name(current: &str) -> String {
    let names = skin_names();
    let index = names.iter().position(|name| name == current).unwrap_or(0);
    names[(index + 1) % names.len()].clone()
}

/// 主题名 → 主题库主题；未知名字返回 `None`（调用方回退到 voicefox 槽位）。
fn library_theme(value: &str) -> Option<ThemeName> {
    let wanted = value.trim().to_ascii_lowercase();
    ThemeName::all()
        .iter()
        .copied()
        .find(|name| skin_key(*name) == wanted)
}

/// 两个 RGB 按比例混合：`t = 0` 全取 `a`，`t = 1` 全取 `b`。
pub fn blend_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let mix = |x: u8, y: u8| {
        (f32::from(x) + (f32::from(y) - f32::from(x)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

/// 把主题库的 10 个语义色展开成项目在用的 24 个槽位。
///
/// 项目现有 294 处调用点用的是 Catppuccin 的槽位名（surface/overlay/peach…）。
/// 与其把它们全部改写成语义名（高风险大重构），不如在这里做一次映射：
/// 层次色由 `bg → fg` 的插值生成，色相槽位折叠到主题库的
/// `error/warning/success/info/accent/secondary` 上。
///
/// 未知槽位返回 `None`，由调用方回退到配置里的十六进制色。
pub fn slot_color(palette: ThemePalette, slot: &str) -> Option<Rgb> {
    // 主题库的颜色是 ratatui 的 Color；项目只接受 RGB，非 RGB 变体（理论上
    // 不会出现）回落到背景色，保证渲染路径永远拿到确定值。
    let rgb = |color: Color| match color {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    };
    let bg = rgb(palette.bg);
    let fg = rgb(palette.fg);
    // 层次色一律朝前景色方向插值：深色主题是"提亮"，浅色主题是"压暗"，
    // 两边都能得到正确的分层方向。
    let layer = |t: f32| blend_rgb(bg, fg, t);
    Some(match slot {
        "base" => bg,
        "mantle" => layer(0.04),
        "crust" => layer(0.08),
        "surface_0" => layer(0.10),
        "surface_1" => layer(0.16),
        "surface_2" => layer(0.24),
        "overlay_0" => blend_rgb(rgb(palette.muted), bg, 0.35),
        "overlay_1" => rgb(palette.muted),
        "overlay_2" => blend_rgb(rgb(palette.muted), fg, 0.35),
        "text" => fg,
        "subtext_0" => blend_rgb(fg, bg, 0.30),
        "subtext_1" => blend_rgb(fg, bg, 0.18),
        "accent" => rgb(palette.accent),
        "border" => layer(0.28),
        // 语义色直接对应
        "yellow" => rgb(palette.warning),
        "red" => rgb(palette.error),
        "green" => rgb(palette.success),
        "blue" => rgb(palette.info),
        // 主题库没有这么多色相：次要槽位折叠到最近的语义色上，保持"颜色含义"不丢
        "peach" => rgb(palette.secondary),
        "mauve" | "lavender" | "rosewater" | "flamingo" | "pink" => rgb(palette.accent),
        "sapphire" | "sky" => rgb(palette.info),
        "teal" => rgb(palette.success),
        "maroon" => rgb(palette.error),
        // 选中行上的文字：用主题背景色，在强调色底上一定读得清
        "selection_fg" => bg,
        _ => return None,
    })
}

const ROSEWATER: Rgb = [245, 224, 220];
const FLAMINGO: Rgb = [242, 205, 205];
const PINK: Rgb = [245, 194, 231];
const MAUVE: Rgb = [203, 166, 247];
const RED: Rgb = [243, 139, 168];
const MAROON: Rgb = [235, 160, 172];
const PEACH: Rgb = [250, 179, 135];
const YELLOW: Rgb = [249, 226, 175];
const GREEN: Rgb = [166, 227, 161];
const TEAL: Rgb = [148, 226, 213];
const SKY: Rgb = [137, 220, 235];
const SAPPHIRE: Rgb = [116, 199, 236];
const BLUE: Rgb = [137, 180, 250];
const LAVENDER: Rgb = [180, 190, 254];
const TEXT: Rgb = [205, 214, 244];
const SUBTEXT_1: Rgb = [186, 194, 222];
const SUBTEXT_0: Rgb = [166, 173, 200];
const OVERLAY_2: Rgb = [147, 153, 178];
const OVERLAY_1: Rgb = [127, 132, 156];
const OVERLAY_0: Rgb = [108, 112, 134];
const SURFACE_2: Rgb = [88, 91, 112];
const SURFACE_1: Rgb = [69, 71, 90];
const SURFACE_0: Rgb = [49, 50, 68];
const BASE: Rgb = [30, 30, 46];
const MANTLE: Rgb = [24, 24, 37];
const CRUST: Rgb = [17, 17, 27];

/// 取一套主题的全部渲染色（一次读配置）。
///
/// GUI 与 TUI 都从这里取色；`name` 决定是否套用主题库的具名主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub rosewater: Rgb,
    pub flamingo: Rgb,
    pub pink: Rgb,
    pub mauve: Rgb,
    pub red: Rgb,
    pub maroon: Rgb,
    pub peach: Rgb,
    pub yellow: Rgb,
    pub green: Rgb,
    pub teal: Rgb,
    pub sky: Rgb,
    pub sapphire: Rgb,
    pub blue: Rgb,
    pub lavender: Rgb,
    pub text: Rgb,
    pub subtext1: Rgb,
    pub subtext0: Rgb,
    pub overlay2: Rgb,
    pub overlay1: Rgb,
    pub overlay0: Rgb,
    pub surface2: Rgb,
    pub surface1: Rgb,
    pub surface0: Rgb,
    pub base: Rgb,
    pub mantle: Rgb,
    pub crust: Rgb,
    pub accent: Rgb,
    pub border: Rgb,
    pub muted: Rgb,
    pub selection_fg: Rgb,
}

impl Palette {
    /// 按配置解析：`theme.name` 命中主题库时整体换肤，否则用手工槽位。
    pub fn from_config(config: &ThemeConfig) -> Self {
        let slot = |name: &str, value: &String, fallback: Rgb| -> Rgb {
            if let Some(theme) = library_theme(&config.name)
                && let Some(color) = slot_color(theme.palette(), name)
            {
                return color;
            }
            parse(value, fallback)
        };
        Self {
            rosewater: slot("rosewater", &config.rosewater, ROSEWATER),
            flamingo: slot("flamingo", &config.flamingo, FLAMINGO),
            pink: slot("pink", &config.pink, PINK),
            mauve: slot("mauve", &config.mauve, MAUVE),
            red: slot("red", &config.red, RED),
            maroon: slot("maroon", &config.maroon, MAROON),
            peach: slot("peach", &config.peach, PEACH),
            yellow: slot("yellow", &config.yellow, YELLOW),
            green: slot("green", &config.green, GREEN),
            teal: slot("teal", &config.teal, TEAL),
            sky: slot("sky", &config.sky, SKY),
            sapphire: slot("sapphire", &config.sapphire, SAPPHIRE),
            blue: slot("blue", &config.blue, BLUE),
            lavender: slot("lavender", &config.lavender, LAVENDER),
            text: slot("text", &config.text, TEXT),
            subtext1: slot("subtext_1", &config.subtext_1, SUBTEXT_1),
            subtext0: slot("subtext_0", &config.subtext_0, SUBTEXT_0),
            overlay2: slot("overlay_2", &config.overlay_2, OVERLAY_2),
            overlay1: slot("overlay_1", &config.overlay_1, OVERLAY_1),
            overlay0: slot("overlay_0", &config.overlay_0, OVERLAY_0),
            surface2: slot("surface_2", &config.surface_2, SURFACE_2),
            surface1: slot("surface_1", &config.surface_1, SURFACE_1),
            surface0: slot("surface_0", &config.surface_0, SURFACE_0),
            base: slot("base", &config.base, BASE),
            mantle: slot("mantle", &config.mantle, MANTLE),
            crust: slot("crust", &config.crust, CRUST),
            accent: slot("accent", &config.accent, MAUVE),
            border: slot("border", &config.border, SURFACE_2),
            muted: slot("muted", &config.muted, SUBTEXT_0),
            selection_fg: slot("selection_fg", &config.crust, CRUST),
        }
    }
}

/// `[theme].name` 在主题库里的映射方式。
///
/// 主题库的名字带明暗后缀（`catppuccin-mocha` / `catppuccin-latte`、
/// `dracula` / `dracula-light`…）。桌面 GUI 跟随系统深浅色时，只换后缀、
/// 保留同一套色相，用户选的主题仍然被尊重。
pub fn resolve_theme_name(theme: &ThemeConfig, dark: bool) -> String {
    let current = match library_theme(&theme.name) {
        Some(theme) => theme,
        // `voicefox` 或未知名字：用主题库的默认深/浅主题兜底。
        None => {
            return if dark {
                SKIN_VOICEFOX.to_string()
            } else {
                "catppuccin-latte".to_string()
            };
        }
    };
    let wanted = |hint: &str| -> Option<ThemeName> {
        ThemeName::all()
            .iter()
            .copied()
            .find(|name| skin_key(*name).contains(hint))
    };
    let current_key = skin_key(current);
    let is_light = current_key.contains("latte") || current_key.contains("light");
    if is_light == !dark {
        return current_key;
    }
    // 反向后缀优先；否则换到任意同明暗的主题，保证不会给出对不上的明暗。
    let hints: [&str; 2] = if dark {
        ["mocha", "dark"]
    } else {
        ["latte", "light"]
    };
    hints
        .iter()
        .find_map(|hint| wanted(hint))
        .map(skin_key)
        .unwrap_or(current_key)
}

/// 按明暗取色：具名主题换到同色相的对应变体，`voicefox` 槽位在浅色时反转
/// 层次与前景色（配置里的槽位按深色调的，浅色下原样用会读不清）。
pub fn palette_for_config(theme: &ThemeConfig, dark: bool) -> Palette {
    if dark {
        return Palette::from_config(theme);
    }
    let resolved = ThemeConfig {
        name: resolve_theme_name(theme, false),
        ..theme.clone()
    };
    if library_theme(&resolved.name).is_some() {
        return Palette::from_config(&resolved);
    }
    // `voicefox` 槽位：背景/层次色换成浅色，正文与辅助色取深色版的反向槽位。
    let mut light = theme.clone();
    let swap = |light: &mut ThemeConfig, theme: &ThemeConfig| {
        light.base = rgb_hex(parse(&theme.text, TEXT));
        light.mantle = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.06,
        ));
        light.crust = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.12,
        ));
        light.surface_0 = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.10,
        ));
        light.surface_1 = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.16,
        ));
        light.surface_2 = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.24,
        ));
        light.text = rgb_hex(parse(&theme.base, BASE));
        light.subtext_1 = rgb_hex(parse(&theme.subtext_1, SUBTEXT_1));
        light.subtext_0 = rgb_hex(parse(&theme.subtext_0, SUBTEXT_0));
        light.muted = rgb_hex(blend_rgb(
            parse(&theme.base, BASE),
            parse(&theme.muted, SUBTEXT_0),
            0.55,
        ));
        light.border = rgb_hex(blend_rgb(
            parse(&theme.text, TEXT),
            parse(&theme.base, BASE),
            0.28,
        ));
    };
    swap(&mut light, theme);
    Palette::from_config(&light)
}

fn rgb_hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// 解析一个槽位的颜色（不含封面混合）。
pub fn slot_from_config(config: &ThemeConfig, slot: &str) -> Option<Rgb> {
    let value = match slot {
        "accent" => &config.accent,
        "border" => &config.border,
        "text" => &config.text,
        "muted" => &config.muted,
        _ => return slot_color(library_theme(&config.name)?.palette(), slot),
    };
    Some(parse(value, MAUVE))
}

fn parse(value: &str, fallback: Rgb) -> Rgb {
    parse_value(value).unwrap_or(fallback)
}

/// 解析一个主题色值；无法识别时返回 `None`。
///
/// 支持四种写法：
///
/// - **跟随终端**：`default` / `reset` / `terminal` / `none` / `transparent` / `-`
///   都视为"跟随终端"，RGB 视图里回落到调用方给的 fallback。
/// - 16 个 ANSI 名字：`black`…`white`，另接受 `light_*` / `bright_*` 与
///   `dark_gray` 的几种写法。
/// - `#rgb` 与 `#rrggbb`。
/// - ANSI 256 色号：`236` 或 `color236`（引用终端调色板）。
pub fn parse_value(value: &str) -> Option<Rgb> {
    match parse_color(value) {
        ParsedColor::Rgb(rgb) => Some(rgb),
        _ => None,
    }
}

/// 值是"没写 / 无法识别"，还是"明确要求跟随终端"。
fn parsed_ok(value: &str) -> bool {
    !matches!(parse_color(value), ParsedColor::Invalid)
}

enum ParsedColor {
    Rgb(Rgb),
    TerminalDefault,
    Invalid,
}

fn parse_color(value: &str) -> ParsedColor {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return ParsedColor::Invalid;
    }
    if matches!(
        value.as_str(),
        "default" | "reset" | "terminal" | "none" | "transparent" | "-"
    ) {
        return ParsedColor::TerminalDefault;
    }
    let ansi = match value.as_str() {
        "black" => Some(0),
        "red" | "dark_red" => Some(1),
        "green" | "dark_green" => Some(2),
        "yellow" | "dark_yellow" => Some(3),
        "blue" | "dark_blue" => Some(4),
        "magenta" | "dark_magenta" => Some(5),
        "cyan" | "dark_cyan" => Some(6),
        "white" | "light_gray" | "gray" | "grey" => Some(7),
        "dark_gray" | "dark_grey" => Some(8),
        "light_red" | "bright_red" => Some(9),
        "light_green" | "bright_green" => Some(10),
        "light_yellow" | "bright_yellow" => Some(11),
        "light_blue" | "bright_blue" => Some(12),
        "light_magenta" | "bright_magenta" => Some(13),
        "light_cyan" | "bright_cyan" => Some(14),
        "light_white" | "bright_white" => Some(15),
        _ => None,
    };
    if let Some(index) = ansi {
        return ParsedColor::Rgb(ansi_rgb(index));
    }
    if let Some(rest) = value.strip_prefix('#')
        && let Some(rgb) = parse_hex(rest)
    {
        return ParsedColor::Rgb(rgb);
    }
    let digits = value.strip_prefix("color").unwrap_or(&value);
    if !digits.is_empty() && digits.len() <= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
        return match digits.parse::<u8>() {
            Ok(index) => ParsedColor::Rgb(xterm_rgb(index)),
            Err(_) => ParsedColor::Invalid,
        };
    }
    ParsedColor::Invalid
}

/// 16 个基础 ANSI 色的 RGB 近似值（终端实际配色由用户主题决定）。
fn ansi_rgb(index: u8) -> Rgb {
    const BASE: [Rgb; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    BASE[usize::from(index) % 16]
}

/// xterm 256 色号 → RGB。
fn xterm_rgb(index: u8) -> Rgb {
    if index < 16 {
        return ansi_rgb(index);
    }
    if index < 232 {
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let i = usize::from(index - 16);
        return [LEVELS[i / 36 % 6], LEVELS[i / 6 % 6], LEVELS[i % 6]];
    }
    let value = 8 + (index - 232) * 10;
    [value, value, value]
}

fn parse_hex(rest: &str) -> Option<Rgb> {
    if rest.is_empty() || !rest.is_ascii() {
        // 非 ASCII（例如误把汉字写进色值）直接判为无法识别，绝不能按字节切
        // 片——那会切在字符中间并 panic，而主题色每帧都解析。
        return None;
    }
    match rest.len() {
        3 => {
            let bytes = rest.as_bytes();
            Some([
                hex_digit(bytes[0])? * 17,
                hex_digit(bytes[1])? * 17,
                hex_digit(bytes[2])? * 17,
            ])
        }
        6 => {
            let number = u32::from_str_radix(rest, 16).ok()?;
            Some([(number >> 16) as u8, (number >> 8) as u8, number as u8])
        }
        _ => None,
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// 在配置加载后调用一次：把识别不了的主题色值报出来。
///
/// `parse_value` 认不出来时会静默回退到默认色，用户会以为"改了配置却没生效"，
/// 所以在加载阶段集中提示一次（渲染路径每帧都会解析颜色，不能在那里打日志）。
pub fn warn_unrecognized(theme: &ThemeConfig) {
    let fields: [(&str, &String); 29] = [
        ("accent", &theme.accent),
        ("text", &theme.text),
        ("muted", &theme.muted),
        ("border", &theme.border),
        ("rosewater", &theme.rosewater),
        ("flamingo", &theme.flamingo),
        ("pink", &theme.pink),
        ("mauve", &theme.mauve),
        ("red", &theme.red),
        ("maroon", &theme.maroon),
        ("peach", &theme.peach),
        ("yellow", &theme.yellow),
        ("green", &theme.green),
        ("teal", &theme.teal),
        ("sky", &theme.sky),
        ("sapphire", &theme.sapphire),
        ("blue", &theme.blue),
        ("lavender", &theme.lavender),
        ("subtext_1", &theme.subtext_1),
        ("subtext_0", &theme.subtext_0),
        ("overlay_2", &theme.overlay_2),
        ("overlay_1", &theme.overlay_1),
        ("overlay_0", &theme.overlay_0),
        ("surface_2", &theme.surface_2),
        ("surface_1", &theme.surface_1),
        ("surface_0", &theme.surface_0),
        ("base", &theme.base),
        ("mantle", &theme.mantle),
        ("crust", &theme.crust),
    ];
    for (name, value) in fields {
        if !value.trim().is_empty() && !parsed_ok(value) {
            tracing::warn!(
                "主题色 theme.{name} = {value:?} 无法识别，已回退到默认色；\
                 可用 #rgb / #rrggbb、ANSI 名字（red/light_blue/…）、256 色号（236 / color236），\
                 或 default（跟随终端）"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SKIN_VOICEFOX;
    use super::{
        Rgb, blend_rgb, library_theme, next_skin_name, skin_label, skin_names, slot_color,
    };
    use super::{parse, parse_value};
    use lx_core::model::config::ThemeConfig;
    use ratatui::style::Color;
    use ratatui_themes::ThemeName;

    #[test]
    fn parses_hex_color() {
        assert_eq!(parse("#12aBcD", [1, 2, 3]), [0x12, 0xab, 0xcd]);
    }

    #[test]
    fn non_ascii_hex_falls_back_instead_of_panicking() {
        // 修复前："#abc晴" 字节长度正好是 7，会走进 &value[3..5] 这类汉字中间的
        // 切片并在非字符边界 panic；主题色每帧都解析，等于一启动就崩。
        assert_eq!(parse("#abc晴", super::RED), super::RED);
        assert_eq!(parse("#日日", super::RED), super::RED);
        // 位数不对的也一律回退。
        assert_eq!(parse("#12345", super::RED), super::RED);
        assert_eq!(parse("#1234567", super::RED), super::RED);
    }

    #[test]
    fn three_digit_hex_is_expanded() {
        assert_eq!(parse_value("#abc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_value("#F0A"), Some([0xff, 0x00, 0xaa]));
    }

    #[test]
    fn default_tokens_follow_the_terminal() {
        for token in [
            "default",
            "reset",
            "terminal",
            "none",
            "transparent",
            "-",
            "  DEFAULT  ",
        ] {
            // RGB 视图里没有"跟随终端"，回落到调用方的 fallback。
            assert_eq!(parse_value(token), None, "{token}");
            assert!(
                super::parsed_ok(token),
                "{} 是合法写法，不应报无法识别",
                token
            );
        }
    }

    #[test]
    fn ansi_names_and_256_indexes_are_supported() {
        assert_eq!(parse_value("light_blue"), Some(super::ansi_rgb(12)));
        assert_eq!(parse_value("bright_blue"), Some(super::ansi_rgb(12)));
        assert_eq!(parse_value("dark_gray"), Some(super::ansi_rgb(8)));
        assert_eq!(parse_value("236"), Some([48, 48, 48]));
        assert_eq!(parse_value("color236"), Some([48, 48, 48]));
        assert_eq!(parse_value("255"), Some([238, 238, 238]));
    }

    #[test]
    fn empty_and_unknown_values_are_reported_as_none() {
        assert_eq!(parse_value(""), None);
        assert_eq!(parse_value("   "), None);
        assert_eq!(parse_value("greyish"), None);
        assert_eq!(parse_value("color999"), None);
        assert_eq!(parse_value("300"), None);
        assert!(!super::parsed_ok("greyish"));
    }

    #[test]
    fn base_can_follow_the_terminal() {
        // 方案 A：默认配色不变，但 base 显式设成 default 时 RGB 视图回落到 fallback。
        let theme = ThemeConfig {
            base: "default".to_string(),
            ..ThemeConfig::default()
        };
        assert_eq!(parse(&theme.base, super::BASE), super::BASE);
        // 没配的字段仍然用 Mocha 默认值。
        assert_eq!(theme.accent, "#cba6f7");
    }

    #[test]
    fn palette_reads_hand_tuned_and_library_themes() {
        let default = super::Palette::from_config(&ThemeConfig::default());
        assert_eq!(default.base, super::BASE);
        assert_eq!(default.text, super::TEXT);
        assert_eq!(default.accent, super::MAUVE);

        let nord = ThemeConfig {
            name: "nord".to_string(),
            ..ThemeConfig::default()
        };
        let palette = super::Palette::from_config(&nord);
        assert_eq!(
            palette.base,
            super::slot_color(ThemeName::Nord.palette(), "base").unwrap()
        );
        assert_eq!(
            palette.text,
            super::slot_color(ThemeName::Nord.palette(), "text").unwrap()
        );
    }

    // ── 多主题：槽位映射 ──

    const ALL_SLOTS: [&str; 25] = [
        "base",
        "mantle",
        "crust",
        "surface_0",
        "surface_1",
        "surface_2",
        "overlay_0",
        "overlay_1",
        "overlay_2",
        "text",
        "subtext_0",
        "subtext_1",
        "accent",
        "border",
        "yellow",
        "red",
        "green",
        "blue",
        "peach",
        "mauve",
        "lavender",
        "sapphire",
        "sky",
        "teal",
        "selection_fg",
    ];

    #[test]
    fn blend_hits_both_ends_and_the_middle() {
        let black: Rgb = [0, 0, 0];
        let white: Rgb = [255, 255, 255];
        assert_eq!(blend_rgb(black, white, 0.0), black);
        assert_eq!(blend_rgb(black, white, 1.0), white);
        assert_eq!(blend_rgb(black, white, 0.5), [128, 128, 128]);
    }

    /// 每个在用的槽位都要能从主题库推导出来，未知槽位返回 None 由调用方回退。
    #[test]
    fn every_slot_used_by_the_ui_maps_to_a_library_theme() {
        let palette = ThemeName::Dracula.palette();
        for slot in ALL_SLOTS {
            assert!(
                slot_color(palette, slot).is_some(),
                "槽位 {slot} 必须有映射，否则该处会退回过时的十六进制默认色"
            );
        }
        assert_eq!(slot_color(palette, "not_a_slot"), None);
    }

    /// 层次色必须按"离背景越来越远"排序，深色与浅色主题都要成立。
    #[test]
    fn surface_layers_are_ordered_for_dark_and_light_themes() {
        let luminance =
            |color: Rgb| u32::from(color[0]) + u32::from(color[1]) + u32::from(color[2]);
        for name in [ThemeName::Dracula, ThemeName::CatppuccinLatte] {
            let palette = name.palette();
            let bg = luminance(super::slot_color(palette, "base").unwrap());
            let fg = luminance(super::slot_color(palette, "text").unwrap());
            let surfaces = ["surface_0", "surface_1", "surface_2"]
                .map(|slot| luminance(slot_color(palette, slot).unwrap()));
            if fg >= bg {
                assert!(
                    surfaces[0] < surfaces[1] && surfaces[1] < surfaces[2],
                    "{name:?}"
                );
            } else {
                assert!(
                    surfaces[0] > surfaces[1] && surfaces[1] > surfaces[2],
                    "{name:?}"
                );
            }
            let (low, high) = (bg.min(fg), bg.max(fg));
            for value in surfaces {
                assert!(value >= low && value <= high, "{name:?}: {value} 越界");
            }
        }
    }

    /// 语义色必须真的对得上：错误红、成功绿、警告黄。
    #[test]
    fn semantic_slots_take_the_matching_semantic_color() {
        let rgb = |color: Color| match color {
            Color::Rgb(r, g, b) => [r, g, b],
            _ => panic!("主题库颜色应为 RGB"),
        };
        let palette = ThemeName::Nord.palette();
        assert_eq!(slot_color(palette, "red").unwrap(), rgb(palette.error));
        assert_eq!(slot_color(palette, "green").unwrap(), rgb(palette.success));
        assert_eq!(slot_color(palette, "yellow").unwrap(), rgb(palette.warning));
        assert_eq!(slot_color(palette, "blue").unwrap(), rgb(palette.info));
        assert_eq!(
            slot_color(palette, "selection_fg").unwrap(),
            rgb(palette.bg)
        );
    }

    /// 浅色主题也要可用：文字比背景暗。
    #[test]
    fn light_themes_stay_readable() {
        let palette = ThemeName::CatppuccinLatte.palette();
        assert!(palette.is_light(), "这个主题应当是浅色的");
        let text = slot_color(palette, "text").unwrap();
        let rgb = |color: Color| match color {
            Color::Rgb(r, g, b) => [r, g, b],
            _ => panic!("主题库颜色应为 RGB"),
        };
        assert_eq!(text, rgb(palette.fg));
        assert_ne!(text, rgb(palette.bg), "文字不能和背景同色");
    }

    #[test]
    fn voicefox_is_the_default_and_keeps_the_hand_tuned_slots() {
        assert_eq!(library_theme(SKIN_VOICEFOX), None);
        assert_eq!(library_theme(""), None);
        assert_eq!(library_theme("不存在的主题"), None);
        assert_eq!(library_theme(" Tokyo-Night "), Some(ThemeName::TokyoNight));
    }

    #[test]
    fn skin_names_cover_voicefox_plus_the_library_and_cycles() {
        let names = skin_names();
        assert_eq!(names.first().map(String::as_str), Some(SKIN_VOICEFOX));
        assert_eq!(names.len(), ThemeName::all().len() + 1);
        let mut current = SKIN_VOICEFOX.to_string();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..names.len() {
            assert!(seen.insert(current.clone()), "循环里不能重复: {current}");
            current = next_skin_name(&current);
        }
        assert_eq!(current, SKIN_VOICEFOX);
        assert_eq!(skin_label(SKIN_VOICEFOX), "Voicefox");
        assert!(!skin_label("nord").is_empty());
    }
}
