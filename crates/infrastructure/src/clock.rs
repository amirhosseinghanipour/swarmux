use std::time::{SystemTime, UNIX_EPOCH};

use swarmux_application::ports::Clock;
use swarmux_domain::Millis;

/// Wall-clock time.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as Millis)
    }
}
