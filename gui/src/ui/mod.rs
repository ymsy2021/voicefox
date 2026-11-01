//! GUI 绘制：搜索页 + 队列页 + 歌词页 + 常驻播放栏。
//!
//! 所有状态都来自 [`crate::service::Snapshot`]；这里的按钮只发请求，
//! 不直接碰后端。

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, Sense, Vec2};
use voicefox_application::ApplicationCommand;
use voicefox_runtime::media::{self, PlayMode};

use crate::controller::VoicefoxGuiController;
use crate::cover::{CoverCache, CoverImage, CoverLoader, CoverResults};
use crate::service::{GuiRequest, PlaybackState, SearchView, Snapshot, TrackView};
use crate::theme::GuiTheme;

/// 左栏（搜索 / 队列 / 歌词）宽度。
const SIDE_PANEL_WIDTH: f32 = 420.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Queue,
    Lyrics,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Search => "搜索",
            Self::Queue => "队列",
            Self::Lyrics => "歌词",
        }
    }
}

/// UI 侧的临时状态（输入框内容、当前页签、封面纹理缓存）。
pub struct UiState {
    pub tab: Tab,
    cover: CoverCache,
    wanted_covers: Vec<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Search,
            cover: CoverCache::default(),
            wanted_covers: Vec::new(),
        }
    }
}

impl UiState {
    pub fn with_cover_loader(mut self, loader: std::sync::Arc<dyn CoverLoader>) -> Self {
        self.cover = self.cover.with_loader(loader);
        self
    }

    pub fn set_cover_loader(&mut self, loader: std::sync::Arc<dyn CoverLoader>) {
        self.cover = std::mem::take(&mut self.cover).with_loader(loader);
    }

    pub fn cover_mut(&mut self) -> &mut CoverCache {
        &mut self.cover
    }

