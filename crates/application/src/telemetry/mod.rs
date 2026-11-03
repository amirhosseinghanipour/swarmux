//! Telemetry use case: one snapshot of the machine-wide sources and of what
//! swarmux's own jobs did.

mod agents;
mod snapshot;
#[cfg(test)]
mod tests;
mod truth;

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use swarmux_domain::telemetry::{HOUR, LimitSource, LimitState, window_length};

use crate::JobService;
use crate::ports::{Clock, SessionRepository, TelemetryFeed};

pub use agents::AgentsPoll;
pub use snapshot::{
    ActivityView, Battery, CiRun, Collected, Commit, DiffStat, DiskUsage, EnvironmentView,
    GitStatus, GithubStatus, LimitView, ProcessStat, PullRequest, SystemStats, TelemetrySnapshot,
};
pub use truth::{
    CacheView, CombinedStatusline, InstructionsView, StatuslineFeed, StatuslineView, TruthBoard,
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
    /// Status lines and instruction files the sessions reported.
    truth: Option<(Rc<TruthBoard>, Rc<dyn StatuslineFeed>)>,
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
            truth: None,
        }
    }

    /// Adds what the sessions report themselves: `feed` delivers status
    /// lines into `board`, which the hook bus also fills with the
    /// instruction files sessions load.
    pub fn with_truth(
        mut self,
        board: Rc<TruthBoard>,
        feed: Rc<dyn StatuslineFeed>,
    ) -> TelemetryService {
        self.truth = Some((board, feed));
        self
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
        // Swarmux session ids are the Claude Code session ids.
        let ours: HashSet<String> = self
            .sessions
            .list_active()
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.id.to_string())
            .collect();
        if let Some(report) = snap.usage.data.as_mut() {
            for s in &mut report.sessions {
                s.swarmux = ours.contains(&s.session_id);
            }
        }
        let activity = jobs.activity();
        let reported = self.reported_windows();
        snap.limits = merge_limits(activity.limits(), &reported, now, snap.offset_min);
        if let Some((board, _)) = &self.truth {
            snap.statuslines = board.statuslines();
            snap.instructions = board.instructions();
        }
        // Sessions swarmux runs are shown as jobs, not as foreign sessions.
        if let Some(external) = snap.external.data.as_mut() {
            external.retain(|e| !ours.contains(&e.session_id));
        }
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

impl TelemetryService {
    /// Reads the waiting status lines into the board and returns the
    /// windows they reported.
    fn reported_windows(&self) -> Vec<LimitState> {
        let Some((board, feed)) = &self.truth else {
            return Vec::new();
        };
        for line in feed.poll_statuslines() {
            board.record_statusline(line);
        }
        board.windows()
    }
}

/// One view per window from both sources: the freshest reading wins, and
/// the status line wins a tie. Shortest window first.
fn merge_limits(
    stream: Vec<&LimitState>,
    reported: &[LimitState],
    now: i64,
    offset_min: i32,
) -> Vec<LimitView> {
    let mut views: Vec<LimitView> = Vec::new();
    for state in stream {
        let newer = reported
            .iter()
            .any(|r| r.limit.window == state.limit.window && r.observed_at >= state.observed_at);
        if !newer {
            views.push(LimitView::new(
                state,
                now,
                offset_min,
                LimitSource::RateLimitEvent,
            ));
        }
    }
    for state in reported {
        let older_stream = views
            .iter()
            .any(|v| v.window == state.limit.window && v.source == LimitSource::RateLimitEvent);
        if !older_stream {
            views.push(LimitView::new(
                state,
                now,
                offset_min,
                LimitSource::Statusline,
            ));
        }
    }
    views.sort_by_key(|v| {
        (
            window_length(&v.window).unwrap_or(i64::MAX),
            v.window.clone(),
        )
    });
    views
}
