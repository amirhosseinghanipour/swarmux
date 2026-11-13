use std::rc::Rc;

use swarmux_domain::{JobId, JobProgress, JobResult, JobStatus, Millis, ReviewState, SessionId};

use crate::ports::JobRepository;
use crate::{AppError, AppResult};

/// How many recent jobs history loads and searches.
pub const HISTORY_LIMIT: usize = 2000;

/// One row of job history.
#[derive(Clone, Debug, PartialEq)]
pub struct JobSummary {
    pub job_id: JobId,
    pub session_id: SessionId,
    pub session_name: String,
    /// The full prompt text.
    pub prompt: String,
    pub model: String,
    pub status: JobStatus,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub duration_ms: Option<u64>,
    pub created_at: Millis,
    pub started_at: Option<Millis>,
    pub error: Option<String>,
    /// Set on a review job: the job it audits.
    pub review_of: Option<JobId>,
    /// Share of the job's input served from the prompt cache; `None` when
    /// unknown.
    pub cache_ratio: Option<f64>,
}

impl JobSummary {
    /// The prompt's first line, cut to `max` characters.
    pub fn preview(&self, max: usize) -> String {
        let first = self.prompt.lines().next().unwrap_or_default();
        match first.char_indices().nth(max) {
            Some((i, _)) => format!("{}…", &first[..i]),
            None => first.to_string(),
        }
    }

    /// True when the prompt or session name contains `query`, ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.prompt.to_lowercase().contains(&q)
            || self.session_name.to_lowercase().contains(&q)
    }
}

/// A job with everything known about how it ended.
#[derive(Clone, Debug, PartialEq)]
pub struct JobDetail {
    pub summary: JobSummary,
    pub result: Option<JobResult>,
    /// `None` when no review was started for the job.
    pub review: Option<ReviewState>,
}

/// Use cases for past jobs.
pub struct HistoryService {
    jobs: Rc<dyn JobRepository>,
}

impl HistoryService {
    pub fn new(jobs: Rc<dyn JobRepository>) -> HistoryService {
        HistoryService { jobs }
    }

    /// Where the jobs and their streams are stored, for what reads them
    /// without a use case of its own (the console quotes a stream's tail).
    pub fn repository(&self) -> &dyn JobRepository {
        &*self.jobs
    }

    /// Recent jobs matching `query` (prompt or session name), newest first.
    pub fn list(&self, query: &str) -> AppResult<Vec<JobSummary>> {
        let mut rows = self.jobs.summaries(HISTORY_LIMIT)?;
        rows.retain(|r| r.matches(query));
        Ok(rows)
    }

    pub fn detail(&self, id: JobId) -> AppResult<JobDetail> {
        let summary = self.jobs.summary(id)?.ok_or(AppError::JobNotFound)?;
        Ok(JobDetail {
            summary,
            result: self.jobs.result(id)?,
            review: self.review(id)?,
        })
    }

    /// The job's review. One still marked running whose review job has
    /// ended (swarmux quit meanwhile) is shown as failed.
    pub fn review(&self, id: JobId) -> AppResult<Option<ReviewState>> {
        let Some(state) = self.jobs.review(id)? else {
            return Ok(None);
        };
        if let ReviewState::Running { job } = state {
            let status = self.jobs.summary(job)?.map(|s| s.status);
            if status.is_none_or(|s| s.is_finished()) {
                return Ok(Some(ReviewState::Failed {
                    error: "review interrupted".into(),
                    job: Some(job),
                    cost_usd: 0.0,
                }));
            }
        }
        Ok(Some(state))
    }

    /// Rebuilds a job and its review run (if any) to view them again.
    pub fn replay_with_review(&self, id: JobId) -> AppResult<(JobProgress, Option<JobProgress>)> {
        let mut job = self.replay(id)?;
        let review = self.review(id)?;
        let run = match review.as_ref().and_then(ReviewState::job) {
            Some(rid) => {
                let start = |j: JobId| -> AppResult<Option<Millis>> {
                    Ok(self
                        .jobs
                        .summary(j)?
                        .map(|s| s.started_at.unwrap_or(s.created_at)))
                };
                let offset = match (start(id)?, start(rid)?) {
                    (Some(a), Some(b)) => ((b - a).max(0) / 100) as u32,
                    _ => job.last_tick,
                };
                Some(self.rebuild(rid, Some(offset))?)
            }
            None => None,
        };
        let end = run
            .as_ref()
            .map_or(job.last_tick, |r| r.log_offset + r.last_tick);
        job.set_review(review, end);
        Ok((job, run))
    }

    /// Rebuilds a job's progress from what was stored, to view it again.
    pub fn replay(&self, id: JobId) -> AppResult<JobProgress> {
        self.rebuild(id, None)
    }

