//! GUI 绘制：搜索 / 队列 / 歌词 / 收藏 / 音源 / 下载 + 常驻播放栏。
//!
//! 所有状态都来自 [`crate::service::Snapshot`]；这里的按钮只发请求，
//! 不直接碰后端。

use std::time::Duration;

use eframe::egui::{self, Align, Layout, RichText, Sense};
use voicefox_application::ApplicationCommand;
use voicefox_runtime::media::{self, PlayMode};

use crate::controller::VoicefoxGuiController;
use crate::cover::{CoverCache, CoverLoader, CoverResults};
use crate::service::{GuiRequest, PlaybackState, SearchView, Snapshot, TrackView};
use crate::theme::GuiTheme;

mod downloads;
mod library;
mod sources;

use library::thumbnail;

/// 左栏（搜索 / 队列 / 歌词）宽度。
const SIDE_PANEL_WIDTH: f32 = 420.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Queue,
    Lyrics,
    Library,
    Sources,
    Downloads,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Search => "搜索",
            Self::Queue => "队列",
            Self::Lyrics => "歌词",
            Self::Library => "收藏",
            Self::Sources => "音源",
            Self::Downloads => "下载",
        }
    }

    pub const ALL: [Tab; 6] = [
        Tab::Search,
        Tab::Queue,
        Tab::Lyrics,
        Tab::Library,
        Tab::Sources,
        Tab::Downloads,
    ];
}

/// 收藏页的当前子页（收藏 / 历史 / 歌单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryTab {
    #[default]
    Favorites,
    History,
    Playlists,
}

/// 收藏页里新建歌单的输入框状态。
#[derive(Default)]
struct PlaylistDraft {
    name: String,
    /// 右键「加入歌单」选中的歌曲（弹出菜单里选歌单时使用）。
    add_target: Option<TrackView>,
}

