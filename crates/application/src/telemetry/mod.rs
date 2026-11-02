//! Telemetry use case: one snapshot of the machine-wide sources and of what
//! swarmux's own jobs did.

mod snapshot;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use swarmux_domain::telemetry::HOUR;

use crate::JobService;
use crate::ports::{Clock, SessionRepository, TelemetryFeed};

pub use snapshot::{
    ActivityView, Battery, CiRun, Collected, Commit, DiffStat, DiskUsage, EnvironmentView,
    GitStatus, GithubStatus, LimitView, ProcessStat, PullRequest, SystemStats, TelemetrySnapshot,
};

/// Calls in the ticker.
pub const TICKER_LEN: usize = 20;
/// Files in the most-touched list.
pub const TOP_FILES: usize = 10;
/// Environments listed.
pub const ENVIRONMENTS: usize = 20;

/// Combines the background feed with job activity.
pub struct TelemetryService {
    feed: Rc<dyn TelemetryFeed>,
    sessions: Rc<dyn SessionRepository>,
    clock: Rc<dyn Clock>,
}

impl TelemetryService {
    pub fn new(
        feed: Rc<dyn TelemetryFeed>,
        sessions: Rc<dyn SessionRepository>,
        clock: Rc<dyn Clock>,
    ) -> TelemetryService {
        TelemetryService {
            feed,
            sessions,
            clock,
        }
    }

    /// Points the git and GitHub sources at `cwd`.
    pub fn focus(&self, cwd: &Path) {
        self.feed.focus(cwd);
    }

    /// Everything the stations show. Per-job data (agent tree, todos, the
    /// job's own token flow) comes from [`JobService::job_activity`] and
    /// [`JobService::agent_tree`].
    pub fn snapshot(&self, jobs: &JobService) -> TelemetrySnapshot {
        let now = self.clock.now();
        let mut snap = self.feed.snapshot();
        snap.taken_at = now;
        if let Some(report) = snap.usage.data.as_mut() {
            // Swarmux session ids are the Claude Code session ids.
            let ours: HashSet<String> = self
                .sessions
                .list_active()
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.id.to_string())
                .collect();
            for s in &mut report.sessions {
                s.swarmux = ours.contains(&s.session_id);
            }
        }
        let activity = jobs.activity();
        snap.limits = activity
            .limits()
            .into_iter()
            .map(|l| LimitView::new(l, now, snap.offset_min))
            .collect();
        let flow = activity.flow.series(now);
        snap.activity = ActivityView {
            cache_hit_ratio: swarmux_domain::telemetry::cache_hit_ratio(&flow),
            flow,
            tools: activity.tools.stats(Some(now - HOUR)),
            ticker: activity.tools.ticker(TICKER_LEN),
            files: activity.tools.top_files(TOP_FILES),
        };
        snap.environments = activity
            .environments()
            .into_iter()
            .take(ENVIRONMENTS)
            .map(|(env, at)| EnvironmentView {
                environment: env.clone(),
                seen_at: *at,
            })
            .collect();
        snap
    }
}
