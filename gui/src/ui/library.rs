//! 收藏 / 历史 / 歌单面板，以及搜索结果的通用行。

use eframe::egui::{self, Align, Layout, RichText, Sense};

use super::{LibraryTab, UiState, format_duration, source_label, ui_hover_button};
use crate::VoicefoxGuiController;
use crate::cover::CoverImage;
use crate::service::{GuiRequest, LibraryView, PlaylistView, Snapshot, TrackView};
use crate::theme::GuiTheme;

impl UiState {
    pub(super) fn library_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("收藏").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("刷新").clicked() {
                    controller.send(GuiRequest::RefreshLibrary);
                }
            });
        });
        ui.horizontal(|ui| {
            for tab in [
                LibraryTab::Favorites,
                LibraryTab::History,
                LibraryTab::Playlists,
            ] {
                let label = match tab {
                    LibraryTab::Favorites => "收藏",
                    LibraryTab::History => "历史",
                    LibraryTab::Playlists => "歌单",
                };
                if ui
                    .selectable_label(self.library_tab == tab, RichText::new(label).size(13.0))
                    .clicked()
                {
                    self.library_tab = tab;
                    if tab == LibraryTab::Playlists && snapshot.library.playlists.is_empty() {
                        controller.send(GuiRequest::RefreshLibrary);
                    }
                }
            }
        });
        if let Some(error) = snapshot.library.error.as_deref() {
            ui.colored_label(theme.red, error);
        }
        ui.separator();
        match self.library_tab {
            LibraryTab::Favorites => {
                self.library_song_list(
                    ui,
                    controller,
                    &snapshot.library,
                    &snapshot.library.favorites,
                    "还没有收藏",
                    None,
                    theme,
                );
            }
            LibraryTab::History => {
                self.library_song_list(
                    ui,
                    controller,
                    &snapshot.library,
                    &snapshot.library.history,
                    "还没有播放记录",
                    Some(GuiRequest::RemoveFromHistory as fn(TrackView) -> GuiRequest),
                    theme,
                );
            }
            LibraryTab::Playlists => self.playlists_view(ui, controller, snapshot, theme),
        }
    }

    /// 收藏 / 历史共用的歌曲列表：双击播放、可选的移除按钮。
    #[allow(clippy::too_many_arguments)]
    fn library_song_list(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        _library: &LibraryView,
        songs: &[TrackView],
        empty_hint: &str,
        remove: Option<fn(TrackView) -> GuiRequest>,
        theme: GuiTheme,
    ) {
        let mut play: Option<usize> = None;
        let mut removed: Option<TrackView> = None;
        egui::ScrollArea::vertical()
            .id_salt("library_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if songs.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(empty_hint).color(theme.muted));
                    });
                    return;
                }
                for (index, song) in songs.iter().enumerate() {
                    let cover = song
                        .cover_url
                        .as_deref()
                        .and_then(|url| self.cover.get(url));
                    if song_row(ui, cover.as_ref(), song, theme).double_clicked() {
                        play = Some(index);
                    }
                    if remove.is_some() && ui_hover_button(ui, "移除") {
                        removed = Some(song.clone());
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(index) = play {
            controller.send(GuiRequest::PlayList {
                songs: songs.to_vec(),
                index,
            });
        }
        if let (Some(request), Some(song)) = (remove, removed) {
            controller.send(request(song));
        }
    }

    fn playlists_view(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.playlist_draft.name)
                    .hint_text("新歌单名称")
                    .desired_width(200.0),
            );
            if ui.button("新建").clicked() {
                let name = self.playlist_draft.name.trim().to_string();
                if !name.is_empty() {
                    controller.send(GuiRequest::CreatePlaylist(name));
                    self.playlist_draft.name.clear();
                }
            }
        });
        ui.separator();
        let library = &snapshot.library;
        if let Some(playlist) = library.selected.as_ref() {
            self.playlist_detail(ui, controller, playlist, theme);
        } else {
            self.playlist_list(ui, controller, library, theme);
        }
    }

    fn playlist_list(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        library: &LibraryView,
        theme: GuiTheme,
    ) {
        egui::ScrollArea::vertical()
            .id_salt("playlists_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if library.playlists.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("还没有自建歌单").color(theme.muted));
                    });
                    return;
                }
                for playlist in &library.playlists {
                    egui::Frame::NONE
                        .fill(theme.base)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .button(RichText::new(&playlist.name).color(theme.text))
                                    .clicked()
                                {
                                    controller.send(GuiRequest::OpenPlaylist(playlist.id.clone()));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("删除").clicked() {
                                        controller
                                            .send(GuiRequest::DeletePlaylist(playlist.id.clone()));
                                    }
                                    ui.label(
                                        RichText::new(format!("{} 首", playlist.song_count))
                                            .color(theme.muted)
                                            .size(12.0),
                                    );
                                });
                            });
                        });
                    ui.add_space(4.0);
                }
            });
    }

    fn playlist_detail(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        playlist: &PlaylistView,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            if ui.button("← 返回").clicked() {
                controller.send(GuiRequest::RefreshLibrary);
            }
            ui.heading(RichText::new(&playlist.name).color(theme.text).size(16.0));
            if ui.button("删除歌单").clicked() {
                controller.send(GuiRequest::DeletePlaylist(playlist.id.clone()));
            }
        });
        ui.label(
            RichText::new(format!("{} 首", playlist.songs.len()))
                .color(theme.muted)
                .size(12.0),
        );
        ui.separator();
        let mut play: Option<usize> = None;
        let mut removed: Option<TrackView> = None;
        egui::ScrollArea::vertical()
            .id_salt("playlist_songs")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if playlist.songs.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("歌单里还没有歌").color(theme.muted));
                    });
                    return;
                }
                for (index, song) in playlist.songs.iter().enumerate() {
                    let cover = song
                        .cover_url
                        .as_deref()
                        .and_then(|url| self.cover.get(url));
                    if song_row(ui, cover.as_ref(), song, theme).double_clicked() {
                        play = Some(index);
                    }
                    if ui_hover_button(ui, "从歌单移除") {
                        removed = Some(song.clone());
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(index) = play {
            controller.send(GuiRequest::PlayList {
                songs: playlist.songs.clone(),
                index,
            });
        }
        if let Some(song) = removed {
            controller.send(GuiRequest::RemoveFromPlaylist {
                playlist: playlist.id.clone(),
                song,
            });
        }
    }
}

