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
