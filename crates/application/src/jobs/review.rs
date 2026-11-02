//! The review step: queue a read-only reviewer after a job succeeds, and
//! file its verdict on that job when it ends.

use swarmux_domain::{Job, JobId, JobResult, JobStatus, ReviewState, parse_review, review_prompt};

use super::JobService;
use crate::AppResult;
use crate::ports::NewJob;

impl JobService {
    /// Queues the review of the job at `i`, which just finished with `result`.
    /// A failed job is not reviewed. Problems end up in the review state,
    /// never in the reviewed job's own result.
    pub(super) fn start_review(&mut self, i: usize, result: &JobResult) -> AppResult<()> {
        let tick = self.tick_of(i);
        let t = &mut self.tracked[i];
        if t.progress.status != JobStatus::Done {
            t.progress.set_review(None, tick);
            return Ok(());
        }
        let Some(workspace) = self.workspace.clone() else {
            return Ok(());
        };
        let job = t.job.clone();
        let cwd = match &t.cwd {
            Some(cwd) => cwd.clone(),
            None => match self.sessions.get(job.session_id)? {
                Some(s) => s.cwd,
                None => return self.fail_review(i, "session not found".into(), None, 0.0),
            },
        };
        let diff = match workspace.git_diff(&cwd) {
            Ok(diff) => diff,
            Err(e) => return self.fail_review(i, e.to_string(), None, 0.0),
        };
        let new = NewJob {
            session_id: job.session_id,
            prompt_id: None,
            prompt: review_prompt(job.id, &job.prompt, &result.text, &diff),
            model: self.review.model.clone(),
            priority: job.priority,
            fork: false,
            wants_review: false,
            review_of: Some(job.id),
        };
        let now = self.clock.now();
        let review_id = self.jobs.insert(&new, now)?;
        let session_name = self.tracked[i].progress.session_name.clone();
        self.track(
            Job {
                id: review_id,
                session_id: new.session_id,
                prompt_id: None,
                prompt: new.prompt,
                model: new.model,
                priority: new.priority,
                fork: false,
                status: JobStatus::Queued,
                created_at: now,
                wants_review: false,
                review_of: Some(job.id),
            },
            &session_name,
        );
        self.set_review_state(job.id, ReviewState::Running { job: review_id })
    }

    /// Files the verdict of the review job at `i` on the job it audited.
    pub(super) fn file_review(&mut self, i: usize, of: JobId, result: &JobResult) -> AppResult<()> {
        let review_job = self.tracked[i].job.id;
        let status = self.tracked[i].progress.status;
        let state = if status == JobStatus::Done {
            match parse_review(&result.text) {
                Ok(review) => ReviewState::Done {
                    review,
                    job: review_job,
                    cost_usd: result.cost_usd,
                },
                Err(why) => ReviewState::Failed {
                    error: why,
                    job: Some(review_job),
                    cost_usd: result.cost_usd,
                },
            }
        } else {
            ReviewState::Failed {
                error: result
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("review {}", status.as_str())),
                job: Some(review_job),
                cost_usd: result.cost_usd,
            }
        };
        self.set_review_state(of, state)
    }

    fn fail_review(
        &mut self,
        i: usize,
        error: String,
        job: Option<JobId>,
        cost_usd: f64,
    ) -> AppResult<()> {
        let id = self.tracked[i].job.id;
        let state = ReviewState::Failed {
            error,
            job,
            cost_usd,
        };
        self.set_review_state(id, state)
    }

    /// Stores `state` for `job` and shows it on the job's progress.
    fn set_review_state(&mut self, job: JobId, state: ReviewState) -> AppResult<()> {
        if let Some(j) = self.find(job) {
            let tick = self.tick_of(j);
            self.tracked[j]
                .progress
                .set_review(Some(state.clone()), tick);
        }
        self.jobs.set_review(job, &state)
    }
}

#[cfg(test)]
mod tests;
