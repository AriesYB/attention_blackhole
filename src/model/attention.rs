use crate::model::config::ModelConfig;
use crate::model::signal_buffer::SignalBuffer;
use crate::model::types::TickInput;

#[derive(Debug, Clone)]
pub struct AttentionModel {
    load: f64,
    clock: f64,
    buffer: SignalBuffer,
    config: ModelConfig,
}

impl AttentionModel {
    pub fn new(config: ModelConfig) -> Self {
        let window_secs = config.window.as_secs_f64();
        Self {
            load: 0.0,
            clock: 0.0,
            buffer: SignalBuffer::new(window_secs),
            config,
        }
    }

    pub fn load(&self) -> f64 {
        self.load
    }

    /// 真正的「冷启动」重置：清零 load、时钟与信号历史。
    /// 这样手动「我休息好了」后，疲劳/专注因子不会带着休息前的陈旧数据。
    pub fn reset(&mut self) {
        self.load = 0.0;
        self.clock = 0.0;
        self.buffer.clear();
    }

    /// 把 load 强制封顶到 max（force 关闭时用，封顶 99% 等效）。
    pub fn cap_at(&mut self, max: f64) {
        if self.load > max {
            self.load = max;
        }
    }

    /// 推进一个 tick，返回最新 load。
    pub fn tick(&mut self, input: &TickInput) -> f64 {
        let dt = input.dt.as_secs_f64();
        self.clock += dt;
        self.buffer
            .push(self.clock, input.counts, &input.key_intervals_ms);

        let idle = input.idle.as_secs_f64() > self.config.idle_grace.as_secs_f64();
        let active = input.counts.total_input() > 0;

        if input.on_target && active && !idle {
            // 增长：基线 × 专注减速 × 疲劳加速
            let focus = self.buffer.focus_factor(&self.config);
            let fatigue = self.buffer.fatigue_factor(self.clock, &self.config);
            let dl = self.config.growth_per_min * focus * fatigue * (dt / 60.0);
            self.load = (self.load + dl).min(100.0);
        } else if idle {
            // 空闲缩小（不论在不在目标）
            let dl = self.config.idle_shrink_per_min * (dt / 60.0);
            self.load = (self.load - dl).max(0.0);
        } else if !input.on_target && active {
            // 离开目标但仍活跃：缓慢缩小
            let dl = self.config.offtarget_shrink_per_min * (dt / 60.0);
            self.load = (self.load - dl).max(0.0);
        }
        // 其余（on_target 且非活跃且非空闲 = 纯阅读）：不变
        self.load
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::TickCounts;
    use std::time::Duration;

    fn dt_100ms() -> Duration {
        Duration::from_millis(100)
    }

    fn active_on_target() -> TickInput {
        TickInput {
            counts: TickCounts { keys: 5, mouse: 1, ..Default::default() },
            on_target: true,
            idle: Duration::ZERO,
            dt: dt_100ms(),
            key_intervals_ms: vec![200.0, 250.0],
        }
    }

    #[test]
    fn load_starts_at_zero() {
        let m = AttentionModel::new(ModelConfig::default());
        assert_eq!(m.load(), 0.0);
    }

    #[test]
    fn active_on_target_grows() {
        let mut m = AttentionModel::new(ModelConfig::default());
        let start = m.load();
        m.tick(&active_on_target());
        assert!(m.load() > start);
    }

    #[test]
    fn idle_shrinks() {
        let mut m = AttentionModel::new(ModelConfig::default());
        // 先攒一点 Load
        for _ in 0..100 {
            m.tick(&active_on_target());
        }
        let before = m.load();
        let idle_input = TickInput {
            counts: TickCounts::default(),
            on_target: true,
            idle: Duration::from_secs(60), // > grace(30s)
            dt: dt_100ms(),
            key_intervals_ms: vec![],
        };
        for _ in 0..100 {
            m.tick(&idle_input);
        }
        assert!(m.load() < before);
    }

    #[test]
    fn off_target_active_shrinks_slower_than_idle() {
        let cfg = ModelConfig::default();
        let mut a = AttentionModel::new(cfg);
        let mut b = AttentionModel::new(cfg);
        // 充分预热：让 load 远超 shrink 阶段会移除的量（off-target 60s 移除 4.0，idle 60s 移除 6.0）。
        // 否则两者都被夹到 0，4/min 与 6/min 的速率差异不可见。6000 ticks ≈ 12 load。
        for _ in 0..6000 {
            a.tick(&active_on_target());
            b.tick(&active_on_target());
        }
        let off = TickInput {
            counts: TickCounts { keys: 5, ..Default::default() },
            on_target: false,
            idle: Duration::ZERO,
            dt: dt_100ms(),
            key_intervals_ms: vec![],
        };
        let idle = TickInput {
            counts: TickCounts::default(),
            on_target: false,
            idle: Duration::from_secs(60),
            dt: dt_100ms(),
            key_intervals_ms: vec![],
        };
        for _ in 0..600 {
            a.tick(&off);
            b.tick(&idle);
        }
        // 空闲缩小更快(6/min) > 离开缩小(4/min) -> b 掉得更多
        assert!(b.load() < a.load());
    }

    #[test]
    fn reading_paused_no_change() {
        let mut m = AttentionModel::new(ModelConfig::default());
        for _ in 0..50 {
            m.tick(&active_on_target());
        }
        let before = m.load();
        // on_target、无输入、idle < grace -> 阅读，不涨不缩
        let reading = TickInput {
            counts: TickCounts::default(),
            on_target: true,
            idle: Duration::from_secs(5),
            dt: dt_100ms(),
            key_intervals_ms: vec![],
        };
        for _ in 0..50 {
            m.tick(&reading);
        }
        assert!((m.load() - before).abs() < 1e-9);
    }

    #[test]
    fn load_clamps_to_100() {
        let mut m = AttentionModel::new(ModelConfig::default());
        for _ in 0..100_000 {
            m.tick(&active_on_target());
        }
        assert!(m.load() <= 100.0);
    }

    #[test]
    fn reset_zeroes_load() {
        let mut m = AttentionModel::new(ModelConfig::default());
        for _ in 0..100 {
            m.tick(&active_on_target());
        }
        m.reset();
        assert_eq!(m.load(), 0.0);
    }

    #[test]
    fn reset_clears_signal_history() {
        // 用带切窗/退格的分心输入预热，让 buffer 累积疲劳信号
        let mut m = AttentionModel::new(ModelConfig::default());
        let distracted = TickInput {
            counts: TickCounts { keys: 10, backspaces: 5, switches: 20, ..Default::default() },
            on_target: true,
            idle: Duration::ZERO,
            dt: dt_100ms(),
            key_intervals_ms: vec![100.0, 500.0],
        };
        for _ in 0..100 {
            m.tick(&distracted);
        }
        m.reset();
        // 重置后再 tick 一次，应与全新 model 做同样单 tick 的结果一致（buffer 已清空）
        let mut fresh = AttentionModel::new(ModelConfig::default());
        let load_after_reset = m.tick(&distracted);
        let load_fresh = fresh.tick(&distracted);
        assert!((load_after_reset - load_fresh).abs() < 1e-9, "{load_after_reset} vs {load_fresh}");
    }

    #[test]
    fn cap_at_limits_load() {
        let mut m = AttentionModel::new(ModelConfig::default());
        for _ in 0..100_000 {
            m.tick(&active_on_target());
        }
        m.cap_at(99.0);
        assert!((m.load() - 99.0).abs() < 1e-9);
    }
}
