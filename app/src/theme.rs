//! TUI 侧的主题取色 shim。
//!
//! 配置解析与槽位映射已经挪到 `voicefox_runtime::ui::theme`，与桌面 GUI
//! 共用一份颜色来源；这里只保留 TUI 专有的两件事：
//! 1. 把 [0–255] 的 RGB 包成 ratatui 的颜色类型（也支持"跟随终端"）；
//! 2. 「界面强调色跟随封面」——封面主色提取依赖 TUI 的封面管线。

#![allow(dead_code)]

use ratatui::style::Color;
use ratatui_themes::ThemeName;

pub use voicefox_runtime::ui::theme::{
    SKIN_VOICEFOX, blend_rgb, next_skin_name, parse_value, skin_label, skin_names,
};

use voicefox_runtime::ui::theme;

use crate::context::AppContext;
use lx_core::model::config::AccentFollowCover;

/// 配置里的颜色槽位 → ratatui 颜色。
pub type Rgb = voicefox_runtime::ui::theme::Rgb;

fn to_color(rgb: Rgb) -> Color {
    Color::Rgb(rgb[0], rgb[1], rgb[2])
}

/// 各档位把封面主色混入 accent 的比例：过高会失去可读性与主题个性，
/// 过低则感知不到（提取色已归一化到鲜艳区间，见 cover::accent）。
const ACCENT_COVER_BLEND_SUBTLE: f32 = 0.45;
const ACCENT_COVER_BLEND_STRONG: f32 = 0.75;

pub fn accent(ctx: &AppContext) -> Color {
    let base = to_color(configured(ctx, "accent", |theme| &theme.accent));
    blend_with_cover(ctx, base)
}

/// 「界面强调色跟随封面」：把当前专辑封面提取出的主色混进 accent。
///
/// 只动 accent 一个槽位（`blend_with_cover` 的调用点仅此一处），层次色与
/// 语义色保持主题原样；档位关闭 / 无封面 / 封面是灰调（无主色）时原样返回。
fn blend_with_cover(ctx: &AppContext, base: Color) -> Color {
    let mode = ctx
        .config
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .ui
        .accent_follow_cover;
    let blend_t = match mode {
        AccentFollowCover::Off => return base,
        AccentFollowCover::Subtle => ACCENT_COVER_BLEND_SUBTLE,
        AccentFollowCover::Strong => ACCENT_COVER_BLEND_STRONG,
    };
    let Some(path) = ctx.cover_service.image_path() else {
        return base;
    };
    let Some(rgb) = crate::cover::accent::current(Some(&path)) else {
        return base;
    };
    blend(base, Color::Rgb(rgb[0], rgb[1], rgb[2]), blend_t)
}

/// 两个颜色按比例混合：`t = 0` 全取 `a`，`t = 1` 全取 `b`。
///
/// 非 RGB 颜色（终端默认色 / ANSI 名）无法参与混合，原样返回 `a`。
pub(crate) fn blend(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let mixed = blend_rgb([ar, ag, ab], [br, bg, bb], t);
            to_color(mixed)
        }
        _ => a,
    }
}

pub fn border(ctx: &AppContext) -> Color {
    to_color(configured(ctx, "border", |theme| &theme.border))
}

pub fn text(ctx: &AppContext) -> Color {
    to_color(configured(ctx, "text", |theme| &theme.text))
}

pub fn muted(ctx: &AppContext) -> Color {
    to_color(configured(ctx, "muted", |theme| &theme.muted))
}

macro_rules! palette_color {
    ($name:ident, $field:ident) => {
        pub fn $name(ctx: &AppContext) -> Color {
            to_color(configured(ctx, stringify!($field), |theme| &theme.$field))
        }
    };
}

palette_color!(rosewater, rosewater);
palette_color!(flamingo, flamingo);
palette_color!(pink, pink);
palette_color!(mauve, mauve);
palette_color!(red, red);
palette_color!(maroon, maroon);
palette_color!(peach, peach);
palette_color!(yellow, yellow);
palette_color!(green, green);
palette_color!(teal, teal);
palette_color!(sky, sky);
palette_color!(sapphire, sapphire);
palette_color!(blue, blue);
palette_color!(lavender, lavender);
palette_color!(subtext1, subtext_1);
palette_color!(subtext0, subtext_0);
palette_color!(overlay2, overlay_2);
palette_color!(overlay1, overlay_1);
palette_color!(overlay0, overlay_0);
palette_color!(surface2, surface_2);
palette_color!(surface1, surface_1);
palette_color!(surface0, surface_0);
palette_color!(base, base);
palette_color!(mantle, mantle);
palette_color!(crust, crust);

