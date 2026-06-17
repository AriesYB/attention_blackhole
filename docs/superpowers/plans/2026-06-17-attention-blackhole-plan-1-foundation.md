# 注意力黑洞 — Plan 1: 基础（model + controller）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 搭出平台无关的纯 Rust 基础：注意力模型（信号缓冲、涨缩公式、状态机）+ controller 编排 + trait 抽象 + 一个能用 mock 信号跑起来、可观察涨缩与强制休息的命令行 demo。

**Architecture:** 单个 library crate `attention_blackhole`，模块按 spec §4.1 分层。`model/` 纯函数式无 I/O，用模拟时间（`f64` 秒）保证确定性单测；`controller/` 通过 `SignalProvider` / `Renderer` trait 解耦真实平台层（后续 plan 实现）。本计划不触碰任何 Windows/DX 代码，全部可在任意平台 `cargo test`。

**Tech Stack:** Rust（edition 2021），仅 std，零外部依赖（保证离线可构建可测试）。

---

## File Structure（本计划产出）

- `Cargo.toml` — crate 定义，library + 一个 example
- `src/lib.rs` — 顶层模块导出
- `src/model/mod.rs` — model 子模块导出
- `src/model/config.rs` — `ModelConfig`（所有可调参数 + 默认值）
- `src/model/types.rs` — `TickCounts`、`TickInput`
- `src/model/signal_buffer.rs` — `SignalBuffer`（60s 滚动统计 + focus/fatigue 因子）
- `src/model/state.rs` — `AppState` + 状态机 `next_state`
- `src/model/attention.rs` — `AttentionModel`（涨缩核心）
- `src/controller.rs` — `Controller` + `SignalProvider`/`Renderer`/`Frame`
- `src/mock.rs` — `ScriptedProvider`、`CollectingRenderer`（测试/示例用）
- `examples/cli_demo.rs` — 可运行的时间线 demo
- `tests/integration.rs` — 端到端集成测试

**分层纪律落地**：本计划只产出 `model/`、`controller/`、`mock/`，三者均平台无关。`platform/` 与 `renderer/`（Windows/DX）留给后续 plan，通过 trait 接入。

---

## 关键常量校准说明（实现值）

spec 把 focus/fatigue 映射留到实现，这里定下来（均可配）：
- `growth_per_min = 2.0`（中性基线 ≈ 50 分钟填满）
- `focus_min = 0.6`、`focus_max = 1.0`：稳定输入减速、最多减 40%
- `fatigue_min = 1.0`、`fatigue_max = 2.0`
- `idle_shrink_per_min = 6.0`：满负荷(100)→UNLOCK(70) 需 `(100-70)/6 = 5` 分钟（对齐 5 分钟自动解锁）
- `offtarget_shrink_per_min = 4.0`、`idle_grace = 30s`
- 阈值 `dim=80`、`forced=100`、`unlock=70`

---

## Task 1: 脚手架 + config + types

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/model/mod.rs`
- Create: `src/model/config.rs`
- Create: `src/model/types.rs`

- [ ] **Step 1: 初始化 crate 目录与 Cargo.toml**

创建 `Cargo.toml`：

```toml
[package]
name = "attention_blackhole"
version = "0.1.0"
edition = "2021"

[lib]
name = "attention_blackhole"
path = "src/lib.rs"
```

> 注意：**不要**在 `Cargo.toml` 里声明 `[[example]]`。Cargo 会自动发现 `examples/` 下的 `*.rs` 作为 example 目标；若现在就声明指向尚不存在的 `examples/cli_demo.rs`（Task 7 才创建），则 Task 1–6 的 `cargo build` / `cargo test` 会因找不到 example 文件而失败。Task 7 创建该文件后即被自动识别，无需改 `Cargo.toml`。

- [ ] **Step 2: 写 lib.rs 与 model/mod.rs**

`src/lib.rs`：

```rust
pub mod controller;
pub mod mock;
pub mod model;
```

`src/model/mod.rs`：

```rust
pub mod attention;
pub mod config;
pub mod signal_buffer;
pub mod state;
pub mod types;
```

- [ ] **Step 3: 写 config.rs（先写含一个断言默认值的测试）**

`src/model/config.rs`：

```rust
use std::time::Duration;

