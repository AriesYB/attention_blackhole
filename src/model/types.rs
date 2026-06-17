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
