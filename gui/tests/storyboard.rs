//! 界面快照剧本：用真实的 GUI 绘制 + 演示后端，无网络地走一遍主要流程，
//! 并把每一步写成 PNG（供验收、PR 说明与文档）。
//!
//! 设置 `VOICEFOX_GUI_SCREENSHOT=<目录>` 时输出 `step-<n>-<name>.png`；
//! 不设置时只跑断言行（CI 里也无副作用）。

use std::sync::Arc;

use egui_kittest::{Harness, kittest::Queryable};
use lx_core::model::config::Config;
use voicefox_application::ApplicationCommand;
use voicefox_gui::demo::DemoBackend;
use voicefox_gui::theme::GuiTheme;
use voicefox_gui::ui::{Tab, UiState};
use voicefox_gui::{GuiBackend, GuiRequest, Snapshot, VoicefoxGuiController};

/// 一个剧本步骤：改状态 + 断言界面内容。
struct Step {
    name: &'static str,
    run: Box<dyn Fn(&mut Stage)>,
    expect: &'static [&'static str],
    absent: &'static [&'static str],
}

struct Stage {
    controller: VoicefoxGuiController,
    ui: UiState,
    theme: GuiTheme,
    backend: Arc<DemoBackend>,
}

impl Stage {
    fn send(&mut self, request: GuiRequest) {
        self.controller.send(request);
    }

    fn command(&mut self, command: ApplicationCommand) {
        self.send(GuiRequest::Dispatch(command));
    }
}

fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "search-empty",
            run: Box::new(|_| {}),
            expect: &["Voicefox", "还没有在播放", "未在播放", "列表循环"],
            absent: &[],
        },
        Step {
            name: "search-results",
            run: Box::new(|stage: &mut Stage| {
                stage.command(ApplicationCommand::Search {
                    keyword: "周杰伦".into(),
                    source: None,
                });
            }),
            expect: &["夜曲", "七里香", "播放全部", "周杰伦"],
            absent: &[],
        },
        Step {
            name: "playing",
            run: Box::new(|stage: &mut Stage| {
                // 双击第 2 条结果 → 播放整张结果列表的对应条目。
                let songs = stage.backend.take_snapshot().search.items;
                stage.send(GuiRequest::PlayList { songs, index: 1 });
            }),
            expect: &["七里香", "播放中"],
            absent: &["还没有在播放"],
        },
        Step {
            name: "queue",
            run: Box::new(|stage: &mut Stage| {
                stage.ui.tab = Tab::Queue;
            }),
            expect: &["清空", "拖动排序"],
            absent: &[],
        },
        Step {
            name: "queue-reordered",
            run: Box::new(|stage: &mut Stage| {
                // 拖动排序的结果落到同一条请求上（UI 拖动结束时发的就是它）。
                stage.send(GuiRequest::ReorderQueue { from: 0, to: 2 });
            }),
            expect: &["夜曲"],
            absent: &[],
        },
        Step {
            name: "paused-lyrics",
            run: Box::new(|stage: &mut Stage| {
                stage.command(ApplicationCommand::Toggle);
                stage.ui.tab = Tab::Lyrics;
            }),
            expect: &["已暂停", "歌词", "翻译", "逐字"],
            absent: &[],
        },
        Step {
            name: "lyrics-translation",
            run: Box::new(|stage: &mut Stage| {
                stage.send(GuiRequest::ToggleLyric(
                    voicefox_gui::service::LyricToggle::Translation,
                ));
                // 演示后端没有真的核心，这里把「当前行译文」放进快照，
                // 断言界面确实把翻译画在当前行下面。
                stage
                    .backend
                    .set_lyric_translation(Some("（演示）翻译行".into()));
            }),
            expect: &["（演示）翻译行"],
            absent: &[],
        },
        Step {
            name: "library-favorites",
            run: Box::new(|stage: &mut Stage| {
                stage.send(GuiRequest::RefreshLibrary);
                stage.ui.tab = Tab::Library;
                stage.ui.library_tab = voicefox_gui::ui::LibraryTab::Favorites;
            }),
            expect: &["收藏", "历史", "歌单", "夜曲"],
            absent: &[],
        },
        Step {
            name: "library-playlists",
            run: Box::new(|stage: &mut Stage| {
                stage.ui.library_tab = voicefox_gui::ui::LibraryTab::Playlists;
            }),
            expect: &["新建", "通勤歌单"],
            absent: &[],
        },
        Step {
            name: "sources-qr",
            run: Box::new(|stage: &mut Stage| {
                stage.ui.tab = Tab::Sources;
                stage.send(GuiRequest::StartLogin("kg".into()));
            }),
            expect: &["音源", "酷狗", "扫码登录"],
            absent: &[],
        },
        Step {
            name: "settings",
            run: Box::new(|stage: &mut Stage| {
                stage.ui.tab = Tab::Settings;
                stage.send(GuiRequest::RefreshSettings);
            }),
            expect: &[
                "设置",
                "音质",
                "音量",
                "下载音质",
                "外观",
                "深浅色",
                "快捷键",
            ],
            absent: &[],
        },
        Step {
            name: "settings-light",
            run: Box::new(|stage: &mut Stage| {
                stage.send(GuiRequest::SetAppearance("light".into()));
            }),
            expect: &["强制浅色"],
            absent: &[],
        },
        Step {
            name: "downloads",
            run: Box::new(|stage: &mut Stage| {
                stage.ui.tab = Tab::Downloads;
                let song = stage
                    .backend
                    .take_snapshot()
                    .search
                    .items
                    .first()
                    .cloned()
                    .expect("搜索结果");
                stage.send(GuiRequest::DownloadSong(song));
            }),
            expect: &["下载", "取消", "夜曲"],
            absent: &[],
        },
    ]
}

