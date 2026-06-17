use std::time::Duration;

use attention_blackhole::controller::{Controller, Frame, Renderer, SignalProvider};
use attention_blackhole::model::config::ModelConfig;
use attention_blackhole::model::types::{TickCounts, TickInput};

/// 模拟时间线 provider：
///  - 0..40 min：分心式输入（on_target、活跃、有切窗/退格）-> load 上升
///  - 40..46 min：空闲（真休息，6 min 足以把 load 从 100 降到 ~64、跌破 unlock=70）-> 衰减并解锁
///  - 46..60 min：恢复活跃
struct TimelineProvider {
    t_secs: f64,
}

impl SignalProvider for TimelineProvider {
    fn snapshot(&mut self, dt: Duration) -> TickInput {
        let prev = self.t_secs;
        self.t_secs += dt.as_secs_f64();

        let typing_phase = prev < 40.0 * 60.0 || prev >= 46.0 * 60.0;
        let idle_phase = ((40.0 * 60.0)..(46.0 * 60.0)).contains(&prev);

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
