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
            expect: &["清空", "调整顺序"],
            absent: &[],
        },
        Step {
            name: "queue-reordered",
            run: Box::new(|stage: &mut Stage| {
                // 拖动排序：把第 1 首挪到第 3 位。
                stage.send(GuiRequest::MoveQueue { from: 0, to: 2 });
            }),
            expect: &[],
            absent: &[],
        },
        Step {
            name: "paused-lyrics",
            run: Box::new(|stage: &mut Stage| {
                stage.command(ApplicationCommand::Toggle);
                stage.ui.tab = Tab::Lyrics;
            }),
            expect: &["已暂停", "歌词"],
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
        .send(GuiRequest::MoveQueue { from: 0, to: 2 })
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
