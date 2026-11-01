//! 音源切换（与 TUI 同一套配置语义）与扫码登录（二维码 + 状态轮询）。

use eframe::egui::{self, Align, Layout, RichText, Vec2};

use super::UiState;
use crate::VoicefoxGuiController;
use crate::service::{GuiRequest, LoginView, Snapshot};
use crate::theme::GuiTheme;

impl UiState {
    pub(super) fn sources_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("音源").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("刷新").clicked() {
                    controller.send(GuiRequest::RefreshSources);
                }
            });
        });
        ui.separator();
        if let Some(error) = snapshot.sources.error.as_deref() {
            ui.colored_label(theme.red, error);
        }
        let sources = &snapshot.sources;
        egui::ScrollArea::vertical()
            .id_salt("sources_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if sources.available.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("正在读取音源配置…").color(theme.muted));
                    });
                }
                for source in &sources.available {
                    let enabled = sources.enabled.contains(source);
                    let logged_in = sources.logged_in.contains(source);
                    let mut checkbox = enabled;
                    egui::Frame::NONE
                        .fill(theme.base)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .checkbox(&mut checkbox, super::source_label(source))
                                    .changed()
                                {
                                    controller.send(GuiRequest::ToggleSource(source.clone()));
                                }
                                if sources.default == *source {
                                    ui.label(RichText::new("默认").color(theme.accent).size(11.0));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if logged_in {
                                        if ui.small_button("登出").clicked() {
                                            controller
                                                .send(GuiRequest::LogoutSource(source.clone()));
                                        }
                                        ui.label(
                                            RichText::new("已登录").color(theme.green).size(11.0),
                                        );
                                    } else if ui.small_button("扫码登录").clicked() {
                                        controller.send(GuiRequest::StartLogin(source.clone()));
                                    }
                                    if sources.default != *source
                                        && enabled
                                        && ui.small_button("设为默认").clicked()
                                    {
                                        controller
                                            .send(GuiRequest::SetDefaultSource(source.clone()));
                                    }
                                });
                            });
                        });
                    ui.add_space(4.0);
                }
                if let Some(login) = sources.login.as_ref() {
                    ui.separator();
                    ui.label(
                        RichText::new(format!(
                            "{} 扫码登录 · {}",
                            super::source_label(&login.source),
                            login.message
                        ))
                        .color(theme.text)
                        .size(13.0),
                    );
                    if login.polling {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new(&login.message).color(theme.muted));
                        });
                    }
                    self.qr_code(ui, snapshot, theme);
                }
            });
    }

    /// 二维码：QQ 走服务端图片（base64），其余音源用链接在本地编码。
    fn qr_code(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, theme: GuiTheme) {
        let Some(login) = snapshot.sources.login.as_ref() else {
            return;
        };
        let Some(payload) = qr_payload(login) else {
            return;
        };
        let key = format!("qr:{}:{}", login.source, login.status);
        if let Some(texture) = self.cover_mut().qr_texture(ui.ctx(), &key, &payload) {
            ui.add(egui::Image::new((texture.id(), Vec2::splat(180.0))));
        }
        ui.label(
            RichText::new(format!("状态：{}", login.status))
                .color(theme.muted)
                .size(11.0),
        );
    }
}

/// 二维码内容：图片渠道返回 base64 图片，链接渠道返回链接本身。
pub(crate) fn qr_payload(login: &LoginView) -> Option<String> {
    if let Some(image) = login.image_base64.as_ref()
        && !image.is_empty()
    {
        return Some(image.clone());
    }
    (!login.url.is_empty()).then(|| login.url.clone())
}
