//! 桌面版入口：启动 egui 窗口并连上 voicefox 核心。
//!
//! `cargo run -p voicefox-gui` 打开窗口；配置与播放历史和 TUI 共用
//! （`~/.config/voicefox/config.toml` 与 runtime 的存储层）。

fn main() {
    if let Err(error) = run() {
        eprintln!("voicefox GUI 启动失败: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // 播放器（libmpv 事件 pump）与媒体会话依赖 tokio 运行时：在启动线程上先建
    // 一个，运行期间始终 `enter`，之后 GUI 服务线程自己再建一个跑事件循环。
    let (config, path) = voicefox_runtime::load_config()?;
    let bootstrap = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("voicefox-gui-bootstrap")
        .build()?;
    let _guard = bootstrap.enter();
    let runtime = voicefox_runtime::ApplicationRuntime::desktop(config.clone(), path)?;
    voicefox_gui::run(config, runtime)?;
    Ok(())
}
