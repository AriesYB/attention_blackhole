//! Plan 2 验证用：连真实 Windows hook 跑 ~30 秒，每秒打印信号，证明 platform 层端到端走通。
//!
//! Run: cargo run --example platform_demo [target_title]
//! （会触发真实键鼠 hook + 前台窗口查询；按 Ctrl+C 提前结束）
//!
//! 必须在**前台终端**运行才能捕获键盘事件——后台/无 tty 的进程拿不到键盘 hook 派发。
//! 不含自动断言——hook 需要真实桌面会话，无法在无头 CI 跑。
//! 手工 QA 清单见 docs/superpowers/plans/2026-06-18-...-plan-2-platform.md Task 5。

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::thread;
    use std::time::{Duration, Instant};

    use attention_blackhole::controller::SignalProvider;
    use attention_blackhole::platform::Win32SignalProvider;

    // 默认目标窗口标题子串；可通过命令行第一个参数覆盖。
    let target = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Visual Studio Code".into());

    println!("platform_demo starting");
    println!("  target window title contains: {:?}", target);
    println!("  go type / click / switch windows for 30s to see counts");
    println!("  (Ctrl+C to stop early)");
    println!();

    let mut prov = Win32SignalProvider::new(&target)?;

    // 等 hook 线程就绪（diag 到 4=进入泵），最多等 1s。
    // 这一行保留：它是确认 hook 真正装上的最直接信号（4=键盘+鼠标 hook 就绪+进泵）。
    let t0 = Instant::now();
    loop {
        let d = prov.hook_diag();
        if d >= 4 || d == 99 || t0.elapsed() > Duration::from_secs(1) {
            println!(
                "  hook thread diag = {} (4=ready, 99=failed)",
                d
            );
            if d != 4 {
                eprintln!("  WARNING: hook not ready (diag={}); counts will be 0.", d);
            }
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    println!();
    println!(
        "{:>3} | {:>4} {:>5} {:>2} {:>2} | {:>8} | {:>8} | {:>5}",
        "sec", "keys", "mouse", "sw", "bk", "idle_ms", "on_targ", "intv"
    );
    println!("{}", "-".repeat(60));

    let dt = Duration::from_millis(1000); // snapshot 的 dt 参数（告诉 provider 这一拍多长）
    let start = Instant::now();

    for sec in 0..30u64 {
        let input = prov.snapshot(dt);
        // 击键间隔取平均（若有），便于一眼看出节奏；空则显示 -。
        let avg_interval = if input.key_intervals_ms.is_empty() {
            String::from("  -  ")
        } else {
            let avg =
                input.key_intervals_ms.iter().sum::<f64>() / input.key_intervals_ms.len() as f64;
            format!("{:>5.0}", avg)
        };
        println!(
            "{:>3} | {:>4} {:>5} {:>2} {:>2} | {:>8} | {:>8} | {}",
            sec,
            input.counts.keys,
            input.counts.mouse,
            input.counts.switches,
            input.counts.backspaces,
            input.idle.as_millis(),
            input.on_target,
            avg_interval,
        );
        // 关键：每拍后 sleep 到下一秒边界，让循环按 ~1Hz 跑（而非全速空转）。
        let next = start + Duration::from_secs(sec + 1);
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        }
        if start.elapsed() >= Duration::from_secs(30) {
            break;
        }
    }

    println!("{}", "-".repeat(60));
    println!("done. Drop will unhook + join the message-pump thread.");
    // prov 在此 drop —— 验证 Drop 干净卸 hook 不卡死。
    drop(prov);
    println!("clean exit.");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("platform_demo is Windows-only (platform/ module is cfg(windows)).");
}
