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
