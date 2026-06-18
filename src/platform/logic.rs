//! platform 层的纯逻辑核心，无 Windows 依赖，单测主力。
//!
//! 把「可测的数学」从「不可测的 Win32 调用」里剥出来：增量计数聚合、
//! idle 时长计算、标题子串匹配、击键间隔记录。
//!
//! 生产路径（Win32SignalProvider）部分用原子直接算（见 input_hook），本模块的
//! Counts/IdleTracker/KeyIntervals 作为「纯逻辑可测基准」保留，单测覆盖其数学正确性，
//! 供未来扩展或对照验证。因此对未在非测试代码引用的项标注 allow(dead_code)。

#![allow(dead_code)]

use std::time::{Duration, Instant};

/// 一个 tick 内的增量事件计数（与 model 的 TickCounts 同构，但这里是
/// platform 内部聚合用，独立定义避免循环依赖）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub keys: u32,
    pub mouse: u32,
    pub switches: u32,
    pub backspaces: u32,
}

impl Counts {
    pub fn add_key(&mut self, is_backspace: bool) {
        self.keys += 1;
        if is_backspace {
            self.backspaces += 1;
        }
    }

    pub fn add_mouse_click(&mut self) {
        self.mouse += 1;
    }

    pub fn add_switch(&mut self) {
        self.switches += 1;
    }

    /// 合并另一份计数（hook 回调线程 → 主线程聚合时用）。
    pub fn merge(&mut self, other: &Counts) {
        self.keys += other.keys;
        self.mouse += other.mouse;
        self.switches += other.switches;
        self.backspaces += other.backspaces;
    }

    /// snapshot 后清零：每次 snapshot 报告的是「自上次以来的增量」。
    pub fn drain(&mut self) -> Counts {
        let out = *self;
        *self = Counts::default();
        out
    }
}

/// 基于「最后一次输入时间戳」的空闲计算。时间来源由调用方注入（生产用
/// `Instant::now()`，测试用 fake），保证纯逻辑可测。
///
/// 注：生产路径（Win32SignalProvider）用 hook 的 `last_input_ms` 原子直接算 idle，
/// 本结构作为「纯逻辑可测基准」保留（单测覆盖 idle 数学正确性）。
#[derive(Debug, Clone)]
pub struct IdleTracker {
    last_input: Instant,
}

impl IdleTracker {
    pub fn new(now: Instant) -> Self {
        Self { last_input: now }
    }

    /// 任意输入到来时调用，刷新时间戳（spec §6.5：任何键鼠都重置空闲）。
    pub fn note_input(&mut self, now: Instant) {
        if now >= self.last_input {
            self.last_input = now;
        }
        // now < last_input（时钟回拨）时保持原值，保守不倒退。
    }

    pub fn idle_since(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_input)
    }
}

/// 标题子串匹配。大小写不敏感。空 target 视为「无目标」→ 永不 on_target。
pub fn title_matches(title: &str, target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    title.to_lowercase().contains(&target.to_lowercase())
}

/// 记录击键时间戳，产出相邻间隔（毫秒）。snapshot 后排空。
/// 上限保留最近 `MAX_KEYS` 个时间戳以防内存膨胀。
pub struct KeyIntervals {
    times: Vec<Instant>,
}

const MAX_KEYS: usize = 64;

impl KeyIntervals {
    pub fn new() -> Self {
        Self {
            times: Vec::with_capacity(MAX_KEYS),
        }
    }

    pub fn push(&mut self, t: Instant) {
        if self.times.len() >= MAX_KEYS {
            // 满了就丢最旧，保持滑动窗口语义。
            self.times.remove(0);
        }
        self.times.push(t);
    }

    /// 计算相邻击键间隔（毫秒），排空内部缓冲。
    pub fn drain_intervals_ms(&mut self) -> Vec<f64> {
        let n = self.times.len();
        let mut out = Vec::with_capacity(n.saturating_sub(1));
        for w in self.times.windows(2) {
            let ms = w[1].saturating_duration_since(w[0]).as_secs_f64() * 1000.0;
            out.push(ms);
        }
        self.times.clear();
        out
    }
}

impl Default for KeyIntervals {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_add_and_drain() {
        let mut c = Counts::default();
        c.add_key(false);
        c.add_key(true); // backspace 也算 key
        c.add_mouse_click();
        c.add_switch();
        assert_eq!(c.keys, 2);
        assert_eq!(c.backspaces, 1);
        assert_eq!(c.mouse, 1);
        assert_eq!(c.switches, 1);
        let drained = c.drain();
        assert_eq!(drained.keys, 2);
        assert_eq!(c, Counts::default()); // 清零
    }

    #[test]
    fn counts_merge() {
        let mut a = Counts {
            keys: 1,
            mouse: 2,
            switches: 3,
            backspaces: 4,
        };
        let b = Counts {
            keys: 10,
            mouse: 20,
            switches: 30,
            backspaces: 40,
        };
        a.merge(&b);
        assert_eq!(a.keys, 11);
        assert_eq!(a.mouse, 22);
        assert_eq!(a.switches, 33);
        assert_eq!(a.backspaces, 44);
    }

    #[test]
    fn idle_tracker_basic() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        // 无输入 5s
        assert_eq!(
            idle.idle_since(t0 + Duration::from_secs(5)),
            Duration::from_secs(5)
        );
        // 第 3s 有输入
        idle.note_input(t0 + Duration::from_secs(3));
        // 第 7s 查：自第 3s 起 4s
        assert_eq!(
            idle.idle_since(t0 + Duration::from_secs(7)),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn idle_tracker_clock_back_safe() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        // 时钟回拨：不更新，保守保持原 last_input
        idle.note_input(t0 - Duration::from_secs(10));
        assert_eq!(idle.last_input, t0);
    }

    #[test]
    fn title_match_case_insensitive_substring() {
        assert!(title_matches(
            "Visual Studio Code - main.rs",
            "visual studio code"
        ));
        assert!(title_matches("VISUAL STUDIO CODE", "visual studio code"));
        assert!(!title_matches("Visual Studio", "visual studio code"));
        // 空 target 永不匹配
        assert!(!title_matches("anything", ""));
    }

    #[test]
    fn key_intervals_basic() {
        let t0 = Instant::now();
        let mut k = KeyIntervals::new();
        k.push(t0);
        k.push(t0 + Duration::from_millis(200));
        k.push(t0 + Duration::from_millis(500)); // 距上一次 300ms
        let v = k.drain_intervals_ms();
        assert_eq!(v.len(), 2);
        assert!((v[0] - 200.0).abs() < 1.0);
        assert!((v[1] - 300.0).abs() < 1.0);
    }

    #[test]
    fn key_intervals_empty_single() {
        let mut k = KeyIntervals::new();
        assert!(k.drain_intervals_ms().is_empty());
        k.push(Instant::now());
        assert!(k.drain_intervals_ms().is_empty()); // 单个无间隔
    }

    #[test]
    fn key_intervals_caps_history() {
        let mut k = KeyIntervals::new();
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS + 10) {
            k.push(t0 + Duration::from_millis(i as u64));
        }
        // 排空后不应 panic，且内部窗口被限
        let v = k.drain_intervals_ms();
        assert!(v.len() <= MAX_KEYS);
    }
}