#[test]
fn storyboard_renders_every_main_state() {
    let backend = Arc::new(DemoBackend::new());
    let probe = egui::Context::default();
    voicefox_gui::install_cjk_font(&probe);
    let controller = VoicefoxGuiController::with_backend(&probe, Box::new(Arc::clone(&backend)));
    let theme = GuiTheme::from_config(&Config::default());
    let out_dir = std::env::var_os("VOICEFOX_GUI_SCREENSHOT").map(std::path::PathBuf::from);
    let stage = Stage {
        controller,
        ui: UiState::default(),
        theme,
        backend: Arc::clone(&backend),
    };

    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_context_setup(|ctx| {
            voicefox_gui::install_cjk_font(ctx);
            ctx.set_visuals(GuiTheme::from_config(&Config::default()).visuals());
        })
        .build_ui_state(
            |ui, stage: &mut Stage| {
                let snapshot: Snapshot = stage.backend.take_snapshot();
                stage.controller.apply_snapshot(snapshot);
                let theme = stage.theme;
                stage.ui.show_in(
                    ui,
                    &mut stage.controller,
                    theme,
                    &Arc::new(std::sync::Mutex::new(Vec::new())),
                );
            },
            stage,
        );

    for (number, step) in steps().into_iter().enumerate() {
        (step.run)(harness.state_mut());
        harness.run_steps(2);
        for label in step.expect {
            assert!(
                !harness
                    .get_all_by_label_contains(label)
                    .collect::<Vec<_>>()
                    .is_empty(),
                "步骤 {} 应出现「{label}」",
                step.name
            );
        }
        for label in step.absent {
            assert!(
                harness.query_by_label_contains(label).is_none(),
                "步骤 {} 不应出现「{label}」",
                step.name
            );
        }
        if let Some(dir) = out_dir.as_ref() {
            std::fs::create_dir_all(dir).expect("create screenshot dir");
            let image = harness.render().expect("render frame");
            let file = dir.join(format!("step-{}-{}.png", number + 1, step.name));
            image.save(&file).expect("save screenshot");
            image
                .save(dir.join(format!("latest-{}.png", step.name)))
                .expect("save screenshot alias");
            println!("wrote {}", file.display());
        }
    }
}