/// 一行歌曲：封面、标题、歌手 · 来源、时长。返回整行响应（双击播放）。
pub(super) fn song_row(
    ui: &mut egui::Ui,
    cover: Option<&CoverImage>,
    song: &TrackView,
    theme: GuiTheme,
) -> egui::Response {
    egui::Frame::NONE
        .fill(theme.base)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                thumbnail(ui, cover, 34.0, theme);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&song.name).color(theme.text).strong());
                    ui.label(
                        RichText::new(format!("{} · {}", song.singer, source_label(&song.source)))
                            .color(theme.muted)
                            .size(12.0),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format_duration(std::time::Duration::from_millis(
                            song.duration_ms,
                        )))
                        .color(theme.muted)
                        .size(12.0),
                    );
                });
            });
            ui.interact(
                ui.min_rect(),
                ui.scope_id().with(("library_row", &song.id)),
                Sense::click(),
            )
        })
        .inner
}

/// 封面缩略图（与搜索行同一张缓存）。
pub(super) fn thumbnail(ui: &mut egui::Ui, cover: Option<&CoverImage>, size: f32, theme: GuiTheme) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, theme.surface0);
    match cover {
        Some(CoverImage::Ready(texture)) => {
            egui::Image::new((texture.id(), egui::Vec2::splat(size)))
                .corner_radius(8.0)
                .paint_at(ui, rect);
        }
        _ => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "♪",
                egui::FontId::proportional(size * 0.4),
                theme.muted,
            );
        }
    }
}
