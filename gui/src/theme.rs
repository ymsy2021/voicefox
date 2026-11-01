//! GUI 配色：把 `voicefox_runtime::ui::theme` 的调色板映射成 egui 视觉风格。
//!
//! 与 TUI 共用同一份配置来源（`[theme]` 槽位或主题库具名主题），
//! 用户在配置文件里换肤，两套前端一起变色。

use eframe::egui::{Color32, CornerRadius, Stroke, Visuals};
use lx_core::model::config::Config;
use voicefox_runtime::ui::theme::Palette;

fn color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// 一套 GUI 用色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiTheme {
    pub base: Color32,
    pub mantle: Color32,
    pub crust: Color32,
    pub surface0: Color32,
    pub surface1: Color32,
    pub surface2: Color32,
    pub text: Color32,
    pub subtext: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub border: Color32,
    pub red: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub selection_fg: Color32,
}

impl GuiTheme {
    pub fn from_config(config: &Config) -> Self {
        let palette = Palette::from_config(&config.theme);
        Self::from_palette(&palette)
    }

    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            base: color(palette.base),
            mantle: color(palette.mantle),
            crust: color(palette.crust),
            surface0: color(palette.surface0),
            surface1: color(palette.surface1),
            surface2: color(palette.surface2),
            text: color(palette.text),
            subtext: color(palette.subtext0),
            muted: color(palette.muted),
            accent: color(palette.accent),
            border: color(palette.border),
            red: color(palette.red),
            green: color(palette.green),
            yellow: color(palette.yellow),
            selection_fg: color(palette.selection_fg),
        }
    }

    /// 应用到 egui 的深色视觉风格。
    pub fn visuals(self) -> Visuals {
        let mut visuals = Visuals::dark();
        visuals.panel_fill = self.base;
        visuals.window_fill = self.base;
        visuals.extreme_bg_color = self.mantle;
        visuals.faint_bg_color = self.mantle;
        visuals.code_bg_color = self.surface0;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.window_corner_radius = CornerRadius::same(10);
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.selection.bg_fill = self.accent.gamma_multiply(0.35);
        visuals.selection.stroke = Stroke::new(1.0, self.accent);
        visuals.hyperlink_color = self.accent;
        visuals.override_text_color = Some(self.text);

        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.mantle;
        widgets.noninteractive.weak_bg_fill = self.mantle;
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.subtext);
        widgets.inactive.bg_fill = self.surface0;
        widgets.inactive.weak_bg_fill = self.surface0;
        widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        widgets.inactive.fg_stroke = Stroke::new(1.0, self.text);
        widgets.hovered.bg_fill = self.surface1;
        widgets.hovered.weak_bg_fill = self.surface1;
        widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent.gamma_multiply(0.7));
        widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        widgets.active.bg_fill = self.surface2;
        widgets.active.weak_bg_fill = self.surface2;
        widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        widgets.open.bg_fill = self.surface1;
        widgets.open.weak_bg_fill = self.surface1;
        widgets.open.bg_stroke = Stroke::new(1.0, self.border);
        widgets.open.fg_stroke = Stroke::new(1.0, self.text);
        visuals
    }
}

#[cfg(test)]
mod tests {
    use super::GuiTheme;
    use lx_core::model::config::Config;

    #[test]
    fn default_config_maps_to_mocha_colors() {
        let theme = GuiTheme::from_config(&Config::default());
        assert_eq!(theme.base.to_array(), [30, 30, 46, 255]);
        assert_eq!(theme.text.to_array(), [205, 214, 244, 255]);
        assert_eq!(theme.accent.to_array(), [203, 166, 247, 255]);
    }

    #[test]
    fn named_theme_changes_the_palette() {
        let nord = Config {
            theme: lx_core::model::config::ThemeConfig {
                name: "nord".into(),
                ..Default::default()
            },
            ..Config::default()
        };
        let theme = GuiTheme::from_config(&nord);
        assert_ne!(theme.base.to_array(), [30, 30, 46, 255]);
    }

    #[test]
    fn visuals_use_theme_background() {
        let theme = GuiTheme::from_config(&Config::default());
        let visuals = theme.visuals();
        assert_eq!(visuals.panel_fill, theme.base);
        assert_eq!(visuals.override_text_color, Some(theme.text));
    }
}
