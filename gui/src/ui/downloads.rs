//! 下载面板：复用共享的并发分片下载管理器（进度 / 速度 / 取消 / 重试）。

use eframe::egui::{self, Align, Layout, RichText};

use super::UiState;
use crate::VoicefoxGuiController;
use crate::service::{GuiRequest, Snapshot};
use crate::theme::GuiTheme;

impl UiState {
    pub(super) fn downloads_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("下载").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("清理已结束").clicked() {
                    controller.send(GuiRequest::ClearFinishedDownloads);
                }
            });
        });
        ui.label(
            RichText::new(format!("保存到 {}", snapshot.downloads.download_dir))
                .color(theme.muted)
                .size(11.0),
        );
        ui.separator();
        let mut retry = false;
        let mut cancel: Option<u64> = None;
        egui::ScrollArea::vertical()
            .id_salt("downloads_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if snapshot.downloads.tasks.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("还没有下载任务").color(theme.muted));
                        ui.label(
                            RichText::new("在搜索结果行点「下载」，或播放栏的「下载」")
                                .color(theme.muted)
                                .size(12.0),
                        );
                    });
                }
                for task in &snapshot.downloads.tasks {
                    egui::Frame::NONE
                        .fill(theme.base)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&task.name).color(theme.text));
                                    ui.label(
                                        RichText::new(format!("{} · {}", task.singer, task.state))
                                            .color(theme.muted)
                                            .size(12.0),
                                    );
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if task.active && ui.small_button("取消").clicked() {
                                        cancel = Some(task.id);
                                    }
                                    if task.can_retry && ui.small_button("重试").clicked() {
                                        retry = true;
                                    }
                                });
                            });
                            let fraction =
                                task.percent().map_or(0.0, |percent| percent as f32 / 100.0);
                            if fraction > 0.0 || task.active {
                                ui.add(
                                    egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                                        .desired_height(6.0)
                                        .show_percentage(),
                                );
                            }
                            if task.active {
                                ui.label(
                                    RichText::new(format!(
                                        "{} / {} · {}/s",
                                        super::format_bytes(task.downloaded),
                                        super::format_bytes(task.total),
                                        super::format_bytes(task.speed_bps)
                                    ))
                                    .color(theme.muted)
                                    .size(11.0),
                                );
                            }
                            if let Some(error) = task.error.as_deref() {
                                ui.colored_label(theme.red, error);
                            }
                        });
                    ui.add_space(4.0);
                }
            });
        if let Some(id) = cancel {
            controller.send(GuiRequest::CancelDownload(id));
        }
        if retry && let Some(song) = snapshot.song.clone() {
            // 重试 = 重新入队当前曲目（下载记录的去重索引会让已完成的不重复下）。
            controller.send(GuiRequest::DownloadSong(song));
        }
    }
}