/// UI 侧的临时状态（输入框内容、当前页签、拖动中的队列项、封面纹理缓存）。
pub struct UiState {
    pub tab: Tab,
    pub library_tab: LibraryTab,
    /// 正在拖动排序的队列项（按下时记录源索引）。
    drag_source: Option<usize>,
    /// 拖动过程中指针所在的落点预览（只影响绘制，不改数据）。
    drag_target: Option<usize>,
    /// 新建歌单 / 加入歌单的输入状态。
    playlist_draft: PlaylistDraft,
    cover: CoverCache,
    wanted_covers: Vec<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            tab: Tab::Search,
            library_tab: LibraryTab::Favorites,
            drag_source: None,
            drag_target: None,
            playlist_draft: PlaylistDraft::default(),
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
                Tab::Lyrics => self.lyrics_panel(ui, controller, &snapshot, theme),
                Tab::Library => self.library_panel(ui, controller, &snapshot, theme),
                Tab::Sources => self.sources_panel(ui, controller, &snapshot, theme),
                Tab::Downloads => self.downloads_panel(ui, controller, &snapshot, theme),
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
        let lists: [&[TrackView]; 5] = [
            &snapshot.search.items,
            &snapshot.library.favorites,
            &snapshot.library.history,
            snapshot
                .library
                .selected
                .as_ref()
                .map_or(&[][..], |playlist| &playlist.songs),
            &snapshot.queue,
        ];
        for song in lists.iter().flat_map(|list| list.iter()) {
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
            for tab in Tab::ALL {
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
                let mut favorite: Option<TrackView> = None;
                let mut download: Option<TrackView> = None;
                let mut add_target: Option<TrackView> = None;
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
                                        thumbnail(ui, covering.as_ref(), 34.0, theme);
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
                                                if ui
                                                    .small_button("↓")
                                                    .on_hover_text("下载")
                                                    .clicked()
                                                {
                                                    download = Some(song.clone());
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
                                if ui.button("收藏").clicked() {
                                    favorite = Some(song.clone());
                                    ui.close();
                                }
                                if ui.button("下载").clicked() {
                                    download = Some(song.clone());
                                    ui.close();
                                }
                                if ui.button("加入歌单…").clicked() {
                                    add_target = Some(song.clone());
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
                if let Some(song) = favorite {
                    controller.send(GuiRequest::ToggleFavorite(song));
                }
                if let Some(song) = download {
                    controller.send(GuiRequest::DownloadSong(song));
                    controller.set_status("已加入下载");
                }
                if let Some(song) = add_target {
                    self.playlist_draft.add_target = Some(song);
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

    /// 队列：按 egui 的拖动协议重排。
    ///
    /// 拖动本身只更新 UI 侧的预览（`drag_source` / `drag_target`）；松手时把
    /// 「从哪到哪」作为一条请求发给服务线程，由核心队列服务执行
    /// `ApplicationCommand::QueueMove` 并广播 `QueueChanged`。UI 从不直接改
    /// 队列结构，因此不会出现界面与核心不一致。
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
                "{} 首 · 拖动排序 · 双击播放 · × 移除",
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
        let mut moved: Option<(usize, usize)> = None;
        egui::ScrollArea::vertical()
            .id_salt("queue_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let total = snapshot.queue.len();
                // 拖动中的落点：指针越过某行的一半就插到它前面/后面。
                let pointer = ui.input(|input| input.pointer.interact_pos());
                let mut hover_target = None;
                for (index, song) in snapshot.queue.iter().enumerate() {
                    let current = index == snapshot.queue_index;
                    let dragging = self.drag_source == Some(index);
                    let is_target = self.drag_source.is_some()
                        && self.drag_target == Some(index)
                        && self.drag_source != Some(index);
                    let fill = if current {
                        theme.surface0
                    } else if is_target || dragging {
                        theme.surface1
                    } else {
                        theme.base
                    };
                    let frame = egui::Frame::NONE
                        .fill(fill)
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            // 整行的可点区域必须在子控件之前注册（与搜索行同一原因：
                            // 后注册的整行矩形会在命中测试里盖住行内按钮）。
                            let row = ui.scope_builder(
                                egui::UiBuilder::new().sense(Sense::click()),
                                |ui| {
                                    ui.horizontal(|ui| {
                                        // 拖动把手：整行可拖，但按钮先注册、不参与拖动。
                                        let handle = ui
                                            .add(
                                                egui::Label::new(RichText::new("≡").color(
                                                    if dragging {
                                                        theme.accent
                                                    } else {
                                                        theme.muted
                                                    },
                                                ))
                                                .sense(Sense::drag()),
                                            )
                                            .on_hover_text("按住拖动排序");
                                        let row_rect = ui.max_rect();
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
                                            },
                                        );
                                        if handle.drag_started() {
                                            self.drag_source = Some(index);
                                            self.drag_target = Some(index);
                                        }
                                        // 拖动经过的落点：同一时刻只有一个 hover，交给
                                        // `hover_target` 汇总，松手时按最后的值提交。
                                        if handle.hovered()
                                            || row_rect.contains(pointer.unwrap_or_default())
                                        {
                                            hover_target = Some(index);
                                        }
                                        let _ = row_rect;
                                    })
                                },
                            );
                            if row.response.double_clicked() {
                                play = Some(index);
                            }
                        });
                    // 不再对整行矩形补一次 interact：那会在行内按钮之后注册，
                    // 命中测试里盖住「×」，按钮点不动。双击由 `row.response` 负责。
                    let _ = &frame;
                    if self.drag_source.is_some() && hover_target == Some(index) {
                        self.drag_target = Some(index);
                    }
                    ui.add_space(4.0);
                }
                // 松手：在整面板范围里判定，避免拖出某一行后丢失。
                let released = ui.input(|input| input.pointer.any_released());
                if released && let Some(from) = self.drag_source.take() {
                    let to = self.drag_target.take();
                    if let Some(to) = to
                        && to != from
                        && to < total
                    {
                        moved = Some((from, to));
                    }
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
        if let Some((from, to)) = moved {
            controller.send(GuiRequest::ReorderQueue { from, to });
        }
    }

    /// 歌词页：当前行居中；翻译显示在当前行下方；YRC 逐字按播放位置高亮；
    /// 播放时行滚动跟随（只在当前行变化时滚一次，不抢用户的手动滚动）。
    fn lyrics_panel(
        &mut self,
        ui: &mut egui::Ui,
        controller: &mut VoicefoxGuiController,
        snapshot: &Snapshot,
        theme: GuiTheme,
    ) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("歌词").color(theme.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("逐字")
                    .on_hover_text("切换逐字歌词（YRC）")
                    .clicked()
                {
                    controller.send(GuiRequest::ToggleLyric(crate::service::LyricToggle::Yrc));
                }
                if ui
                    .small_button("翻译")
                    .on_hover_text("切换歌词翻译")
                    .clicked()
                {
                    controller.send(GuiRequest::ToggleLyric(
                        crate::service::LyricToggle::Translation,
                    ));
                }
            });
        });
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
        let position_ms = snapshot.position.as_millis() as u64;
        egui::ScrollArea::vertical()
            .id_salt("lyrics_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, line) in snapshot.lyrics.lines.iter().enumerate() {
                    let current = index == snapshot.lyrics.current;
                    if current && !snapshot.lyrics.words.is_empty() {
                        // YRC：当前行按词拼接，已唱完的词用强调色。
                        ui.horizontal_wrapped(|ui| {
                            for word in &snapshot.lyrics.words {
                                let sung = position_ms >= word.start_ms;
                                let finished = position_ms >= word.start_ms + word.duration_ms;
                                let color = if finished {
                                    theme.accent
                                } else if sung {
                                    theme.text
                                } else {
                                    theme.muted
                                };
                                ui.label(RichText::new(&word.text).color(color).size(16.0));
                            }
                        });
                    } else {
                        let text = if current {
                            RichText::new(line).color(theme.accent).strong().size(16.0)
                        } else {
                            RichText::new(line).color(theme.subtext).size(14.0)
                        };
                        ui.label(text);
                    }
                    if current && let Some(translation) = snapshot.lyrics.translation.as_deref() {
                        ui.label(
                            RichText::new(translation.trim())
                                .color(theme.accent)
                                .italics()
                                .size(13.0),
                        );
                    }
                    ui.add_space(4.0);
                }
            });
        self.follow_current_lyric(ui, snapshot);
    }

    /// 当前行滚动跟随：只在「换到新的一行」时滚动一次到中间位置。
    ///
    /// 不每帧强制滚动：用户手动往回翻时不会被拉回；跟随状态按歌曲 id
    /// 区分，换歌后从新歌的当前行重新跟随。
    fn follow_current_lyric(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        let key = egui::Id::unique((
            "lyrics_follow",
            snapshot.song.as_ref().map(|song| song.id.clone()),
        ));
        let last: Option<usize> = ui.ctx().data(|data| data.get_temp(key));
        if last == Some(snapshot.lyrics.current) {
            return;
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(key, snapshot.lyrics.current));
        let offset = lyric_follow_offset(snapshot);
        ui.scroll_to_rect(
            egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), offset),
                egui::Vec2::new(ui.max_rect().width(), LYRIC_ROW_HEIGHT),
            ),
            Some(egui::Align::Center),
        );
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
                    thumbnail(ui, cover.as_ref(), 260.0, theme);
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
                    thumbnail(ui, None, 260.0, theme);
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
                if let Some(translation) = snapshot.lyrics.translation.as_deref() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(translation.trim())
                            .color(theme.accent)
                            .italics()
                            .size(13.0),
                    );
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
            thumbnail(ui, cover.as_ref(), 52.0, theme);
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
            if let Some(song) = snapshot.song.clone() {
                let favorite = snapshot
                    .library
                    .favorites
                    .iter()
                    .any(|item| item.id == song.id);
                if ui
                    .button(if favorite { "♥" } else { "♡" })
                    .on_hover_text(if favorite { "取消收藏" } else { "收藏" })
                    .clicked()
                {
                    controller.send(GuiRequest::ToggleFavorite(song.clone()));
                }
                if ui.button("下载").on_hover_text("下载当前歌曲").clicked() {
                    controller.send(GuiRequest::DownloadSong(song.clone()));
                    controller.set_status("已加入下载");
                }
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
}