/// 队列拖动排序：界面发出的请求走服务层，索引校正与核心同一个规则。
#[test]
fn queue_reorder_request_is_forwarded_as_core_command() {
    let backend = Arc::new(DemoBackend::with_queue_preset(true));
    let mut controller = VoicefoxGuiController::with_backend(
        &egui::Context::default(),
        Box::new(Arc::clone(&backend)),
    );
    controller.send(GuiRequest::ReorderQueue { from: 0, to: 2 });
    let snapshot = backend.take_snapshot();
    assert_eq!(snapshot.queue.len(), 3);
    assert!(
        backend
            .requests()
            .iter()
            .any(|request| matches!(request, GuiRequest::ReorderQueue { from: 0, to: 2 })),
        "拖动排序必须以一条请求交给服务线程"
    );
}

/// 队列排序本身的行为（不依赖网络，直接检查演示后端的状态迁移）。
#[test]
fn queue_reorder_keeps_track_order() {
    let backend = Arc::new(DemoBackend::with_queue_preset(true));
    let before: Vec<String> = backend
        .take_snapshot()
        .queue
        .iter()
        .map(|song| song.name.clone())
        .collect();
    assert!(before.len() >= 3, "预设队列至少 3 首");
    backend
        .send(GuiRequest::ReorderQueue { from: 0, to: 2 })
        .expect("move request");
    let after: Vec<String> = backend
        .take_snapshot()
        .queue
        .iter()
        .map(|song| song.name.clone())
        .collect();
    assert_eq!(after[0], before[1]);
    assert_eq!(after[1], before[2]);
    assert_eq!(after[2], before[0]);
}

