//! 真实屏幕捕获黑洞演示：黑洞会**扭曲你真实的桌面内容**（不是程序化网格）。
//!
//! Run: cargo run --example capture_demo
//! （弹出全屏置顶透明 overlay，捕获整个主显示器 → 经引力透镜扭曲 → 你当前的桌面
//! 窗口/代码在洞周明显弯曲。约 25 秒；按 Ctrl+C 提前结束）
//!
//! 关键机制（spec §8.1/§8.3 + §9 隐私）：
//! - WGC 捕获**整个主显示器**（`CaptureTarget::Monitor`），零拷贝直采显存纹理。
//! - overlay 自身已 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 自排除（overlay.rs），
//!   破「捕获包含 overlay → 无限自指」反馈环——故捕获纹理 = overlay **背后**的真实桌面。
//! - shader `u_has_capture=1` 分支：用弯曲后的坐标采样捕获纹理 → 桌面像素被引力透镜扭曲。
//! - 隐私硬规则：捕获像素只在显存，绝不拷回 RAM/落盘/外传。
//!
//! 必须在**真实桌面会话**运行（Win10 2004+ WGC + D3D11 GPU + DWM 合成）。无头 CI 无法验证。
//!
//! 你会看到：黑洞中心的代码/窗口被弯曲成弧、爱因斯坦环区域桌面被镜像翻转拉伸、
//! 视界内被吞噬（变黑）。这是 spec §8.1「吞噬你的代码」的真实实现。
//!
//! 手工 QA 清单：
//! 1. 洞周的桌面图标/文字明显弯曲（引力透镜），不是程序化网格。
//! 2. 远场桌面清晰（仅轻微弱场偏折），近场强烈扭曲。
//! 3. 视界内变黑（吞噬），光子环附近桌面被拉伸成细环。
//! 4. overlay 不捕获自身（无反馈环导致的闪烁/重复图案）。
//! 5. 切换/移动其它窗口 → 捕获纹理实时更新（~30fps）。

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::time::{Duration, Instant};

    use attention_blackhole::controller::{Frame, Renderer};
    use attention_blackhole::model::state::AppState;
    use attention_blackhole::renderer::capture::CaptureTarget;
    use attention_blackhole::renderer::D3D11Renderer;
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTOPRIMARY};
    use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

    println!("capture_demo starting (真实屏幕捕获黑洞)");
    println!("  捕获整个主显示器 → 引力透镜扭曲你的真实桌面");
    println!("  overlay 已 WDA_EXCLUDEFROMCAPTURE 自排除（破反馈环）");
    println!("  把一些窗口/文字放在屏幕上，观察它们在洞周被弯曲");
    println!();

    // 取主显示器 HMONITOR：从桌面窗口查所在显示器。
    let desktop = unsafe { GetDesktopWindow() };
    let hmon = unsafe { MonitorFromWindow(desktop, MONITOR_DEFAULTTOPRIMARY) };
    if hmon.is_invalid() {
        eprintln!("无法获取主显示器 HMONITOR");
        std::process::exit(1);
    }

    // consent=true + Monitor 捕获：激活真实屏幕捕获。
    let mut renderer = match D3D11Renderer::new(Some(CaptureTarget::Monitor(hmon)), true) {
        Ok(r) => {
            println!("  D3D11Renderer constructed OK (overlay + WGC monitor capture + geodesic shader)");
            r
        }
        Err(e) => {
            eprintln!("D3D11Renderer construction FAILED: {:?}", e);
            eprintln!("  (常见原因：无 GPU/驱动、旧 OS(< Win10 2004)、无桌面会话)");
            std::process::exit(1);
        }
    };

    // ~25 秒：load 从 0 缓慢爬到 ~78（压暗阈值 80 之下），再回落。
    // on_target=true 全程：保持捕获激活（painter 设 u_has_capture=1）。
    let duration = Duration::from_secs(25);
    let ramp_up = 18.0_f32;
    let peak = 78.0_f32;
    let start = Instant::now();
    let mut last_report = Instant::now();

    while start.elapsed() < duration {
        let t = start.elapsed().as_secs_f32();
        let load = if t < ramp_up {
            let s = t / ramp_up;
            peak * (s * s * (3.0 - 2.0 * s))
        } else {
            let s = (t - ramp_up) / (duration.as_secs_f32() - ramp_up);
            peak * (1.0 - s).max(0.0)
        };
        // state 固定 Working：避免 Dimming(u_dim=0.6) 压暗掩盖透镜效果。
        let state = AppState::Working;
        // on_target=true：激活捕获会话，u_has_capture=1 → shader 采样捕获纹理。
        let frame = Frame {
            load: load as f64,
            state,
            on_target: true,
        };
        renderer.render(&frame);

        if last_report.elapsed() >= Duration::from_secs(1) {
            println!(
                "  t={:>5.1}s  load={:>5.1}  state={:?}  on_target=true (真实桌面被扭曲中...)",
                t,
                load,
                state
            );
            last_report = Instant::now();
        }

        // 推帧节奏：10Hz（与 controller 的 tick 一致），渲染线程自行 60fps。
        std::thread::sleep(Duration::from_millis(100));
    }

    println!("capture_demo done (renderer Drop 停捕获 + 拆 overlay + 线程)");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("capture_demo is Windows-only (D3D11 + WGC).");
    std::process::exit(2);
}
