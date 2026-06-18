//! Windows 平台信号源实现（spec §4.1 platform/）。
//!
//! 仅 `cfg(windows)` 编译。`controller` 通过 `&mut dyn SignalProvider` 消费，
//! 本模块不反向依赖 controller。privacy 硬规则：所有 hook 回调只计数、只记时间戳，
//! 绝不保留按键内容（vkCode 除外比较 VK_BACK 这一个常量）。见 spec §9。

mod logic;

#[cfg(windows)]
mod input_hook;
#[cfg(windows)]
mod window_tracker;

#[cfg(windows)]
pub use window_tracker::WindowTracker;

use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::controller::SignalProvider;

/// Windows 平台 `SignalProvider`。组装 InputHook + WindowTracker，把它们的原始
/// 信号聚合成 `TickInput` 喂给 controller。
///
/// 非 windows 平台上不构造（`new` 仅 `cfg(windows)` 暴露）。
#[cfg(windows)]
pub struct Win32SignalProvider {
    hook: input_hook::InputHook,
    window: WindowTracker,
    /// ms 基准时刻：与 hook 内部的 start 近似同时（new 里先记 start 再 spawn hook）。
    /// hook 回调把每次输入的 `elapsed_ms` 写进 last_input_ms，snapshot 据此算 idle。
    start: Instant,
}

#[cfg(windows)]
impl Win32SignalProvider {
    /// 启动 hook 线程并初始化各子组件。失败返回 io::Error（hook 装不上）。
    pub fn new(target_window_title_contains: impl Into<String>) -> std::io::Result<Self> {
        let start = Instant::now();
        let hook = input_hook::InputHook::new()?;
        Ok(Self {
            hook,
            window: WindowTracker::new(target_window_title_contains),
            start,
        })
    }

    /// 诊断：返回 hook 线程的执行进度码（供 demo 排查为何不计数）。
    /// 0 未启 / 1 已注入状态 / 2 键盘hook装上 / 3 鼠标hook装上 / 4 进入泵 / 99 失败。
    pub fn hook_diag(&self) -> u32 {
        self.hook.diag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 诊断：返回回调被系统调用的总次数（key+mouse 不分类型）。
    /// 用于隔离「回调没触发」vs「thread_local 读不到」。
    pub fn cb_fired(&self) -> i64 {
        input_hook::CB_FIRED.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 诊断：返回 key_proc 被调用的次数（隔离键盘回调是否触发）。
    pub fn key_cb(&self) -> i64 {
        input_hook::KEY_CB.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(windows)]
impl SignalProvider for Win32SignalProvider {
    fn snapshot(&mut self, dt: Duration) -> crate::model::types::TickInput {
        let now = Instant::now();

        // 1. 从 hook 线程拉取增量计数（原子 drain，自上次 snapshot 起）。
        let mut inc = self.hook.counts.drain();

        // 2. 查前台窗口：on_target + 切窗检测（switch 计入本 tick 增量）。
        let (on_target, switched) = self.window.poll();
        if switched {
            inc.add_switch();
        }

        // 3. 空闲：hook 的 last_input_ms 是「最后一次输入相对 start 的 ms」，
        //    当前时刻相对 start 的 ms 减去它 = idle。无输入时 last_input_ms=0，
        //    idle 即「自 start 起全程无输入」（合理初值）。
        let now_ms = now.duration_since(self.start).as_millis() as i64;
        let last_ms = self.hook.last_input_ms.load(std::sync::atomic::Ordering::Relaxed);
        let idle_ms = (now_ms - last_ms).max(0) as u64;
        let idle = Duration::from_millis(idle_ms);

        // 4. 击键间隔：从 hook 线程的 key_times 缓冲 drain，转成相邻 ms。
        let key_intervals_ms = {
            let mut guard = self.hook.key_times.lock().unwrap();
            let times = std::mem::take(&mut *guard);
            drop(guard);
            intervals_from(&times)
        };

        crate::model::types::TickInput {
            counts: crate::model::types::TickCounts {
                keys: inc.keys,
                mouse: inc.mouse,
                switches: inc.switches,
                backspaces: inc.backspaces,
            },
            on_target,
            idle,
            dt,
            key_intervals_ms,
        }
    }
}

/// 由击键时间戳序列算相邻间隔（毫秒）。与 logic::KeyIntervals 同语义。
#[cfg(windows)]
fn intervals_from(times: &[Instant]) -> Vec<f64> {
    let mut out = Vec::with_capacity(times.len().saturating_sub(1));
    for w in times.windows(2) {
        let ms = w[1].saturating_duration_since(w[0]).as_secs_f64() * 1000.0;
        out.push(ms);
    }
    out
}
