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