/// 切歌按钮：用 egui 自带的箭头图形，不依赖字体里的媒体符号字形
/// （默认字体没有 U+23EE 这类码位，会渲染成方框）。
fn transport_button(ui: &mut egui::Ui, tooltip: &str, previous: bool) -> egui::Response {
    let symbol = if previous { "◀◀" } else { "▶▶" };
    ui.add(egui::Button::new(RichText::new(symbol).size(12.0)))
        .on_hover_text(tooltip)
}

/// 播放按钮在空闲时等价于「继续播放队列当前曲目」。
/// 列表行右侧的悬停显示按钮（移除等操作）。
pub fn ui_hover_button(ui: &mut egui::Ui, tooltip: &str) -> bool {
    ui.small_button("×").on_hover_text(tooltip).clicked()
}

/// 音源显示名：配置里是音源 id（`kw` / `tx` …），界面上给出可读名称。
pub fn source_label(source: &str) -> String {
    match source {
        "kw" => "酷我",
        "kg" => "酷狗",
        "tx" => "QQ 音乐",
        "wy" => "网易云",
        "mg" => "咪咕",
        "bili" => "哔哩哔哩",
        "qianqian" => "千千",
        "joox" => "JOOX",
        "jamendo" => "Jamendo",
        "fivesing" => "5sing",
        "apple" => "Apple Music",
        "soda" => "汽水",
        "local" => "本地",
        other => other,
    }
    .to_string()
}

