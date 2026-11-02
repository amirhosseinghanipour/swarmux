use std::rc::Rc;

use swarmux_domain::{JobId, JobProgress, JobResult, JobStatus, Millis, SessionId};

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
}

/// Use cases for past jobs.
pub struct HistoryService {
    jobs: Rc<dyn JobRepository>,
}

impl HistoryService {
    pub fn new(jobs: Rc<dyn JobRepository>) -> HistoryService {
        HistoryService { jobs }
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
        })
    }

    /// Rebuilds a job's progress from what was stored, to view it again.
    pub fn replay(&self, id: JobId) -> AppResult<JobProgress> {
        let s = self.jobs.summary(id)?.ok_or(AppError::JobNotFound)?;
        let mut p = JobProgress::new(id, &s.session_name, &s.model);
        let start = s.started_at.unwrap_or(s.created_at);
        for ev in self.jobs.events(id)? {
            let tick = ((ev.ts - start).max(0) / 100) as u32;
            p.replay(&ev.actor, &ev.kind, &ev.payload, tick);
        }
        p.status = s.status;
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
    fn preview_is_first_line_cut() {
        let (store, s) = store();
        add(&store, &s, "abcdef\nsecond", 1);
        let row = HistoryService::new(store).list("").unwrap().remove(0);
        assert_eq!(row.preview(3), "abc…");
        assert_eq!(row.preview(10), "abcdef");
    }
}