/// 播放模式按钮循环：列表循环 → 单曲循环 → 随机 → 顺序 → 播完停止。
#[test]
fn play_mode_cycles_from_the_player_bar() {
    let backend = Arc::new(DemoBackend::with_queue_preset(true));
    let controller = VoicefoxGuiController::with_backend(
        &egui::Context::default(),
        Box::new(Arc::clone(&backend)),
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_context_setup(|ctx| {
            voicefox_gui::install_cjk_font(ctx);
            ctx.set_visuals(GuiTheme::from_config(&Config::default()).visuals());
        })
        .build_ui_state(
            |ui, state: &mut (Arc<DemoBackend>, VoicefoxGuiController, UiState)| {
                let (backend, controller, ui_state) = state;
                let snapshot = backend.take_snapshot();
                controller.apply_snapshot(snapshot);
                let theme = GuiTheme::from_config(&Config::default());
                ui_state.show_in(
                    ui,
                    controller,
                    theme,
                    &Arc::new(std::sync::Mutex::new(Vec::new())),
                );
            },
            (Arc::clone(&backend), controller, UiState::default()),
        );
    harness.get_by_label("列表循环").click();
    harness.run_steps(2);
    let mode = backend.take_snapshot().play_mode;
    assert_eq!(mode, voicefox_runtime::media::PlayMode::SingleLoop);
}

/// 搜索框必须留得住用户的输入。
///
/// 回归用例：服务线程每 250ms 推一次快照，而 `apply_snapshot` 曾经无条件用
/// `snapshot.search.keyword` 回写搜索框，于是刚敲进去的字在下一帧就被清空，
/// 表现为「搜索框输入不了」。修法是只在服务端关键词真正变化时才同步。
#[test]
fn typing_in_the_search_box_survives_snapshot_updates() {
    let backend = Arc::new(DemoBackend::new());
    let probe = egui::Context::default();
    voicefox_gui::install_cjk_font(&probe);
    let controller = VoicefoxGuiController::with_backend(&probe, Box::new(Arc::clone(&backend)));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_context_setup(|ctx| {
            voicefox_gui::install_cjk_font(ctx);
            ctx.set_visuals(GuiTheme::from_config(&Config::default()).visuals());
        })
        .build_ui_state(
            |ui, state: &mut (Arc<DemoBackend>, VoicefoxGuiController, UiState)| {
                let (backend, controller, ui_state) = state;
                let snapshot = backend.take_snapshot();
                controller.apply_snapshot(snapshot);
                let theme = GuiTheme::from_config(&Config::default());
                ui_state.show_in(
                    ui,
                    controller,
                    theme,
                    &Arc::new(std::sync::Mutex::new(Vec::new())),
                );
            },
            (Arc::clone(&backend), controller, UiState::default()),
        );
    harness.run_steps(1);

    let field = harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text("周杰伦");

    // 多跑几帧，模拟服务线程持续推快照的那几秒。
    for _ in 0..4 {
        harness.run_steps(1);
        let value = harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .value();
        assert_eq!(
            value.as_deref(),
            Some("周杰伦"),
            "用户输入不应被快照回写清空"
        );
    }
}

/// 回归用例：结果行/队列行里的按钮必须收到真实指针点击。
///
/// 修复前整行 `ui.interact(ui.min_rect(), ...)` 注册在按钮**之后**，命中测试
/// 里盖住按钮，点「+」「×」全无反应（界面点不动）。修法是把整行可点区域
/// 用 `UiBuilder::sense` 在子控件之前注册。
#[test]
fn row_buttons_receive_real_pointer_clicks() {
    let backend = Arc::new(DemoBackend::new());
    let probe = egui::Context::default();
    voicefox_gui::install_cjk_font(&probe);
    let controller = VoicefoxGuiController::with_backend(&probe, Box::new(Arc::clone(&backend)));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .with_context_setup(|ctx| {
            voicefox_gui::install_cjk_font(ctx);
            ctx.set_visuals(GuiTheme::from_config(&Config::default()).visuals());
        })
        .build_ui_state(
            |ui, state: &mut (Arc<DemoBackend>, VoicefoxGuiController, UiState)| {
                let (backend, controller, ui_state) = state;
                let snapshot: Snapshot = backend.take_snapshot();
                controller.apply_snapshot(snapshot);
                let theme = GuiTheme::from_config(&Config::default());
                ui_state.show_in(
                    ui,
                    controller,
                    theme,
                    &Arc::new(std::sync::Mutex::new(Vec::new())),
                );
            },
            (Arc::clone(&backend), controller, UiState::default()),
        );

    // 先出搜索结果。
    harness
        .state_mut()
        .1
        .send(GuiRequest::Dispatch(ApplicationCommand::Search {
            keyword: "周杰伦".into(),
            source: None,
        }));
    harness.run_steps(2);

    // 1) 点「+」加入队列。
    let before = backend.take_snapshot().queue.len();
    harness
        .get_all_by_label("+")
        .next()
        .expect("「+」按钮")
        .click();
    harness.run_steps(2);
    let after_add = backend.take_snapshot().queue.len();
    assert!(
        after_add > before,
        "点「+」应当加入队列：before={before} after={after_add}"
    );

    // 2) 切到队列页，点「×」移除一首。
    harness.get_by_label("队列").click();
    harness.run_steps(2);
    let before_remove = backend.take_snapshot().queue.len();
    // 队列行里现在既有「×」也有拖动把手「≡」，按钮顺序随布局变化；
    // 这里按可访问名精确取「×」，避免误点到相邻控件。
    harness
        .get_all_by_label("×")
        .next()
        .expect("「×」按钮")
        .click();
    harness.run_steps(2);
    let requests = backend.requests();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            GuiRequest::Dispatch(ApplicationCommand::QueueRemove { .. })
        )),
        "点「×」应当发出 QueueRemove：{requests:?}"
    );
    let after_remove = backend.take_snapshot().queue.len();
    assert_eq!(
        after_remove,
        before_remove - 1,
        "点「×」应当移除一首：before={before_remove} after={after_remove}"
    );
}
