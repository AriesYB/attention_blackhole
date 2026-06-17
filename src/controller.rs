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

/// 渲染帧数据。
///
/// **有意不包含窗口矩形 (rect)**：rect 是平台概念（HWND / WindowId / 监视器坐标），
/// 应由平台层渲染器自行从其 `WindowTracker` 获取。把 rect 塞进 `Frame` 会让
/// 平台类型渗入这个纯 Rust 核心，破坏「核心无平台依赖」的分层。`Frame` 只承载
/// 与注意力模型相关的纯值类型。
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

        // force 关闭时把 load 封顶到 forced-1：next_state 本就不会进入 ForcedBreak
        // （state.rs 进入条件含 force_enabled），这里的封顶是语义层——保证对外
        // 查询/渲染的 load 永不显示 100%，避免误导用户「已到极限」。
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
        frames: Vec<Frame>,
    }
    impl Renderer for Sink {
        fn render(&mut self, frame: &Frame) {
            self.frames.push(*frame);
        }
    }

    fn active_on_target() -> TickInput {
        TickInput {
            counts: TickCounts {
                keys: 5,
                mouse: 1,
                ..Default::default()
            },
            on_target: true,
            idle: Duration::ZERO,
            dt: Duration::from_millis(100),
            key_intervals_ms: vec![200.0, 250.0],
        }
    }

    #[test]
    fn tick_drives_growth_and_render() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider {
            input: active_on_target(),
        };
        let mut sink = Sink { frames: vec![] };
        for _ in 0..10 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        assert!(ctrl.load() > 0.0);
        assert_eq!(sink.frames.len(), 10);
        assert!(sink.frames.last().unwrap().on_target);
    }

    #[test]
    fn reaches_forced_break_under_sustained_work() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider {
            input: active_on_target(),
        };
        let mut sink = Sink { frames: vec![] };
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
        let mut prov = FixedProvider {
            input: active_on_target(),
        };
        let mut sink = Sink { frames: vec![] };
        for _ in 0..200_000 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        assert_ne!(ctrl.state(), AppState::ForcedBreak);
        assert!(ctrl.load() <= 99.0 + 1e-9);
    }

    #[test]
    fn reset_clears_state() {
        let mut ctrl = Controller::new(ModelConfig::default(), true);
        let mut prov = FixedProvider {
            input: active_on_target(),
        };
        let mut sink = Sink { frames: vec![] };
        for _ in 0..1000 {
            ctrl.tick(&mut prov, &mut sink, Duration::from_millis(100));
        }
        ctrl.reset();
        assert_eq!(ctrl.load(), 0.0);
        assert_eq!(ctrl.state(), AppState::Working);
    }
}