    /// Replays `id`; with `reviewer`, as a review that started that many
    /// ticks into the job it audits.
    fn rebuild(&self, id: JobId, reviewer: Option<u32>) -> AppResult<JobProgress> {
        let s = self.jobs.summary(id)?.ok_or(AppError::JobNotFound)?;
        let mut p = JobProgress::new(id, &s.session_name, &s.model);
        if let Some(offset) = reviewer {
            p.as_reviewer(offset);
        }
        let start = s.started_at.unwrap_or(s.created_at);
        for ev in self.jobs.events(id)? {
            let tick = ((ev.ts - start).max(0) / 100) as u32;
            p.replay(&ev.actor, &ev.kind, &ev.payload, tick);
        }
        p.finish_replay(s.status);
        p.cost_usd = s.cost_usd;
        p.tokens_in = s.tokens_in;
        p.tokens_out = s.tokens_out;
        if let Some(e) = &s.error {
            p.replay("lead", "error", e, p.last_tick);
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{JobRepository, NewJob, SessionRepository};
    use crate::testing::InMemoryStore;
    use swarmux_domain::Session;

    fn store() -> (Rc<InMemoryStore>, Session) {
        let store = Rc::new(InMemoryStore::default());
        let s = Session::new(SessionId::random(), "Alpha", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(store.as_ref(), &s).unwrap();
        (store, s)
    }

    fn add(store: &InMemoryStore, s: &Session, prompt: &str, now: Millis) -> JobId {
        let job = NewJob {
            session_id: s.id,
            prompt_id: None,
            prompt: prompt.into(),
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        };
        JobRepository::insert(store, &job, now).unwrap()
    }

    #[test]
    fn list_is_newest_first_and_searchable() {
        let (store, s) = store();
        add(&store, &s, "Fix the Bug", 1);
        add(&store, &s, "write docs", 2);
        let svc = HistoryService::new(store);
        let prompts = |q: &str| -> Vec<String> {
            svc.list(q).unwrap().into_iter().map(|r| r.prompt).collect()
        };
        assert_eq!(prompts(""), ["write docs", "Fix the Bug"]);
        assert_eq!(prompts("  bug "), ["Fix the Bug"]);
        assert_eq!(prompts("ALPHA").len(), 2);
        assert!(prompts("nothing").is_empty());
    }

    #[test]
    fn search_ignores_unicode_case() {
        let (store, s) = store();
        add(&store, &s, "Überprüfe das", 1);
        let svc = HistoryService::new(store);
        assert_eq!(svc.list("überPRÜFE").unwrap().len(), 1);
    }

    #[test]
    fn detail_has_result_and_unknown_job_fails() {
        let (store, s) = store();
        let id = add(&store, &s, "p", 1);
        let r = JobResult {
            tokens_in: 1,
            tokens_out: 2,
            cache_read: 0,
            cache_creation: None,
            cost_usd: 0.25,
            duration_ms: 1000,
            outcome: "success".into(),
            error: None,
            text: "answer".into(),
        };
        store.save_result(id, &r).unwrap();
        let svc = HistoryService::new(store);
        let d = svc.detail(id).unwrap();
        assert_eq!(d.result.unwrap().text, "answer");
        assert_eq!(d.summary.cost_usd, 0.25);
        assert!(matches!(svc.detail(JobId(99)), Err(AppError::JobNotFound)));
    }

    #[test]
    fn replay_uses_stored_events() {
        let (store, s) = store();
        let id = add(&store, &s, "p", 1_000);
        store.set_status(id, JobStatus::Running, 1_000).unwrap();
        store.append_event(id, "text", "lead", "hi", 3_000).unwrap();
        store.set_status(id, JobStatus::Done, 4_000).unwrap();
        let p = HistoryService::new(store).replay(id).unwrap();
        assert_eq!(p.status, JobStatus::Done);
        assert_eq!(p.last_tick, 20);
        assert!(p.log[0].contains("hi"));
    }

    #[test]
    fn review_comes_back_with_detail_and_replay() {
        use swarmux_domain::{Review, ReviewVerdict};
        let (store, s) = store();
        let id = add(&store, &s, "p", 1_000);
        store.set_status(id, JobStatus::Running, 1_000).unwrap();
        store.set_status(id, JobStatus::Done, 2_000).unwrap();
        let rid = add(&store, &s, "swarmux review of job #1", 2_500);
        store.set_status(rid, JobStatus::Running, 3_000).unwrap();
        store
            .append_event(rid, "text", "lead", "checking", 3_500)
            .unwrap();
        let svc = HistoryService::new(store.clone());

        store
            .set_review(id, &ReviewState::Running { job: rid })
            .unwrap();
        assert_eq!(
            svc.review(id).unwrap(),
            Some(ReviewState::Running { job: rid })
        );
        store
            .set_status(rid, JobStatus::Interrupted, 4_000)
            .unwrap();
        assert!(matches!(
            svc.detail(id).unwrap().review,
            Some(ReviewState::Failed { ref error, .. }) if error == "review interrupted"
        ));

        let done = ReviewState::Done {
            review: Review {
                verdict: ReviewVerdict::Pass,
                concerns: vec![],
                diffs_checked: 2,
            },
            job: rid,
            cost_usd: 0.02,
        };
        store.set_review(id, &done).unwrap();
        let (job, run) = svc.replay_with_review(id).unwrap();
        assert_eq!(job.review, Some(done));
        let run = run.unwrap();
        assert_eq!(run.log_offset, 20);
        assert!(run.log[0].starts_with("00:02 reviewer"), "{}", run.log[0]);
        assert_eq!(job.phase(), swarmux_domain::Phase::Ship);
        assert_eq!(svc.review(rid).unwrap(), None);
    }

    #[test]
    fn preview_is_first_line_cut() {
        let (store, s) = store();
        add(&store, &s, "abcdef\nsecond", 1);
        let row = HistoryService::new(store).list("").unwrap().remove(0);
        assert_eq!(row.preview(3), "abc…");
        assert_eq!(row.preview(10), "abcdef");
    }
}
