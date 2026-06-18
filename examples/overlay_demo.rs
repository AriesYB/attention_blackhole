//! Plan 3 验证用：连真实 D3D11 + overlay + 黑洞 shader 跑 ~15 秒，证明 renderer 端到端走通。
//!
//! Run: cargo run --example overlay_demo
//! （会弹出全屏透明置顶窗口，黑洞 shader 动画 ~15 秒；按 Ctrl+C 提前结束）
//!
//! 默认 consent=false → 走程序化背景（spec §9 默认安全）。黑洞视界半径随模拟的
//! attention load 振荡（0..100 正弦），可直观看到「黑洞涨缩吞噬」效果。
//!
//! 必须在**真实桌面会话**运行——需要 D3D11 GPU + DWM 合成 + overlay 窗口可见。
//! 无头 CI 无法验证（无显示器/GPU）。不含自动断言——视觉效果需人眼 QA。
//!
//! 手工 QA 清单（spec §8.3 DoD）：
//! 1. 弹出全屏窗口覆盖主显示器，永远置顶。
//! 2. 窗口透明：黑洞外区域可见桌面/其它窗口（点击穿透）。
//! 3. 黑洞视界随 load 振荡涨缩（中心黑圆变大变小）。
//! 4. 吸积盘旋转 + 光子环高亮。
//! 5. 无崩溃、无明显撕裂（VSync）。
//! 6. 退出后窗口消失（Drop 干净）。

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::time::{Duration, Instant};

    use attention_blackhole::controller::{Frame, Renderer};
    use attention_blackhole::model::state::AppState;
    use attention_blackhole::renderer::D3D11Renderer;

    println!("overlay_demo starting");
    println!("  consent=false -> procedural background (spec §9 default safe)");
    println!("  black hole horizon will oscillate with simulated load (0..100 sine)");
    println!("  watch for ~15s; Ctrl+C to stop early");
    println!();

    // consent=false：默认安全，不捕获真实屏幕。target=None：纯程序化。
    let mut renderer = match D3D11Renderer::new(None, false) {
        Ok(r) => {
            println!("  D3D11Renderer constructed OK (overlay + device + shaders + procedural capture)");
            r
        }
        Err(e) => {
            // eprintln 到 stderr 让 demo 可在脚本里捕获失败。
            eprintln!("D3D11Renderer construction FAILED: {:?}", e);
            eprintln!("  (常见原因：无 GPU/驱动、旧 OS、无桌面会话)");
            std::process::exit(1);
        }
    };

    // 模拟 ~15 秒：attention load 按正弦振荡 0..100，state 随 load 切换。
    let duration = Duration::from_secs(15);
    let start = Instant::now();
    let mut last_report = Instant::now();

    while start.elapsed() < duration {
        let t = start.elapsed().as_secs_f32();
        // load：0..100 正弦，周期 ~4 秒（直观可见的涨缩节奏）。
        let load = 50.0 + 50.0 * (t * std::f32::consts::TAU / 4.0).sin();
        // state：load>80 → Dimming，让黑洞同时被压暗（展示 u_dim 通道）。
        let state = if load > 80.0 {
            AppState::Dimming
        } else {
            AppState::Working
        };
        // on_target=false：capture 永不激活（程序化背景）。
        let frame = Frame {
            load: load as f64,
            state,
            on_target: false,
        };
        renderer.render(&frame);

        // 每秒打印一次进度（证明 render 循环在推帧、未阻塞）。
        if last_report.elapsed() >= Duration::from_secs(1) {
            println!(
                "  t={:>5.1}s  load={:>5.1}  state={:?}  (rendering...)",
                t,
                load,
                state
            );
            last_report = Instant::now();
        }

        // 推帧节奏：10Hz（与 controller 的 tick 一致），渲染线程自行 60fps。
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("overlay_demo done (renderer Drop will tear down overlay + threads)");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("overlay_demo is Windows-only (D3D11 + WGC).");
    std::process::exit(2);
}
