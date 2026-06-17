use std::collections::VecDeque;

use super::config::ModelConfig;
use super::types::TickCounts;

#[derive(Debug, Clone)]
pub struct SignalBuffer {
    window_secs: f64,
    events: VecDeque<(f64, TickCounts)>,
    intervals: VecDeque<(f64, f64)>, // (t_secs, interval_ms)
}

impl SignalBuffer {
    pub fn new(window_secs: f64) -> Self {
        Self {
            window_secs,
            events: VecDeque::new(),
            intervals: VecDeque::new(),
        }
    }

    pub fn push(&mut self, now: f64, counts: TickCounts, intervals_ms: &[f64]) {
        for &ms in intervals_ms {
            self.intervals.push_back((now, ms));
        }
        self.events.push_back((now, counts));
        self.trim(now);
    }

    fn trim(&mut self, now: f64) {
        let cutoff = now - self.window_secs;
        while let Some(&(t, _)) = self.events.front() {
            if t < cutoff {
                self.events.pop_front();
            } else {
                break;
            }
        }
        while let Some(&(t, _)) = self.intervals.front() {
            if t < cutoff {
                self.intervals.pop_front();
            } else {
                break;
            }
        }
    }

    fn span_secs(&self, now: f64) -> f64 {
        match self.events.front() {
            Some(&(t, _)) => (now - t).max(1e-6),
            None => 1e-6,
        }
    }

    fn sums(&self) -> (u32, u32, u32) {
        let mut keys = 0u32;
        let mut switches = 0u32;
        let mut backspaces = 0u32;
        for (_, c) in &self.events {
            keys += c.keys;
            switches += c.switches;
            backspaces += c.backspaces;
        }
        (keys, switches, backspaces)
    }

    pub fn total_keys(&self) -> u32 {
        self.sums().0
    }

    pub fn switch_rate_per_min(&self, now: f64) -> f64 {
        let switches = self.sums().1 as f64;
        let mins = self.span_secs(now) / 60.0;
        switches / mins
    }

    pub fn error_rate(&self) -> f64 {
        let (keys, _, backspaces) = self.sums();
        if keys == 0 {
            0.0
        } else {
            backspaces as f64 / keys as f64
        }
    }

    pub fn typing_jitter_ms(&self) -> f64 {
        let n = self.intervals.len();
        if n < 2 {
            return 0.0;
        }
        let mean = self.intervals.iter().map(|(_, ms)| ms).sum::<f64>() / n as f64;
        let var = self
            .intervals
            .iter()
            .map(|(_, ms)| (ms - mean).powi(2))
            .sum::<f64>()
            / n as f64;
        var.sqrt()
    }

    /// 输入越平稳（jitter 越小）→ 值越低（专注减速）。无输入时取 focus_max（中性）。
    pub fn focus_factor(&self, cfg: &ModelConfig) -> f64 {
        if self.total_keys() == 0 {
            return cfg.focus_max;
        }
        let norm = (self.typing_jitter_ms() / cfg.jitter_max_ms).clamp(0.0, 1.0);
        cfg.focus_min + (cfg.focus_max - cfg.focus_min) * norm
    }

    /// 切窗/抖动/错误越高 → 值越高（疲劳加速），夹在 [fatigue_min, fatigue_max]。
    pub fn fatigue_factor(&self, now: f64, cfg: &ModelConfig) -> f64 {
        let switch_norm = (self.switch_rate_per_min(now) / cfg.switch_max_per_min).clamp(0.0, 1.0);
        let jitter_norm = (self.typing_jitter_ms() / cfg.jitter_max_ms).clamp(0.0, 1.0);
        let error_norm = (self.error_rate() / cfg.error_max).clamp(0.0, 1.0);
        let raw = 1.0 + 0.5 * switch_norm + 0.3 * jitter_norm + 0.5 * error_norm;
        raw.clamp(cfg.fatigue_min, cfg.fatigue_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ModelConfig {
        ModelConfig::default()
    }

    #[test]
    fn empty_buffer_is_neutral() {
        let b = SignalBuffer::new(60.0);
        let c = cfg();
        assert!((b.focus_factor(&c) - c.focus_max).abs() < 1e-9);
        assert!((b.fatigue_factor(0.0, &c) - c.fatigue_min).abs() < 1e-9);
        assert_eq!(b.error_rate(), 0.0);
        assert_eq!(b.typing_jitter_ms(), 0.0);
    }

    #[test]
    fn steady_typing_lowers_focus_factor() {
        let mut b = SignalBuffer::new(60.0);
        let c = cfg();
        for t in 0..10 {
            b.push(t as f64, TickCounts { keys: 4, ..Default::default() }, &[200.0, 200.0]);
        }
        assert!((b.focus_factor(&c) - c.focus_min).abs() < 1e-9);
    }

    #[test]
    fn erratic_typing_raises_focus_and_fatigue() {
        let mut b = SignalBuffer::new(60.0);
        let c = cfg();
        b.push(
            0.0,
            TickCounts { keys: 10, backspaces: 5, switches: 20, ..Default::default() },
            &[100.0, 500.0],
        );
        let f = b.focus_factor(&c);
        assert!(f > c.focus_min);
        let fat = b.fatigue_factor(1.0, &c);
        assert!((fat - c.fatigue_max).abs() < 1e-9);
    }

    #[test]
    fn window_evicts_old_samples() {
        let mut b = SignalBuffer::new(10.0);
        b.push(0.0, TickCounts { keys: 100, ..Default::default() }, &[]);
        b.push(20.0, TickCounts { keys: 1, ..Default::default() }, &[]);
        assert_eq!(b.total_keys(), 1);
    }
}
