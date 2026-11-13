//! Adapters for `swarmux --demo`: stand-ins for `claude`, git and the
//! checks that play a scripted scenario instead of touching anything real.
//!
//! Nothing here is recorded from a real run. The demo is a script; every
//! number on screen is made up, and the interface says so.

mod clock;
mod gate;
mod misc;
mod runner;

pub use clock::DemoClock;
pub use gate::{DemoChecker, DemoGateSource, DemoVcs};
pub use misc::{DemoFeed, DemoMissions, DemoMonitor, DemoPlanner, DemoPlans, DemoWorkspace};
pub use runner::{ScriptedInbox, ScriptedRunner};
