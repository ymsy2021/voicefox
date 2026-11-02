//! 设置页：全部读写已有的 `Config` 字段（没有 GUI 专属配置）。
//!
//! 改动一律交给服务线程写盘（`ConfigChange`），UI 只发请求；下一次快照
//! 回来后界面才显示新值，回滚是自动的（非法的键位字符串根本不会落盘）。

use eframe::egui::{self, Align, Layout, RichText};

use super::UiState;
use crate::VoicefoxGuiController;
use crate::service::{GUI_ACTIONS, GuiRequest, QUALITY_OPTIONS, SettingsView, Snapshot};
use crate::theme::GuiTheme;

impl UiState {
    pub(super) fn settings_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("设置").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("刷新").clicked() {
                    controller.send(GuiRequest::RefreshSettings);
                }
            });
        });
        ui.separator();
        let settings = &snapshot.settings;
        if let Some(error) = settings.error.as_deref() {
            ui.colored_label(theme.red, error);
        }
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                quality_row(ui, controller, settings, theme);
                download_quality_row(ui, controller, settings, theme);
                volume_row(ui, controller, settings, theme);
                source_rows(ui, controller, settings, theme);
                theme_rows(ui, controller, settings, theme);
                keybinding_rows(ui, controller, settings, theme);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "全部改动写入 ~/.config/voicefox/config.toml，与 TUI 共用同一份配置。",
                    )
                    .color(theme.muted)
                    .size(11.0),
                );
            });
    }
}

fn section(ui: &mut egui::Ui, title: &str, theme: GuiTheme) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).color(theme.accent).strong().size(13.0));
}

fn quality_row(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    section(ui, "播放", theme);
    let current = QUALITY_OPTIONS
        .iter()
        .find(|(value, _)| *value == settings.quality)
        .map_or(settings.quality.clone(), |(_, label)| (*label).to_string());
    ui.horizontal(|ui| {
        ui.label(RichText::new("音质").color(theme.subtext));
        egui::ComboBox::from_id_salt("quality_combo")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (value, label) in QUALITY_OPTIONS {
                    if ui
                        .selectable_label(settings.quality == value, label)
                        .clicked()
                    {
                        controller.send(GuiRequest::SetQuality(value.to_string()));
                    }
                }
            });
    });
}

fn download_quality_row(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    let current = if settings.download_quality == "follow" {
        "跟随播放".to_string()
    } else {
        QUALITY_OPTIONS
            .iter()
            .find(|(value, _)| *value == settings.download_quality)
            .map_or(settings.download_quality.clone(), |(_, label)| {
                (*label).to_string()
            })
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("下载音质").color(theme.subtext));
        egui::ComboBox::from_id_salt("download_quality_combo")
            .selected_text(current)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(settings.download_quality == "follow", "跟随播放")
                    .clicked()
                {
                    controller.send(GuiRequest::SetDownloadQuality("follow".to_string()));
                }
                for (value, label) in QUALITY_OPTIONS {
                    if ui
                        .selectable_label(settings.download_quality == value, label)
                        .clicked()
                    {
                        controller.send(GuiRequest::SetDownloadQuality(value.to_string()));
                    }
                }
            });
    });
}

fn volume_row(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("音量").color(theme.subtext));
        let mut volume = settings.volume as f32;
        let slider = ui.add(
            egui::Slider::new(&mut volume, 0.0..=100.0)
                .show_value(false)
                .integer()
                .custom_formatter(|value, _| format!("{value:.0}")),
        );
        if slider.changed() {
            controller.send(GuiRequest::SetVolumeSetting(volume.round() as u32));
        }
        ui.label(RichText::new(format!("{}", settings.volume)).color(theme.muted));
    });
}

fn source_rows(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    section(ui, "音源", theme);
    // 与「音源」页共用同一组请求：设置页只负责编辑，登录态仍在音源页。
    for source in &settings.enabled_sources {
        ui.horizontal(|ui| {
            let is_default = settings.default_source == *source;
            if is_default {
                ui.label(RichText::new("默认").color(theme.accent).size(11.0));
            }
            ui.label(RichText::new(super::source_label(source)).color(theme.text));
            if !is_default && ui.small_button("设为默认").clicked() {
                controller.send(GuiRequest::SetDefaultSource(source.clone()));
            }
        });
    }
    ui.label(
        RichText::new("启用 / 停用音源在「音源」页切换")
            .color(theme.muted)
            .size(11.0),
    );
}

fn theme_rows(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    section(ui, "外观", theme);
    ui.horizontal(|ui| {
        ui.label(RichText::new("主题").color(theme.subtext));
        let label = settings
            .themes
            .iter()
            .find(|(name, _)| *name == settings.theme)
            .map_or(settings.theme.clone(), |(_, label)| label.clone());
        egui::ComboBox::from_id_salt("theme_combo")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (name, label) in &settings.themes {
                    if ui
                        .selectable_label(&settings.theme == name, label)
                        .clicked()
                    {
                        controller.send(GuiRequest::SetTheme(name.clone()));
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("深浅色").color(theme.subtext));
        for (value, label) in [
            ("system", "跟随系统"),
            ("light", "强制浅色"),
            ("dark", "强制深色"),
        ] {
            if ui
                .selectable_label(settings.appearance == value, label)
                .clicked()
            {
                controller.send(GuiRequest::SetAppearance(value.to_string()));
            }
        }
    });
    ui.label(
        RichText::new(format!(
            "当前生效：{}（{}）",
            settings.effective_theme,
            if settings.effective_dark {
                "深色"
            } else {
                "浅色"
            }
        ))
        .color(theme.muted)
        .size(11.0),
    );
}

fn keybinding_rows(
    ui: &mut egui::Ui,
    controller: &mut VoicefoxGuiController,
    settings: &SettingsView,
    theme: GuiTheme,
) {
    section(ui, "快捷键", theme);
    ui.label(
        RichText::new("GUI 兑现播放 / 暂停、上一首、下一首、聚焦搜索；其余键位保留给 TUI。")
            .color(theme.muted)
            .size(11.0),
    );
    for action in GUI_ACTIONS {
        let name = format!("{action:?}");
        let current = settings
            .global_keys
            .iter()
            .find(|(key, _)| *key == name)
            .map_or_else(String::new, |(_, keys)| keys.clone());
        let mut draft = current.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(action.label()).color(theme.text));
            let response = ui.add(
                egui::TextEdit::singleline(&mut draft)
                    .desired_width(120.0)
                    .hint_text("如 Ctrl+l"),
            );
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                controller.send(GuiRequest::SetKeybinding {
                    action: name.clone(),
                    keys: draft.clone(),
                });
            }
        });
    }
}
