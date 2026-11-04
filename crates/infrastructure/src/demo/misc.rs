use std::cell::RefCell;
use std::path::Path;

use swarmux_application::ports::{SystemMonitor, TelemetryFeed, Workspace};
use swarmux_application::{AppResult, MissionStore, TelemetrySnapshot};
use swarmux_domain::mission::Mission;

/// Missions kept in memory; they last as long as the demo.
#[derive(Default)]
pub struct DemoMissions {
    rows: RefCell<Vec<Mission>>,
}

impl MissionStore for DemoMissions {
    fn save(&self, mission: &Mission) -> AppResult<()> {
        let mut rows = self.rows.borrow_mut();
        match rows.iter_mut().find(|m| m.slug == mission.slug) {
            Some(row) => *row = mission.clone(),
            None => rows.push(mission.clone()),
        }
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<Mission>> {
        Ok(self.rows.borrow().clone())
    }
}

/// A machine with plenty of memory and mains power.
pub struct DemoMonitor;

impl SystemMonitor for DemoMonitor {
    fn free_memory_bytes(&self) -> Option<u64> {
        Some(12 << 30)
    }

    fn on_battery(&self) -> Option<bool> {
        Some(false)
    }
}

/// Folders have no uncommitted changes.
pub struct DemoWorkspace;

impl Workspace for DemoWorkspace {
    fn git_diff(&self, _cwd: &Path) -> AppResult<String> {
        Ok(String::new())
    }
}

/// No machine-wide telemetry: the demo shows what its own jobs report.
pub struct DemoFeed;

impl TelemetryFeed for DemoFeed {
    fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot::default()
    }

    fn focus(&self, _cwd: &Path) {}
}
