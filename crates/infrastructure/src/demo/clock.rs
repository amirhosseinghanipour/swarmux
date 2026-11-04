use std::cell::Cell;

use swarmux_application::ports::Clock;
use swarmux_domain::Millis;

/// A clock the demo moves by hand, so a run always sees the same times.
pub struct DemoClock(Cell<Millis>);

impl DemoClock {
    pub fn new(start: Millis) -> DemoClock {
        DemoClock(Cell::new(start))
    }

    pub fn set(&self, now: Millis) {
        self.0.set(now);
    }
}

impl Clock for DemoClock {
    fn now(&self) -> Millis {
        self.0.get()
    }
}