/// 字节数的可读形式（下载面板用）。
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 歌词每行在跟随定位时使用的高度估计（正文行 + 间距）。
const LYRIC_ROW_HEIGHT: f32 = 22.0;

/// 跟随定位时当前行的纵向偏移（每行正文 + 间距；当前行有译文时再让出一行）。
pub fn lyric_follow_offset(snapshot: &Snapshot) -> f32 {
    let translation_row =
        usize::from(snapshot.lyrics.translation.is_some() && !snapshot.lyrics.lines.is_empty());
    (snapshot.lyrics.current * 2 + translation_row) as f32 * LYRIC_ROW_HEIGHT
}

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
    use super::{
        Tab, format_bytes, format_duration, lyric_follow_offset, source_label, toggle_command,
    };
    use crate::service::PlaybackState;
    use std::time::Duration;
    use voicefox_application::ApplicationCommand;

    #[test]
    fn lyric_follow_offset_tracks_the_current_row() {
        let mut snapshot = crate::service::Snapshot::default();
        snapshot.lyrics.lines = vec!["a".into(), "b".into(), "c".into()];
        snapshot.lyrics.current = 0;
        assert_eq!(lyric_follow_offset(&snapshot), 0.0);
        snapshot.lyrics.current = 2;
        let rows = lyric_follow_offset(&snapshot) / super::LYRIC_ROW_HEIGHT;
        assert_eq!(rows, 4.0, "每行正文占一行，当前行有译文时再让出一行");
        snapshot.lyrics.translation = Some("译文".into());
        assert!(lyric_follow_offset(&snapshot) / super::LYRIC_ROW_HEIGHT > 4.0);
    }

    #[test]
    fn p1_tabs_and_labels_are_visible() {
        assert_eq!(Tab::ALL.len(), 6);
        assert_eq!(Tab::Library.label(), "收藏");
        assert_eq!(Tab::Sources.label(), "音源");
        assert_eq!(Tab::Downloads.label(), "下载");
        assert_eq!(source_label("kg"), "酷狗");
        assert_eq!(source_label("tx"), "QQ 音乐");
        assert_eq!(source_label("unknown"), "unknown");
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

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
