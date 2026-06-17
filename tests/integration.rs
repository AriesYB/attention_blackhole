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