pub fn selection_fg(ctx: &AppContext) -> Color {
    to_color(configured(ctx, "selection_fg", |theme| &theme.crust))
}

/// 取一个槽位的颜色（TUI 版）。
///
/// 两条来源，优先级从高到低：
/// 1. `theme.name` 选了主题库里的具名主题 → 由它的语义色推导（整界面一起换肤）；
/// 2. 否则（默认 `voicefox`）→ 用 `[theme]` 里手工调好的十六进制色，观感与历史一致。
///
/// `default` 等"跟随终端"写法映射到 [`Color::Reset`]。
fn configured(
    ctx: &AppContext,
    slot: &str,
    value: fn(&lx_core::model::config::ThemeConfig) -> &String,
) -> Rgb {
    let config = ctx.config.read().unwrap_or_else(|e| e.into_inner());
    if let Some(theme) = library_theme(&config.theme.name)
        && let Some(color) = theme::slot_color(theme.palette(), slot)
    {
        return color;
    }
    parse_value(value(&config.theme))
        // "跟随终端"与"无法识别"在 RGB 视图里都回落到默认槽位：
        // 调用方随后不会区分，这里统一给 Mocha 默认值。
        .or_else(|| theme::parse_value(&default_slot(slot)))
        .unwrap_or([0, 0, 0])
}

/// 每个槽位在 `voicefox` 皮肤下的默认色（与历史版本的观感一致）。
fn default_slot(slot: &str) -> String {
    let defaults = lx_core::model::config::ThemeConfig::default();
    match slot {
        "accent" => defaults.accent,
        "border" => defaults.border,
        "text" => defaults.text,
        "muted" => defaults.muted,
        "rosewater" => defaults.rosewater,
        "flamingo" => defaults.flamingo,
        "pink" => defaults.pink,
        "mauve" => defaults.mauve,
        "red" => defaults.red,
        "maroon" => defaults.maroon,
        "peach" => defaults.peach,
        "yellow" => defaults.yellow,
        "green" => defaults.green,
        "teal" => defaults.teal,
        "sky" => defaults.sky,
        "sapphire" => defaults.sapphire,
        "blue" => defaults.blue,
        "lavender" => defaults.lavender,
        "subtext_1" => defaults.subtext_1,
        "subtext_0" => defaults.subtext_0,
        "overlay_2" => defaults.overlay_2,
        "overlay_1" => defaults.overlay_1,
        "overlay_0" => defaults.overlay_0,
        "surface_2" => defaults.surface_2,
        "surface_1" => defaults.surface_1,
        "surface_0" => defaults.surface_0,
        "base" => defaults.base,
        "mantle" => defaults.mantle,
        "crust" => defaults.crust,
        _ => defaults.mauve,
    }
}

fn library_theme(value: &str) -> Option<ThemeName> {
    // 复用 runtime 的解析：名字大小写/空格不敏感。
    skin_names()
        .into_iter()
        .find(|name| name != SKIN_VOICEFOX && name.eq_ignore_ascii_case(value.trim()))
        .and_then(|name| serde_json::from_value::<ThemeName>(serde_json::Value::String(name)).ok())
}

pub fn warn_unrecognized(theme_config: &lx_core::model::config::ThemeConfig) {
    theme::warn_unrecognized(theme_config);
}

#[cfg(test)]
mod tests {
    use super::SKIN_VOICEFOX;
    use super::{blend, library_theme, next_skin_name, parse_value, skin_label, skin_names};
    use ratatui::style::Color;

    #[test]
    fn parses_hex_color() {
        assert_eq!(parse_value("#12aBcD"), Some([0x12, 0xab, 0xcd]));
    }

    #[test]
    fn non_ascii_hex_falls_back_instead_of_panicking() {
        assert_eq!(parse_value("#abc晴"), None);
        assert_eq!(parse_value("#日日"), None);
        assert_eq!(parse_value("#12345"), None);
        assert_eq!(parse_value("#1234567"), None);
    }

    #[test]
    fn blend_keeps_non_rgb_colors_unchanged() {
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(blend(Color::Reset, white, 0.5), Color::Reset);
        assert_eq!(blend(Color::Red, white, 0.5), Color::Red);
        assert_eq!(
            blend(Color::Rgb(0, 0, 0), white, 1.0),
            Color::Rgb(255, 255, 255)
        );
    }

    #[test]
    fn skin_cycle_returns_to_voicefox() {
        let names = skin_names();
        let mut current = SKIN_VOICEFOX.to_string();
        for _ in 0..names.len() {
            current = next_skin_name(&current);
        }
        assert_eq!(current, SKIN_VOICEFOX);
        assert_eq!(skin_label(SKIN_VOICEFOX), "Voicefox");
        assert!(library_theme(" Tokyo-Night ").is_some());
    }
}