/// 注意力模型所有可调参数。默认值经校准（见 plan 说明）。
#[derive(Debug, Clone, Copy)]
pub struct ModelConfig {
    pub growth_per_min: f64,
    pub idle_shrink_per_min: f64,
    pub offtarget_shrink_per_min: f64,
    pub idle_grace: Duration,
    pub dim_threshold: f64,
    pub forced_threshold: f64,
    pub unlock_threshold: f64,
    pub window: Duration,
    pub focus_min: f64,
    pub focus_max: f64,
    pub fatigue_min: f64,
    pub fatigue_max: f64,
    pub jitter_max_ms: f64,
    pub switch_max_per_min: f64,
    pub error_max: f64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            growth_per_min: 2.0,
            idle_shrink_per_min: 6.0,
            offtarget_shrink_per_min: 4.0,
            idle_grace: Duration::from_secs(30),
            dim_threshold: 80.0,
            forced_threshold: 100.0,
            unlock_threshold: 70.0,
            window: Duration::from_secs(60),
            focus_min: 0.6,
            focus_max: 1.0,
            fatigue_min: 1.0,
            fatigue_max: 2.0,
            jitter_max_ms: 500.0,
            switch_max_per_min: 20.0,
            error_max: 0.3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_calibrated_values() {
        let c = ModelConfig::default();
        assert!((c.growth_per_min - 2.0).abs() < 1e-9);
        assert!((c.idle_shrink_per_min - 6.0).abs() < 1e-9);
        assert_eq!(c.idle_grace, Duration::from_secs(30));
        assert!((c.unlock_threshold - 70.0).abs() < 1e-9);
        // 5 分钟自动解锁的数学一致性：满负荷到 UNLOCK 需要 (100-70)/6 = 5 min
        let mins_to_unlock = (c.forced_threshold - c.unlock_threshold) / c.idle_shrink_per_min;
        assert!((mins_to_unlock - 5.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 4: 写 types.rs**

`src/model/types.rs`：

```rust
use std::time::Duration;

/// 单个 tick 的原始事件计数（仅计数，不含按键内容）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TickCounts {
    pub keys: u32,
    pub mouse: u32,
    pub switches: u32,
    pub backspaces: u32,
}

impl TickCounts {
    pub fn total_input(&self) -> u32 {
        self.keys + self.mouse
    }
}

/// 一个 tick 喂给模型的全部输入。
#[derive(Debug, Clone)]
pub struct TickInput {
    pub counts: TickCounts,
    pub on_target: bool,
    pub idle: Duration,
    pub dt: Duration,
    pub key_intervals_ms: Vec<f64>,
}
```

- [ ] **Step 5: 运行测试验证通过**

Run: `cargo test --lib model::config`
Expected: PASS（1 个测试）。注意：`lib.rs` 声明了 `pub mod controller` / `pub mod mock`，且 `model/mod.rs` 声明了四个子模块；为让本步可独立编译验证，需创建以下**占位文件**（后续 task 替换为完整实现）：
- `src/model/attention.rs`
- `src/model/signal_buffer.rs`
- `src/model/state.rs`
- `src/controller.rs`
- `src/mock.rs`

占位文件示例 `src/model/attention.rs`：

```rust
// 占位：Task 3 填充
```

其余四个占位文件同样放一行注释（如 `// 占位：Task N 填充`）。

Run: `cargo test --lib model::config`
Expected: `test result: ok. 1 passed`

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml src/
git commit -m "feat(model): scaffold crate, config, and tick types"
```

---

## Task 2: SignalBuffer（滚动统计 + 因子）

**Files:**
- Modify: `src/model/signal_buffer.rs`

负责：维护 60s 滚动窗口的计数与击键间隔；计算 `switch_rate_per_min`、`error_rate`、`typing_jitter_ms`，并据此产出 `focus_factor`（输入越稳越低）与 `fatigue_factor`（切窗/抖动/错误越高越高）。使用模拟时间 `f64` 秒保证确定性。

- [ ] **Step 1: 写失败测试**

替换 `src/model/signal_buffer.rs` 全部内容：

```rust
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
        assert!((b.focus_factor(&c) - c.focus_max).abs() < 1e-9); // 无输入 -> 中性
        assert!((b.fatigue_factor(0.0, &c) - c.fatigue_min).abs() < 1e-9);
        assert_eq!(b.error_rate(), 0.0);
        assert_eq!(b.typing_jitter_ms(), 0.0);
    }

    #[test]
    fn steady_typing_lowers_focus_factor() {
        let mut b = SignalBuffer::new(60.0);
        let c = cfg();
        // 完全稳定：间隔全 200ms -> jitter 0 -> focus_min
        for t in 0..10 {
            b.push(t as f64, TickCounts { keys: 4, ..Default::default() }, &[200.0, 200.0]);
        }
        assert!((b.focus_factor(&c) - c.focus_min).abs() < 1e-9);
    }

    #[test]
    fn erratic_typing_raises_focus_and_fatigue() {
        let mut b = SignalBuffer::new(60.0);
        let c = cfg();
        // 高方差间隔 + 退格 + 切窗
        b.push(
            0.0,
            TickCounts { keys: 10, backspaces: 5, switches: 20, ..Default::default() },
            &[100.0, 500.0],
        );
        let f = b.focus_factor(&c);
        assert!(f > c.focus_min);
        let fat = b.fatigue_factor(1.0, &c);
        assert!((fat - c.fatigue_max).abs() < 1e-9); // 各信号拉满 -> 封顶
    }

    #[test]
    fn window_evicts_old_samples() {
        let mut b = SignalBuffer::new(10.0);
        b.push(0.0, TickCounts { keys: 100, ..Default::default() }, &[]);
        b.push(20.0, TickCounts { keys: 1, ..Default::default() }, &[]); // t=0 已过期
        assert_eq!(b.total_keys(), 1);
    }
}
```

- [ ] **Step 2: 运行测试验证通过**

Run: `cargo test --lib model::signal_buffer`
Expected: `test result: ok. 4 passed`

- [ ] **Step 3: 提交**

```bash
git add src/model/signal_buffer.rs
git commit -m "feat(model): rolling signal buffer with focus/fatigue factors"
```

---

## Task 3: AttentionModel（涨缩核心）

**Files:**
- Modify: `src/model/attention.rs`

负责：维护 `load ∈ [0,100]` 与内部模拟时钟；按 spec §6.2/6.3 决定每 tick 的增长或缩小。

- [ ] **Step 1: 写失败测试 + 实现**

替换 `src/model/attention.rs` 全部内容：

```rust
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

    pub fn reset(&mut self) {
        self.load = 0.0;
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
        // 先攒一点 load
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
    fn cap_at_limits_load() {
        let mut m = AttentionModel::new(ModelConfig::default());
        for _ in 0..100_000 {
            m.tick(&active_on_target());
        }
        m.cap_at(99.0);
        assert!((m.load() - 99.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: 运行测试验证通过**

Run: `cargo test --lib model::attention`
Expected: `test result: ok. 8 passed`

- [ ] **Step 3: 提交**

```bash
git add src/model/attention.rs
git commit -m "feat(model): attention model with growth/shrink math"
```

---

## Task 4: 状态机（AppState + next_state）

**Files:**
- Modify: `src/model/state.rs`

负责：spec §6.4 的状态迁移。注意从 `ForcedBreak` 退出需用 `unlock_threshold`，且进入 `ForcedBreak` 需 `force_enabled`。

- [ ] **Step 1: 写失败测试 + 实现**

替换 `src/model/state.rs` 全部内容：

```rust
use crate::model::config::ModelConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Working,
    Dimming,
    ForcedBreak,
}

fn classify(load: f64, cfg: &ModelConfig) -> AppState {
    if load >= cfg.dim_threshold {
        AppState::Dimming
    } else {
        AppState::Working
    }
}

/// 计算下一状态。`prev` 必传：退出 ForcedBreak 用 unlock_threshold。
pub fn next_state(
    prev: AppState,
    load: f64,
    force_enabled: bool,
    cfg: &ModelConfig,
) -> AppState {
    match prev {
        AppState::ForcedBreak => {
            if load < cfg.unlock_threshold {
                classify(load, cfg)
            } else {
                AppState::ForcedBreak
            }
        }
        _ => {
            if force_enabled && load >= cfg.forced_threshold {
                AppState::ForcedBreak
            } else {
                classify(load, cfg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ModelConfig {
        ModelConfig::default()
    }

    #[test]
    fn working_to_dimming_at_80() {
        let c = cfg();
        assert_eq!(next_state(AppState::Working, 79.9, true, &c), AppState::Working);
        assert_eq!(next_state(AppState::Working, 80.0, true, &c), AppState::Dimming);
    }

    #[test]
    fn dimming_to_forced_at_100_when_force_on() {
        let c = cfg();
        assert_eq!(next_state(AppState::Dimming, 99.9, true, &c), AppState::Dimming);
        assert_eq!(next_state(AppState::Dimming, 100.0, true, &c), AppState::ForcedBreak);
    }

    #[test]
    fn never_forced_when_force_off() {
        let c = cfg();
        assert_eq!(next_state(AppState::Working, 100.0, false, &c), AppState::Dimming);
    }

    #[test]
    fn forced_exits_only_below_unlock() {
        let c = cfg();
        // load 在 unlock(70) 与 dim(80) 之间：仍在 ForcedBreak
        assert_eq!(next_state(AppState::ForcedBreak, 75.0, true, &c), AppState::ForcedBreak);
        // 跌破 unlock：按 load 分类（>=dim -> Dimming，这里 <80 -> Working）
        assert_eq!(next_state(AppState::ForcedBreak, 69.0, true, &c), AppState::Working);
        assert_eq!(next_state(AppState::ForcedBreak, 85.0, true, &c), AppState::ForcedBreak);
    }
}
```

- [ ] **Step 2: 运行测试验证通过**

Run: `cargo test --lib model::state`
Expected: `test result: ok. 4 passed`

- [ ] **Step 3: 提交**

```bash
git add src/model/state.rs
git commit -m "feat(model): app state machine with unlock semantics"
```

---

## Task 5: Controller + traits + 帧编排

**Files:**
- Modify: `src/controller.rs`（Task 1 已建占位，本 task 替换为完整实现）

负责：串联 `SignalProvider → AttentionModel → StateMachine → Renderer`；处理 `reset` / `set_force_enabled`；`force_enabled=false` 时封顶 99%。

- [ ] **Step 1: 写失败测试 + 实现**

`src/controller.rs`：

```rust
use std::time::Duration;

use crate::model::attention::AttentionModel;
use crate::model::config::ModelConfig;
use crate::model::state::{self, AppState};
use crate::model::types::TickInput;

/// 平台信号源抽象（后续 plan 用 Windows 实现填充）。
pub trait SignalProvider {
    fn snapshot(&mut self, dt: Duration) -> TickInput;
}

/// 渲染器抽象（后续 plan 用 D3D11 实现填充）。
pub trait Renderer {
    fn render(&mut self, frame: &Frame);
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub load: f64,
    pub state: AppState,
    pub on_target: bool,
}

pub struct Controller {
    model: AttentionModel,
    state: AppState,
    force_enabled: bool,
    config: ModelConfig,
}

impl Controller {
    pub fn new(config: ModelConfig, force_enabled: bool) -> Self {
        Self {
            model: AttentionModel::new(config),
            state: AppState::Working,
            force_enabled,
            config,
        }
    }

    pub fn load(&self) -> f64 {
        self.model.load()
    }

    pub fn state(&self) -> AppState {
        self.state
    }

    pub fn reset(&mut self) {
        self.model.reset();
        self.state = AppState::Working;
    }

    pub fn set_force_enabled(&mut self, v: bool) {
        self.force_enabled = v;
    }

    pub fn tick(
        &mut self,
        provider: &mut dyn SignalProvider,
        renderer: &mut dyn Renderer,
        dt: Duration,
    ) {
        let input = provider.snapshot(dt);
        let on_target = input.on_target;
        self.model.tick(&input);

        if !self.force_enabled {
            self.model.cap_at(self.config.forced_threshold - 1.0);
        }

        let load = self.model.load();
        self.state = state::next_state(self.state, load, self.force_enabled, &self.config);
        renderer.render(&Frame {
            load,
            state: self.state,
            on_target,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::TickCounts;
    use std::cell::RefCell;

    /// 固定返回同一输入的 provider。
    struct FixedProvider {
        input: TickInput,
    }
    impl SignalProvider for FixedProvider {
        fn snapshot(&mut self, _dt: Duration) -> TickInput {
            self.input.clone()
        }
    }

    struct Sink {
        frames: RefCell<Vec<Frame>>,
    }
    impl Renderer for Sink {
        fn render(&mut self, frame: &Frame) {
            self.frames.borrow_mut().push(*frame);
        }
    }

    fn active_on_target() -> TickInput {
        TickInput {
            counts: TickCounts { keys: 5, mouse: 1, ..Default::default() },
            on_target: true,
            idle: Duration::ZERO,
            dt: Duration::from_millis(100),
            key_intervals_ms: vec![200.0, 250.0],
        }
    }

    #[test]
    fn tick_drives_growth_and_render() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider { input: active_on_target() };
        let mut sink = Sink { frames: RefCell::new(vec![]) };
        for _ in 0..10 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        assert!(ctrl.load() > 0.0);
        assert_eq!(sink.frames.borrow().len(), 10);
        assert!(sink.frames.borrow().last().unwrap().on_target);
    }

    #[test]
    fn reaches_forced_break_under_sustained_work() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider { input: active_on_target() };
        let mut sink = Sink { frames: RefCell::new(vec![]) };
        // 大量 tick 足以填满到 100（数值上稳定增长）
        for _ in 0..200_000 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
            if ctrl.state() == AppState::ForcedBreak {
                break;
            }
        }
        assert_eq!(ctrl.state(), AppState::ForcedBreak);
    }

    #[test]
    fn force_off_never_forced() {
        let mut ctrl = Controller::new(ModelConfig::default(), false);
        let mut prov = FixedProvider { input: active_on_target() };
        let mut sink = Sink { frames: RefCell::new(vec![]) };
        for _ in 0..200_000 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        assert_ne!(ctrl.state(), AppState::ForcedBreak);
        assert!(ctrl.load() <= 99.0 + 1e-9);
    }

    #[test]
    fn reset_clears_state() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider { input: active_on_target() };
        let mut sink = Sink { frames: RefCell::new(vec![]) };
        for _ in 0..1000 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        ctrl.reset();
        assert_eq!(ctrl.load(), 0.0);
        assert_eq!(ctrl.state(), AppState::Working);
    }
}
```

- [ ] **Step 2: 运行测试验证通过**

Run: `cargo test --lib controller`
Expected: `test result: ok. 4 passed`

- [ ] **Step 3: 提交**

```bash
git add src/controller.rs
git commit -m "feat(controller): orchestrate model+state into render frames"
```

---

## Task 6: Mock 实现（ScriptedProvider / CollectingRenderer）

**Files:**
- Modify: `src/mock.rs`（Task 1 已建占位，本 task 替换为完整实现）

提供给 example 与集成测试使用。

- [ ] **Step 1: 写实现**

`src/mock.rs`：

```rust
use std::time::Duration;

use crate::controller::{Frame, Renderer, SignalProvider};
use crate::model::types::TickInput;

/// 按 script 循环产出 TickInput（到尾后从头循环）。
pub struct ScriptedProvider {
    script: Vec<TickInput>,
    idx: usize,
}

impl ScriptedProvider {
    pub fn new(script: Vec<TickInput>) -> Self {
        assert!(!script.is_empty(), "script must be non-empty");
        Self { script, idx: 0 }
    }
}

impl SignalProvider for ScriptedProvider {
    fn snapshot(&mut self, _dt: Duration) -> TickInput {
        let input = self.script[self.idx % self.script.len()].clone();
        self.idx += 1;
        input
    }
}

/// 收集所有渲染帧，供断言。
#[derive(Default)]
pub struct CollectingRenderer {
    pub frames: Vec<Frame>,
}

impl Renderer for CollectingRenderer {
    fn render(&mut self, frame: &Frame) {
        self.frames.push(*frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::types::TickCounts;

    #[test]
    fn scripted_provider_cycles() {
        let s = vec![
            TickInput {
                counts: TickCounts { keys: 1, ..Default::default() },
                on_target: true,
                idle: Duration::ZERO,
                dt: Duration::from_millis(100),
                key_intervals_ms: vec![],
            },
            TickInput {
                counts: TickCounts { keys: 2, ..Default::default() },
                on_target: true,
                idle: Duration::ZERO,
                dt: Duration::from_millis(100),
                key_intervals_ms: vec![],
            },
        ];
        let mut p = ScriptedProvider::new(s);
        assert_eq!(p.snapshot(Duration::ZERO).counts.keys, 1);
        assert_eq!(p.snapshot(Duration::ZERO).counts.keys, 2);
        assert_eq!(p.snapshot(Duration::ZERO).counts.keys, 1); // 循环
    }

    #[test]
    fn collecting_renderer_records() {
        let mut r = CollectingRenderer::default();
        r.render(&Frame { load: 1.0, state: crate::model::state::AppState::Working, on_target: true });
        assert_eq!(r.frames.len(), 1);
    }
}
```

- [ ] **Step 2: 运行测试验证通过**

Run: `cargo test --lib mock`
Expected: `test result: ok. 2 passed`

- [ ] **Step 3: 提交**

```bash
git add src/mock.rs
git commit -m "feat: mock provider and collecting renderer for tests/demo"
```

---

## Task 7: cli_demo 示例（可运行时间线）

**Files:**
- Create: `examples/cli_demo.rs`

模拟 1 小时时间线：前段「分心式」输入让 load 涨到 ForcedBreak，中段空闲 5 分钟触发自动解锁，演示完整循环。每模拟分钟打印一行。

- [ ] **Step 1: 写 example**

`examples/cli_demo.rs`：

```rust
use std::time::Duration;

use attention_blackhole::controller::{Controller, Frame, Renderer, SignalProvider};
use attention_blackhole::model::config::ModelConfig;
use attention_blackhole::model::types::{TickCounts, TickInput};

/// 模拟时间线 provider：
///  - 0..40 min：分心式输入（on_target、活跃、有切窗/退格）-> load 上升
///  - 40..45 min：空闲（真休息）-> load 衰减
///  - 45..60 min：恢复活跃
struct TimelineProvider {
    t_secs: f64,
}

impl SignalProvider for TimelineProvider {
    fn snapshot(&mut self, dt: Duration) -> TickInput {
        let prev = self.t_secs;
        self.t_secs += dt.as_secs_f64();

        let typing_phase = prev < 40.0 * 60.0 || prev >= 45.0 * 60.0;
        let idle_phase = ((40.0 * 60.0)..(45.0 * 60.0)).contains(&prev);

        let counts = if typing_phase {
            // 分心：有切窗 + 退格，体现 fatigue 加速
            TickCounts { keys: 6, mouse: 1, switches: 2, backspaces: 2 }
        } else {
            TickCounts::default()
        };
        let intervals = if typing_phase { vec![150.0, 400.0] } else { vec![] };
        let idle = if idle_phase { Duration::from_secs(60) } else { Duration::ZERO };

        TickInput { counts, on_target: typing_phase, idle, dt, key_intervals_ms: intervals }
    }
}

struct NoopRenderer;
impl Renderer for NoopRenderer {
    fn render(&mut self, _frame: &Frame) {}
}

fn main() {
    let mut ctrl = Controller::new(ModelConfig::default(), true);
    let mut prov = TimelineProvider { t_secs: 0.0 };
    let mut sink = NoopRenderer;
    let dt = Duration::from_millis(100); // 10Hz
    let total_ticks = (60.0 * 60.0 / 0.1) as usize; // 1 小时

    let mut last_min = usize::MAX;
    println!("min | load   | state");
    println!("----+--------+-------------");
    for i in 0..total_ticks {
        ctrl.tick(&mut prov, &mut sink, dt);
        let min = i / 600; // 600 ticks = 1 min @10Hz
        if min != last_min {
            last_min = min;
            println!("{:>3} | {:>6.1} | {:?}", min, ctrl.load(), ctrl.state());
        }
    }
}
```

- [ ] **Step 2: 运行 demo 验证**

Run: `cargo run --example cli_demo`
Expected: 打印约 60 行；load 在前 ~30 多分钟上升、触发 `ForcedBreak`，40–45 min 区间因空闲衰减、`load` 跌破 70 后状态回到 `Working`/`Dimming`，随后再次上升。具体数值依实现细节，但应观察到「涨 → ForcedBreak → 空闲缩 → 解锁」的完整循环。

> 说明：本步骤为视觉/行为验证，无自动断言。若 load 未在 40 min 内到 100，说明 fatigue/focus 校准偏慢，可在 `ModelConfig` 微调（非本计划目标，留作实测调参）。

- [ ] **Step 3: 提交**

```bash
git add examples/cli_demo.rs
git commit -m "examples: cli demo timeline showing full load/break cycle"
```

---

## Task 8: 端到端集成测试

**Files:**
- Create: `tests/integration.rs`

用 mock 跑一段确定性脚本，断言完整生命周期：Working → Dimming → ForcedBreak →（空闲）→ Working，以及 reset 与 force_off 行为。

- [ ] **Step 1: 写集成测试**

`tests/integration.rs`：

```rust
use std::time::Duration;

use attention_blackhole::controller::Controller;
use attention_blackhole::model::config::ModelConfig;
use attention_blackhole::model::state::AppState;
use attention_blackhole::model::types::{TickCounts, TickInput};
use attention_blackhole::mock::{CollectingRenderer, ScriptedProvider};

fn active_distracted() -> TickInput {
    TickInput {
        counts: TickCounts { keys: 6, mouse: 1, switches: 2, backspaces: 2 },
        on_target: true,
        idle: Duration::ZERO,
        dt: Duration::from_millis(100),
        key_intervals_ms: vec![150.0, 400.0],
    }
}

fn idle_input() -> TickInput {
    TickInput {
        counts: TickCounts::default(),
        on_target: true,
        idle: Duration::from_secs(60),
        dt: Duration::from_millis(100),
        key_intervals_ms: vec![],
    }
}

#[test]
fn full_lifecycle_working_dimming_forced_unlock() {
    let cfg = ModelConfig::default();
    let mut ctrl = Controller::new(cfg, true);
    let mut prov = ScriptedProvider::new(vec![active_distracted()]);
    let mut sink = CollectingRenderer::default();
    let dt = Duration::from_millis(100);

    // 阶段 1：持续工作直到 ForcedBreak
    for _ in 0..200_000 {
        ctrl.tick(&mut prov, &mut sink, dt);
        if ctrl.state() == AppState::ForcedBreak {
            break;
        }
    }
    assert_eq!(ctrl.state(), AppState::ForcedBreak);

    // 阶段 2：切换到空闲 provider，验证经 ~5 min 解锁
    let mut idle_prov = ScriptedProvider::new(vec![idle_input()]);
    // 5 min = 300s = 3000 ticks @10Hz；多跑一点确保跌破 unlock
    for _ in 0..3500 {
        ctrl.tick(&mut idle_prov, &mut sink, dt);
    }
    assert_ne!(ctrl.state(), AppState::ForcedBreak);
    assert!(ctrl.load() < cfg.unlock_threshold);
}

#[test]
fn reset_drops_to_working() {
    let cfg = ModelConfig::default();
    let mut ctrl = Controller::new(cfg, true);
    let mut prov = ScriptedProvider::new(vec![active_distracted()]);
    let mut sink = CollectingRenderer::default();
    for _ in 0..5_000 {
        ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
    }
    ctrl.reset();
    assert_eq!(ctrl.state(), AppState::Working);
    assert_eq!(ctrl.load(), 0.0);
}

#[test]
fn force_disabled_caps_below_forced() {
    let cfg = ModelConfig::default();
    let mut ctrl = Controller::new(cfg, false);
    let mut prov = ScriptedProvider::new(vec![active_distracted()]);
    let mut sink = CollectingRenderer::default();
    for _ in 0..200_000 {
        ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
    }
    assert_ne!(ctrl.state(), AppState::ForcedBreak);
    assert!(ctrl.load() <= 99.0 + 1e-9);
}
```

- [ ] **Step 2: 运行全部测试**

Run: `cargo test`
Expected: 所有 lib 单测 + 3 个集成测试通过。

- [ ] **Step 3: 提交**

```bash
git add tests/integration.rs
git commit -m "test: end-to-end lifecycle, reset, and force-disabled integration"
```

---

## 完成标准（Plan 1 Definition of Done）

- `cargo test` 全绿（lib 单测 + 集成测试）。
- `cargo run --example cli_demo` 能观察到「涨 → ForcedBreak → 空闲缩 → 解锁」完整循环。
- `model/` 与 `controller/` 全平台无关、零外部依赖；`platform/`、`renderer/` 尚未存在（留给 Plan 2/3）。
- trait `SignalProvider` / `Renderer` 已定义并被 mock 实现，后续 plan 直接替换为真实实现。
