//! 物理测地线黑洞演示：对比 overlay_demo（旧手绘黑洞），这个用新移植的
//! Schwarzschild geodesic shader（每个近场像素逐帧积分自己的零测地线）。
//!
//! Run: cargo run --example geodesic_demo
//! （弹出全屏透明置顶窗口，物理黑洞动画 ~25 秒；按 Ctrl+C 提前结束）
//!
//! 默认 consent=false → 程序化星云背景（spec §9 默认安全）。可见的物理特征：
//! - 光子环：视界边缘极亮细环（光子绕视界多圈聚焦，是物理正确性的强指示器）。
//! - 引力透镜：背景星点/星云在洞周围抹成弧（远场解析弱场偏折 + 近场测地线）。
//! - 卡冈图雅式吸积盘：盘远侧弧越过视界上下方，盘上多次穿越，多普勒半亮半暗 +
//!   色温黑体（内热蓝白→外冷暗红）+ 螺旋旋臂 + 引力时间膨胀（内圈冻结）。
//!
//! load 曲线：0 → 缓慢爬到 100（约 18 秒，给足时间观察涨缩各阶段）→ 回落。
//!
//! 必须在**真实桌面会话**运行（需 D3D11 GPU + DWM 合成 + overlay 可见）。无头 CI 无法验证。
//!
//! 手工 QA 清单：
//! 1. 视界随 load 涨缩（中心黑圆），load 越高越大越压迫。
//! 2. 光子环清晰可见（物理正确性的关键标志）。
//! 3. 吸积盘远侧弧越过视界上方/下方（星际穿越式），非简单椭圆环。
//! 4. 盘半亮半暗（多普勒 beaming）+ 螺旋旋臂旋转。
//! 5. 远处桌面透出（alpha 包络：远场透明），视界附近遮挡。
//! 6. 无崩溃、无明显撕裂（VSync），退出后窗口消失。

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::time::{Duration, Instant};

    use attention_blackhole::controller::{Frame, Renderer};
    use attention_blackhole::model::state::AppState;
    use attention_blackhole::renderer::D3D11Renderer;

    println!("geodesic_demo starting (物理测地线黑洞)");
    println!("  consent=false -> procedural background (spec §9 default safe)");
    println!("  watch for ~25s: load ramps 0->100->0, geodesic photon ring + lensed disk");
    println!("  compare with `overlay_demo` (old hand-painted black hole)");
    println!();

    // consent=false：默认安全，不捕获真实屏幕。target=None：纯程序化。
    let mut renderer = match D3D11Renderer::new(None, false) {
        Ok(r) => {
            println!("  D3D11Renderer constructed OK (overlay + device + geodesic shader + procedural capture)");
            r
        }
        Err(e) => {
            eprintln!("D3D11Renderer construction FAILED: {:?}", e);
            eprintln!("  (常见原因：无 GPU/驱动、旧 OS、无桌面会话)");
            std::process::exit(1);
        }
    };

    // ~25 秒：load 从 0 缓慢爬到 ~78（压暗阈值 80 之下，确保 u_dim=0，看得见真实盘光），
    // 再回落。注意：load>80 会触发 AppState::Dimming → u_dim=0.6 → 整体压暗到 58%，
    // 会掩盖盘的真实质感，故本演示特意停在阈值下方便观察物理盘。
    let duration = Duration::from_secs(25);
    let ramp_up = 18.0_f32;
    let peak = 78.0_f32;
    let start = Instant::now();
    let mut last_report = Instant::now();

    while start.elapsed() < duration {
        let t = start.elapsed().as_secs_f32();
        // load 曲线：0→peak 缓慢爬升（ramp_up 秒内），到顶后回落。
        let load = if t < ramp_up {
            // smoothstep 缓动爬升，给足时间观察各 load 档位的物理细节。
            let s = t / ramp_up;
            peak * (s * s * (3.0 - 2.0 * s))
        } else {
            // 回落段：在剩余时间内降到 0。
            let s = (t - ramp_up) / (duration.as_secs_f32() - ramp_up);
            peak * (1.0 - s).max(0.0)
        };
        // state 固定 Working：避免 Dimming(u_dim=0.6) 压暗掩盖盘真实质感。
        // （Dimming 是独立功能，可在真实应用中按注意力负荷触发。）
        let state = AppState::Working;
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
                "  t={:>5.1}s  load={:>5.1}  state={:?}  (geodesic rendering...)",
                t,
                load,
                state
            );
            last_report = Instant::now();
        }

        // 推帧节奏：10Hz（与 controller 的 tick 一致），渲染线程自行 60fps。
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("geodesic_demo done (renderer Drop will tear down overlay + threads)");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("geodesic_demo is Windows-only (D3D11 + WGC).");
    std::process::exit(2);
}
