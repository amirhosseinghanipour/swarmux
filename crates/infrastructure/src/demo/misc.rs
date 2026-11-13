use std::cell::RefCell;
use std::path::Path;

use swarmux_application::ports::{SystemMonitor, TelemetryFeed, Workspace};
use swarmux_application::{
    AppResult, MissionStore, PlanRequest, PlanSink, PlanStore, Planner, TelemetrySnapshot,
};
use swarmux_domain::Millis;
use swarmux_domain::mission::Mission;
use swarmux_domain::plan::PlanRecord;

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

impl DemoMissions {
    /// The store with `mission` already in it, as if it had been kept by an
    /// earlier run.
    pub fn with(self, mission: Mission) -> DemoMissions {
        self.rows.borrow_mut().push(mission);
        self
    }
}

/// Plans kept in memory; they last as long as the demo.
#[derive(Default)]
pub struct DemoPlans {
    rows: RefCell<Vec<(String, PlanRecord)>>,
}

impl DemoPlans {
    /// The store with a plan record already in it, as if it had been kept by
    /// an earlier run.
    pub fn with(self, slug: &str, record: PlanRecord) -> DemoPlans {
        self.rows.borrow_mut().push((slug.to_string(), record));
        self
    }
}

impl PlanStore for DemoPlans {
    fn save(&self, slug: &str, record: &PlanRecord, _at: Millis) -> AppResult<()> {
        let mut rows = self.rows.borrow_mut();
        match rows.iter_mut().find(|(s, _)| s == slug) {
            Some(row) => row.1 = record.clone(),
            None => rows.push((slug.to_string(), record.clone())),
        }
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<(String, PlanRecord)>> {
        Ok(self.rows.borrow().clone())
    }
}

/// A planner that is never asked: the demo's plans are written in advance.
pub struct DemoPlanner;

impl Planner for DemoPlanner {
    fn plan(&self, _request: PlanRequest, _sink: PlanSink) {}
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
