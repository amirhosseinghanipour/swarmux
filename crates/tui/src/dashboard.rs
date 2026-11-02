//! State of the dashboard: which job it follows.

use swarmux_domain::{JobId, JobProgress};

/// Without a followed job the dashboard shows the most recent one.
#[derive(Default)]
pub struct DashboardState {
    pub followed: Option<JobId>,
    /// A job from an earlier run, rebuilt from history.
    pub replay: Option<JobProgress>,
}

impl DashboardState {
    pub fn follow(&mut self, job: JobId, replay: Option<JobProgress>) {
        self.followed = Some(job);
        self.replay = replay;
    }

    /// The replayed progress of the followed job, if that is what is shown.
    pub fn replayed(&self) -> Option<&JobProgress> {
        self.replay
            .as_ref()
            .filter(|p| Some(p.job_id) == self.followed)
    }
}
