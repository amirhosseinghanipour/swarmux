//! State of the Focus screen: which job it follows.

use swarmux_domain::{JobId, JobProgress};

/// Without a followed job the screen shows the most recent one.
#[derive(Default)]
pub struct DashboardState {
    pub followed: Option<JobId>,
    /// A job from an earlier run, rebuilt from history.
    pub replay: Option<JobProgress>,
    /// The review run of the replayed job, if it had one.
    pub replay_review: Option<JobProgress>,
    /// The replayed job's prompt and final answer, from history.
    pub replay_prompt: Option<String>,
    pub replay_answer: Option<String>,
    /// Draw the panel grid of earlier versions instead of Focus.
    pub classic: bool,
}

impl DashboardState {
    pub fn follow(&mut self, job: JobId, replay: Option<(JobProgress, Option<JobProgress>)>) {
        self.followed = Some(job);
        let (replay, review) = replay.map_or((None, None), |(j, r)| (Some(j), r));
        self.replay = replay;
        self.replay_review = review;
        self.replay_prompt = None;
        self.replay_answer = None;
    }

    /// The review run of the replayed job.
    pub fn replayed_review(&self) -> Option<&JobProgress> {
        self.replayed().and(self.replay_review.as_ref())
    }

    /// The replayed progress of the followed job, if that is what is shown.
    pub fn replayed(&self) -> Option<&JobProgress> {
        self.replay
            .as_ref()
            .filter(|p| Some(p.job_id) == self.followed)
    }
}