    /// 根 Ui 之上的完整绘制。egui 0.37 的面板挂在根 Ui 上，
    /// 因此这里先建一个不可见的整窗 Ui 再画面板。
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        controller: &mut VoicefoxGuiController,
        theme: GuiTheme,
        cover_results: &CoverResults,
    ) {
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("voicefox_root"),
            egui::UiBuilder::new().max_rect(ctx.content_rect()),
        );
        self.show_in(&mut ui, controller, theme, cover_results);
    }

    /// 在一个已有 Ui 上绘制（`eframe::App::ui` / 演示程序直接调用）。
    pub fn show_in(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        theme: GuiTheme,
        cover_results: &CoverResults,
    ) {
        let ctx = ui.ctx().clone();
        ctx.set_visuals(theme.visuals());
        let _ = &ctx;
        let snapshot = controller.snapshot().clone();
        self.collect_covers(&snapshot);
        self.cover.sync(&ctx, cover_results, &self.wanted_covers);

        egui::Panel::top("voicefox_header")
            .frame(
                egui::Frame::NONE
                    .fill(theme.mantle)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| self.header(ui, &snapshot, theme));

        egui::Panel::bottom("voicefox_player")
            .frame(
                egui::Frame::NONE
                    .fill(theme.mantle)
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .stroke(egui::Stroke::new(1.0, theme.border)),
            )
            .show(ui, |ui| self.player_bar(ui, controller, &snapshot, theme));

        egui::Panel::left("voicefox_side")
            .resizable(false)
            .exact_size(SIDE_PANEL_WIDTH)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| match self.tab {
                Tab::Search => self.search_panel(ui, controller, &snapshot, theme),
                Tab::Queue => self.queue_panel(ui, controller, &snapshot, theme),
                Tab::Lyrics => self.lyrics_panel(ui, &snapshot, theme),
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(16)))
            .show(ui, |ui| self.main_panel(ui, &snapshot, theme));
    }

    fn collect_covers(&mut self, snapshot: &Snapshot) {
        self.wanted_covers.clear();
        if let Some(url) = snapshot
            .song
            .as_ref()
            .and_then(|song| song.cover_url.clone())
        {
            self.wanted_covers.push(url);
        }
        for song in &snapshot.search.items {
            if let Some(url) = song.cover_url.clone() {
                self.wanted_covers.push(url);
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, theme: GuiTheme) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Voicefox")
                    .color(theme.accent)
                    .strong()
                    .size(20.0),
            );
            ui.add_space(16.0);
            for tab in [Tab::Search, Tab::Queue, Tab::Lyrics] {
                if ui
                    .selectable_label(self.tab == tab, RichText::new(tab.label()).size(14.0))
                    .clicked()
                {
                    self.tab = tab;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let media = if snapshot.has_media_controls {
                    "媒体键已接入"
                } else {
                    "媒体键未接入"
                };
                ui.label(RichText::new(media).color(theme.muted).size(12.0));
                ui.label(
                    RichText::new(snapshot.state.label())
                        .color(theme.subtext)
                        .size(12.0),
                );
            });
        });
    }

    fn search_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.heading(RichText::new("搜索").color(theme.text));
        ui.add_space(8.0);
        let mut submit = false;
        ui.horizontal(|ui| {
            let field = ui.add_sized(
                [SIDE_PANEL_WIDTH - 130.0, 28.0],
                egui::TextEdit::singleline(controller.search_input_mut())
                    .hint_text("歌曲、歌手或专辑")
                    .desired_width(f32::INFINITY),
            );
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            if ui.button("搜索").clicked() {
                submit = true;
            }
        });
        if submit {
            let keyword = controller.search_input_mut().trim().to_string();
            if keyword.is_empty() {
                controller.set_status("请输入关键词");
            } else {
                controller.send(GuiRequest::Dispatch(ApplicationCommand::Search {
                    keyword,
                    source: None,
                }));
            }
        }
        ui.add_space(6.0);
        let search = snapshot.search.clone();
        if search.loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("正在检索各音源…").color(theme.muted));
            });
        }
        if let Some(error) = search.error.as_deref() {
            ui.colored_label(theme.red, format!("搜索失败：{error}"));
        }
        if !search.items.is_empty() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("播放全部").clicked() {
                    controller.send(GuiRequest::PlayList {
                        songs: search.items.clone(),
                        index: 0,
                    });
                    controller.set_status(format!("开始播放 {} 首搜索结果", search.items.len()));
                }
                ui.label(
                    RichText::new(format!("共 {} 首", search.items.len()))
                        .color(theme.muted)
                        .size(12.0),
                );
            });
        }
        ui.separator();
        self.search_results(ui, controller, &search, theme);
    }

    fn search_results(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        search: &SearchView,
        theme: GuiTheme,
    ) {
        if search.items.is_empty() {
            if !search.loading && search.error.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("输入关键词开始搜索").color(theme.muted));
                    ui.label(
                        RichText::new("双击结果播放，右键可加入队列")
                            .color(theme.muted)
                            .size(12.0),
                    );
                });
            }
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("search_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut play: Option<usize> = None;
                let mut queue_add: Option<TrackView> = None;
                let mut queue_next: Option<TrackView> = None;
                for (index, song) in search.items.iter().enumerate() {
                    let covering = song.cover_url.as_ref().and_then(|url| self.cover.get(url));
                    egui::Frame::NONE
                        .fill(theme.base)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            // 整行的可点区域必须在创建子控件**之前**注册（`UiBuilder::sense`
                            // 在 `Ui::new` 里入栈），否则后注册的整行矩形会在命中测试里盖住
                            // 行内的按钮，点「+」就永远收不到点击。
                            let row = ui.scope_builder(
                                egui::UiBuilder::new().sense(Sense::click()),
                                |ui| {
                                    ui.horizontal(|ui| {
                                        self.thumbnail(ui, covering.as_ref(), 34.0, theme);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                RichText::new(&song.name)
                                                    .color(theme.text)
                                                    .strong(),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "{} · {}",
                                                    song.singer, song.source
                                                ))
                                                .color(theme.muted)
                                                .size(12.0),
                                            );
                                        });
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button("+")
                                                    .on_hover_text("加入队列")
                                                    .clicked()
                                                {
                                                    queue_add = Some(song.clone());
                                                }
                                                ui.label(
                                                    RichText::new(format_duration(
                                                        Duration::from_millis(song.duration_ms),
                                                    ))
                                                    .color(theme.muted)
                                                    .size(12.0),
                                                );
                                            },
                                        );
                                    });
                                },
                            );
                            let response = row.response;
                            if response.double_clicked() {
                                play = Some(index);
                            }
                            response.context_menu(|ui| {
                                if ui.button("播放").clicked() {
                                    play = Some(index);
                                    ui.close();
                                }
                                if ui.button("下一首播放").clicked() {
                                    queue_next = Some(song.clone());
                                    ui.close();
                                }
                                if ui.button("加入队列").clicked() {
                                    queue_add = Some(song.clone());
                                    ui.close();
                                }
                            });
                        });
                    ui.add_space(4.0);
                }
                if let Some(song) = queue_add {
                    controller.send(GuiRequest::Dispatch(ApplicationCommand::QueueAdd {
                        song: song.to_song(),
                        next: false,
                    }));
                    controller.set_status(format!("已加入队列：{}", song.name));
                }
                if let Some(song) = queue_next {
                    controller.send(GuiRequest::Dispatch(ApplicationCommand::QueueAdd {
                        song: song.to_song(),
                        next: true,
                    }));
                    controller.set_status(format!("下一首播放：{}", song.name));
                }
                if let Some(index) = play {
                    controller.send(GuiRequest::PlayList {
                        songs: search.items.clone(),
                        index,
                    });
                }
                if search.has_more {
                    ui.add_space(8.0);
                    if ui.button("加载更多").clicked() {
                        controller.send(GuiRequest::Dispatch(ApplicationCommand::SearchMore {
                            keyword: search.keyword.clone(),
                            page: search.page + 1,
                            source: None,
                        }));
                    }
                }
            });
    }

    fn queue_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("队列").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !snapshot.queue.is_empty() && ui.button("清空").clicked() {
                    controller.send(GuiRequest::Dispatch(ApplicationCommand::QueueClear));
                }
            });
        });
        ui.label(
            RichText::new(format!(
                "{} 首 · 双击播放 · ↑↓ 调整顺序 · × 移除",
                snapshot.queue.len()
            ))
            .color(theme.muted)
            .size(12.0),
        );
        ui.separator();
        if snapshot.queue.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("队列为空").color(theme.muted));
                ui.label(
                    RichText::new("在搜索结果里点 + 加入队列")
                        .color(theme.muted)
                        .size(12.0),
                );
            });
            return;
        }
        let mut play: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("queue_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let total = snapshot.queue.len();
                for (index, song) in snapshot.queue.iter().enumerate() {
                    let current = index == snapshot.queue_index;
                    let fill = if current { theme.surface0 } else { theme.base };
                    egui::Frame::NONE
                        .fill(fill)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            // 同搜索行：整行可点区域先于子控件注册，避免盖住
                            // 「×」「↑」「↓」按钮。
                            let row = ui.scope_builder(
                                egui::UiBuilder::new().sense(Sense::click()),
                                |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(if current { "▶" } else { "·" })
                                                .color(theme.accent),
                                        );
                                        ui.vertical(|ui| {
                                            ui.label(RichText::new(&song.name).color(theme.text));
                                            ui.label(
                                                RichText::new(&song.singer)
                                                    .color(theme.muted)
                                                    .size(12.0),
                                            );
                                        });
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button("×")
                                                    .on_hover_text("移除")
                                                    .clicked()
                                                {
                                                    remove = Some(index);
                                                }
                                                if index + 1 < total
                                                    && ui
                                                        .small_button("↓")
                                                        .on_hover_text("下移")
                                                        .clicked()
                                                {
                                                    move_down = Some(index);
                                                }
                                                if index > 0
                                                    && ui
                                                        .small_button("↑")
                                                        .on_hover_text("上移")
                                                        .clicked()
                                                {
                                                    move_up = Some(index);
                                                }
                                            },
                                        );
                                    });
                                },
                            );
                            if row.response.double_clicked() {
                                play = Some(index);
                            }
                        });
                    ui.add_space(4.0);
                }
            });
        if let Some(index) = play {
            controller.send(GuiRequest::PlayList {
                songs: snapshot.queue.clone(),
                index,
            });
        }
        if let Some(index) = remove {
            controller.send(GuiRequest::Dispatch(ApplicationCommand::QueueRemove {
                index,
            }));
        }
        if let Some(index) = move_up {
            controller.send(GuiRequest::MoveQueue {
                from: index,
                to: index - 1,
            });
        }
        if let Some(index) = move_down {
            controller.send(GuiRequest::MoveQueue {
                from: index,
                to: index + 1,
            });
        }
    }

    fn lyrics_panel(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, theme: GuiTheme) {
        ui.heading(RichText::new("歌词").color(theme.text));
        ui.separator();
        if snapshot.lyrics.lines.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if snapshot.song.is_some() {
                        "暂无歌词"
                    } else {
                        "开始播放后显示歌词"
                    })
                    .color(theme.muted),
                );
            });
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("lyrics_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, line) in snapshot.lyrics.lines.iter().enumerate() {
                    let text = if index == snapshot.lyrics.current {
                        RichText::new(line).color(theme.accent).strong().size(16.0)
                    } else {
                        RichText::new(line).color(theme.subtext).size(14.0)
                    };
                    ui.label(text);
                    ui.add_space(2.0);
                }
            });
    }

    fn main_panel(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, theme: GuiTheme) {
        let cover = snapshot
            .song
            .as_ref()
            .and_then(|song| song.cover_url.as_ref())
            .and_then(|url| self.cover.get(url));
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            match snapshot.song.as_ref() {
                Some(song) => {
                    self.thumbnail(ui, cover.as_ref(), 260.0, theme);
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new(&song.name)
                            .color(theme.text)
                            .size(26.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    let subtitle = if song.album.trim().is_empty() {
                        song.singer.clone()
                    } else {
                        format!("{} — {}", song.singer, song.album)
                    };
                    ui.label(RichText::new(subtitle).color(theme.subtext).size(14.0));
                }
                None => {
                    self.thumbnail(ui, None, 260.0, theme);
                    ui.add_space(18.0);
                    ui.label(RichText::new("还没有在播放").color(theme.text).size(24.0));
                    ui.label(
                        RichText::new("在左侧搜索，双击结果开始播放")
                            .color(theme.muted)
                            .size(13.0),
                    );
                }
            }
            ui.add_space(20.0);
            if !snapshot.lyrics.lines.is_empty() {
                let current = snapshot
                    .lyrics
                    .lines
                    .get(snapshot.lyrics.current)
                    .cloned()
                    .unwrap_or_default();
                if !current.is_empty() {
                    ui.label(RichText::new(current).color(theme.accent).size(18.0));
                }
            } else if snapshot.song.is_some() {
                ui.label(RichText::new("暂无歌词").color(theme.muted).size(13.0));
            }
        });
    }

    /// 常驻播放栏：封面、歌名、上一首/播放/下一首、模式、进度、音量。
    fn player_bar(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        let cover = snapshot
            .song
            .as_ref()
            .and_then(|song| song.cover_url.as_ref())
            .and_then(|url| self.cover.get(url));
        ui.horizontal(|ui| {
            self.thumbnail(ui, cover.as_ref(), 52.0, theme);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                let title = snapshot
                    .song
                    .as_ref()
                    .map_or_else(|| "未在播放".to_string(), |song| song.name.clone());
                let singer = snapshot
                    .song
                    .as_ref()
                    .map_or_else(String::new, |song| song.singer.clone());
                ui.label(RichText::new(title).color(theme.text).strong().size(15.0));
                ui.label(RichText::new(singer).color(theme.muted).size(12.0));
            });
            ui.add_space(20.0);
            if transport_button(ui, "上一首", true).clicked() {
                controller.send(GuiRequest::Dispatch(ApplicationCommand::Previous));
            }
            let playing = snapshot.state.is_playing();
            let restart = snapshot.playlist_ended && !playing;
            let (icon, name) = if playing {
                ("‖", "暂停")
            } else if restart {
                ("▶", "重新播放")
            } else {
                ("▶", "播放")
            };
            let label = format!("{icon}  {name}");
            if ui
                .add_sized(
                    [64.0, 30.0],
                    egui::Button::new(RichText::new(label).size(14.0)),
                )
                .on_hover_text(name)
                .clicked()
            {
                // 顺序 / 播完停止模式播到结尾后，播放键从头再来。
                if snapshot.playlist_ended && !playing {
                    controller.send(GuiRequest::RestartPlaylist);
                } else {
                    controller.send(GuiRequest::Dispatch(toggle_command(snapshot.state)));
                }
            }
            if transport_button(ui, "下一首", false).clicked() {
                controller.send(GuiRequest::Dispatch(ApplicationCommand::Next));
            }
            ui.add_space(12.0);
            let mode = snapshot.play_mode;
            if ui
                .button(mode.label())
                .on_hover_text("切换播放模式")
                .clicked()
            {
                let next = media::cycle_play_mode(mode);
                controller.send(GuiRequest::SetPlayMode(next));
                controller.set_status(format!("播放模式：{}", next.label()));
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    format_duration(snapshot.position),
                    format_duration(snapshot.duration)
                ))
                .color(theme.muted)
                .size(12.0),
            );
            let mut position = snapshot.position.as_secs_f64();
            let max = snapshot.duration.as_secs_f64().max(0.001);
            let slider = ui
                .add_enabled(
                    snapshot.duration.as_millis() > 0,
                    egui::Slider::new(&mut position, 0.0..=max)
                        .show_value(false)
                        .trailing_fill(true)
                        .min_decimals(0),
                )
                .on_hover_text("播放进度（拖动跳转）");
            if slider.drag_stopped() {
                controller.send(GuiRequest::Dispatch(ApplicationCommand::Seek(
                    Duration::from_secs_f64(position),
                )));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut volume = snapshot.volume as f32;
                let volume_slider = ui
                    .add_sized(
                        [110.0, 18.0],
                        egui::Slider::new(&mut volume, 0.0..=100.0).integer(),
                    )
                    .on_hover_text("音量");
                if volume_slider.changed() {
                    controller.send(GuiRequest::SetVolume(volume.round() as u32));
                }
                ui.label(RichText::new("音量").color(theme.muted).size(12.0));
            });
        });
        if let Some(status) = controller.status().map(str::to_string) {
            ui.add_space(2.0);
            ui.label(RichText::new(status).color(theme.subtext).size(11.0));
        }
    }

    fn thumbnail(&self, ui: &mut egui::Ui, cover: Option<&CoverImage>, size: f32, theme: GuiTheme) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 8.0, theme.surface0);
        match cover {
            Some(CoverImage::Ready(texture)) => {
                egui::Image::new((texture.id(), Vec2::splat(size)))
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
}

/// 切歌按钮：用 egui 自带的箭头图形，不依赖字体里的媒体符号字形
/// （默认字体没有 U+23EE 这类码位，会渲染成方框）。
fn transport_button(ui: &mut egui::Ui, tooltip: &str, previous: bool) -> egui::Response {
    let symbol = if previous { "◀◀" } else { "▶▶" };
    ui.add(egui::Button::new(RichText::new(symbol).size(12.0)))
        .on_hover_text(tooltip)
}

/// 播放按钮在空闲时等价于「继续播放队列当前曲目」。
fn toggle_command(state: PlaybackState) -> ApplicationCommand {
    match state {
        PlaybackState::Idle | PlaybackState::Stopped => ApplicationCommand::PlayCurrent,
        _ => ApplicationCommand::Toggle,
    }
}

pub fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// 播放模式标签（与 MPRIS loop status 同一份定义）。
pub fn play_mode_label(mode: PlayMode) -> &'static str {
    mode.label()
}

#[cfg(test)]
mod tests {
    use super::{Tab, format_duration, toggle_command};
    use crate::service::PlaybackState;
    use std::time::Duration;
    use voicefox_application::ApplicationCommand;

    #[test]
    fn durations_render_as_minutes_and_seconds() {
        assert_eq!(format_duration(Duration::ZERO), "00:00");
        assert_eq!(format_duration(Duration::from_secs(65)), "01:05");
        assert_eq!(format_duration(Duration::from_secs(3725)), "62:05");
    }

    #[test]
    fn tabs_cover_the_requested_areas() {
        assert_eq!(Tab::Search.label(), "搜索");
        assert_eq!(Tab::Queue.label(), "队列");
        assert_eq!(Tab::Lyrics.label(), "歌词");
    }

    #[test]
    fn play_button_starts_queue_when_idle() {
        assert!(matches!(
            toggle_command(PlaybackState::Idle),
            ApplicationCommand::PlayCurrent
        ));
        assert!(matches!(
            toggle_command(PlaybackState::Playing),
            ApplicationCommand::Toggle
        ));
    }
}
