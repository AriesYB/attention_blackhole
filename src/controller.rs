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
