use rusqlite::{OptionalExtension, Row, params};
use swarmux_application::ports::{JobRepository, NewJob, StoredEvent};
use swarmux_application::{AppError, AppResult, JobSummary};
use swarmux_domain::{Job, JobId, JobResult, JobStatus, Millis, PromptId, ReviewState, SessionId};

use super::sessions::session_id_at;
use super::{OrStorage, SqliteStore};

/// Max bytes of an event payload kept in the DB (the full text stays in Claude Code's transcript).
pub const PAYLOAD_CAP: usize = 8192;
/// Max bytes of a job's final answer kept in the DB.
pub const TEXT_CAP: usize = 64 * 1024;
const TRUNCATED: &str = "…[truncated]";

fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &text[..end])
}

fn status_at(r: &Row, idx: usize) -> rusqlite::Result<JobStatus> {
    let s: String = r.get(idx)?;
    JobStatus::parse(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

const JOB_COLS: &str = "id, session_id, prompt_id, body_rendered, model, priority, fork, status, \
     created_at, wants_review, review_of";

fn job_from_row(r: &Row) -> rusqlite::Result<Job> {
    Ok(Job {
        id: JobId(r.get(0)?),
        session_id: session_id_at(r, 1)?,
        prompt_id: r.get::<_, Option<i64>>(2)?.map(PromptId),
        prompt: r.get(3)?,
        model: r.get(4)?,
        priority: r.get(5)?,
        fork: r.get(6)?,
        status: status_at(r, 7)?,
        created_at: r.get(8)?,
        wants_review: r.get(9)?,
        review_of: r.get::<_, Option<i64>>(10)?.map(JobId),
    })
}

const SUMMARY_SELECT: &str = "
SELECT j.id, j.session_id, COALESCE(s.name, ''), j.body_rendered, j.model, j.status,
       r.cost_usd, r.tokens_in, r.tokens_out, r.duration_ms, j.created_at, j.started_at, r.error,
       j.review_of, r.cache_read, r.cache_creation
FROM jobs j
LEFT JOIN sessions s ON s.id = j.session_id
LEFT JOIN job_results r ON r.job_id = j.id";

fn summary_from_row(r: &Row) -> rusqlite::Result<JobSummary> {
    Ok(JobSummary {
        job_id: JobId(r.get(0)?),
        session_id: session_id_at(r, 1)?,
        session_name: r.get(2)?,
        prompt: r.get(3)?,
        model: r.get(4)?,
        status: status_at(r, 5)?,
        cost_usd: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
        tokens_in: r.get::<_, Option<i64>>(7)?.unwrap_or(0) as u64,
        tokens_out: r.get::<_, Option<i64>>(8)?.unwrap_or(0) as u64,
        duration_ms: r.get::<_, Option<i64>>(9)?.map(|d| d as u64),
        created_at: r.get(10)?,
        started_at: r.get(11)?,
        error: r.get(12)?,
        review_of: r.get::<_, Option<i64>>(13)?.map(JobId),
        cache_ratio: {
            let read = r.get::<_, Option<i64>>(14)?;
            let created = r.get::<_, Option<i64>>(15)?;
            let input = r.get::<_, Option<i64>>(7)?.unwrap_or(0);
            read.and_then(|read| {
                swarmux_domain::cache_read_ratio(
                    input as u64,
                    read as u64,
                    created.map(|c| c as u64),
                )
            })
        },
    })
}

impl JobRepository for SqliteStore {
    fn insert(&self, job: &NewJob, now: Millis) -> AppResult<JobId> {
        self.conn
            .execute(
                "INSERT INTO jobs (prompt_id, body_rendered, session_id, model, priority, fork, status,
                                   created_at, wants_review, review_of)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    job.prompt_id.map(|p| p.0),
                    job.prompt,
                    job.session_id.to_string(),
                    job.model,
                    job.priority,
                    job.fork,
                    JobStatus::Queued.as_str(),
                    now,
                    job.wants_review,
                    job.review_of.map(|j| j.0)
                ],
            )
            .or_storage()?;
        Ok(JobId(self.conn.last_insert_rowid()))
    }

    fn get(&self, id: JobId) -> AppResult<Option<Job>> {
        self.conn
            .query_row(
                &format!("SELECT {JOB_COLS} FROM jobs WHERE id = ?1"),
                [id.0],
                job_from_row,
            )
            .optional()
            .or_storage()
    }

    fn queued(&self) -> AppResult<Vec<Job>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {JOB_COLS} FROM jobs WHERE status = ?1"))
            .or_storage()?;
        let rows = stmt
            .query_map([JobStatus::Queued.as_str()], job_from_row)
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn set_status(&self, id: JobId, status: JobStatus, now: Millis) -> AppResult<()> {
        let result = match status {
            JobStatus::Running => self.conn.execute(
                "UPDATE jobs SET status = ?1, started_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.0],
            ),
            s if s.is_finished() => self.conn.execute(
                "UPDATE jobs SET status = ?1, ended_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.0],
            ),
            _ => self.conn.execute(
                "UPDATE jobs SET status = ?1 WHERE id = ?2",
                params![status.as_str(), id.0],
            ),
        };
        result.or_storage()?;
        Ok(())
    }

    fn status(&self, id: JobId) -> AppResult<JobStatus> {
        self.conn
            .query_row("SELECT status FROM jobs WHERE id = ?1", [id.0], |r| {
                status_at(r, 0)
            })
            .or_storage()
    }

    fn set_priority(&self, id: JobId, priority: i32) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE jobs SET priority = ?1 WHERE id = ?2",
                params![priority, id.0],
            )
            .or_storage()?;
        Ok(())
    }

    fn set_session(&self, id: JobId, session: SessionId) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE jobs SET session_id = ?1 WHERE id = ?2",
                params![session.to_string(), id.0],
            )
            .or_storage()?;
        Ok(())
    }

    fn save_result(&self, id: JobId, r: &JobResult) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO job_results
                 (job_id, tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error, text, cache_creation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id.0,
                    r.tokens_in as i64,
                    r.tokens_out as i64,
                    r.cache_read as i64,
                    r.cost_usd,
                    r.duration_ms as i64,
                    r.outcome,
                    r.error,
                    cap(&r.text, TEXT_CAP),
                    r.cache_creation.map(|n| n as i64)
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn result(&self, id: JobId) -> AppResult<Option<JobResult>> {
        self.conn
            .query_row(
                "SELECT tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error, text, cache_creation
                 FROM job_results WHERE job_id = ?1",
                [id.0],
                |r| {
                    Ok(JobResult {
                        tokens_in: r.get::<_, i64>(0)? as u64,
                        tokens_out: r.get::<_, i64>(1)? as u64,
                        cache_read: r.get::<_, i64>(2)? as u64,
                        cost_usd: r.get(3)?,
                        duration_ms: r.get::<_, i64>(4)? as u64,
                        outcome: r.get(5)?,
                        error: r.get(6)?,
                        text: r.get(7)?,
                        cache_creation: r.get::<_, Option<i64>>(8)?.map(|n| n as u64),
                    })
                },
            )
            .optional()
            .or_storage()
    }

    fn append_event(
        &self,
        id: JobId,
        kind: &str,
        actor: &str,
        payload: &str,
        now: Millis,
    ) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO events (job_id, ts, kind, actor, payload_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id.0, now, kind, actor, cap(payload, PAYLOAD_CAP)],
            )
            .or_storage()?;
        Ok(())
    }

    fn events(&self, id: JobId) -> AppResult<Vec<StoredEvent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ts, kind, actor, payload_json FROM events WHERE job_id = ?1 ORDER BY id",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([id.0], |r| {
                Ok(StoredEvent {
                    ts: r.get(0)?,
                    kind: r.get(1)?,
                    actor: r.get(2)?,
                    payload: r.get(3)?,
                })
            })
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn recent_events(&self, id: JobId, until: Millis, limit: usize) -> AppResult<Vec<StoredEvent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ts, kind, actor, payload_json FROM                  (SELECT id, ts, kind, actor, payload_json FROM events                   WHERE job_id = ?1 AND ts <= ?2 ORDER BY id DESC LIMIT ?3)                  ORDER BY id",
            )
            .or_storage()?;
        let rows = stmt
            .query_map(
                rusqlite::params![id.0, until, i64::try_from(limit).unwrap_or(i64::MAX)],
                |r| {
                    Ok(StoredEvent {
                        ts: r.get(0)?,
                        kind: r.get(1)?,
                        actor: r.get(2)?,
                        payload: r.get(3)?,
                    })
                },
            )
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn interrupt_running(&self, now: Millis) -> AppResult<usize> {
        self.conn
            .execute(
                "UPDATE jobs SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
                [now],
            )
            .or_storage()
    }

    fn summaries(&self, limit: usize) -> AppResult<Vec<JobSummary>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "{SUMMARY_SELECT} ORDER BY j.created_at DESC, j.id DESC LIMIT ?1"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map([limit as i64], summary_from_row)
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn summary(&self, id: JobId) -> AppResult<Option<JobSummary>> {
        self.conn
            .query_row(
                &format!("{SUMMARY_SELECT} WHERE j.id = ?1"),
                [id.0],
                summary_from_row,
            )
            .optional()
            .or_storage()
    }

    fn set_review(&self, id: JobId, review: &ReviewState) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE jobs SET review = ?1 WHERE id = ?2",
                params![review.encode(), id.0],
            )
            .or_storage()?;
        Ok(())
    }

    fn review(&self, id: JobId) -> AppResult<Option<ReviewState>> {
        let text: Option<String> = self
            .conn
            .query_row("SELECT review FROM jobs WHERE id = ?1", [id.0], |r| {
                r.get(0)
            })
            .optional()
            .or_storage()?
            .flatten();
        text.map(|t| ReviewState::decode(&t))
            .transpose()
            .map_err(|e| AppError::Storage(e.to_string()))
    }
}

#[cfg(test)]
impl SqliteStore {
    pub(crate) fn job_times(&self, id: JobId) -> (Option<i64>, Option<i64>) {
        self.conn
            .query_row(
                "SELECT started_at, ended_at FROM jobs WHERE id = ?1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    pub(crate) fn job_prompt(&self, id: JobId) -> Option<i64> {
        self.conn
            .query_row("SELECT prompt_id FROM jobs WHERE id = ?1", [id.0], |r| {
                r.get(0)
            })
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::new_job;
    use swarmux_application::ports::SessionRepository;
    use swarmux_domain::Session;

    fn store() -> (SqliteStore, Session) {
        let s = SqliteStore::open_in_memory().unwrap();
        let sess = Session::new(SessionId::random(), "Demo", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&s, &sess).unwrap();
        (s, sess)
    }

    #[test]
    fn job_round_trips_with_priority_and_fork() {
        let (s, sess) = store();
        let mut new = new_job(sess.id, "fix it");
        new.priority = 3;
        new.fork = true;
        let id = JobRepository::insert(&s, &new, 7).unwrap();
        let job = JobRepository::get(&s, id).unwrap().unwrap();
        assert_eq!(
            (job.priority, job.fork, job.status, job.created_at),
            (3, true, JobStatus::Queued, 7)
        );
        assert_eq!((job.prompt.as_str(), job.session_id), ("fix it", sess.id));
        assert_eq!(JobRepository::get(&s, JobId(99)).unwrap(), None);
    }

    #[test]
    fn queued_priority_and_session_updates() {
        let (s, sess) = store();
        let a = JobRepository::insert(&s, &new_job(sess.id, "a"), 1).unwrap();
        let b = JobRepository::insert(&s, &new_job(sess.id, "b"), 2).unwrap();
        s.set_status(a, JobStatus::Running, 3).unwrap();
        let queued: Vec<JobId> = s.queued().unwrap().into_iter().map(|j| j.id).collect();
        assert_eq!(queued, [b]);

        s.set_priority(b, -2).unwrap();
        let other = Session::new(SessionId::random(), "o", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&s, &other).unwrap();
        s.set_session(b, other.id).unwrap();
        let job = JobRepository::get(&s, b).unwrap().unwrap();
        assert_eq!((job.priority, job.session_id), (-2, other.id));
    }

    #[test]
    fn cancelled_status_round_trips_with_end_time() {
        let (s, sess) = store();
        let id = JobRepository::insert(&s, &new_job(sess.id, "a"), 1).unwrap();
        s.set_status(id, JobStatus::Cancelled, 4).unwrap();
        assert_eq!(s.status(id).unwrap(), JobStatus::Cancelled);
        assert_eq!(s.job_times(id), (None, Some(4)));
    }

    #[test]
    fn events_come_back_in_order() {
        let (s, sess) = store();
        let id = JobRepository::insert(&s, &new_job(sess.id, "a"), 1).unwrap();
        s.append_event(id, "init", "lead", "m", 5).unwrap();
        s.append_event(id, "text", "toolu_A", "hi", 6).unwrap();
        let ev = s.events(id).unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(
            (ev[1].ts, ev[1].kind.as_str(), ev[1].actor.as_str()),
            (6, "text", "toolu_A")
        );
    }

    #[test]
    fn recent_events_are_the_tail_of_the_stream_up_to_a_moment() {
        let (s, sess) = store();
        let id = JobRepository::insert(&s, &new_job(sess.id, "a"), 1).unwrap();
        let other = JobRepository::insert(&s, &new_job(sess.id, "b"), 1).unwrap();
        for (ts, text) in [(10, "one"), (20, "two"), (30, "three"), (40, "four")] {
            s.append_event(id, "text", "lead", text, ts).unwrap();
        }
        s.append_event(other, "text", "lead", "not mine", 25)
            .unwrap();
        let texts = |until, limit| -> Vec<String> {
            s.recent_events(id, until, limit)
                .unwrap()
                .into_iter()
                .map(|e| e.payload)
                .collect()
        };
        // The newest two at or before 30, oldest first.
        assert_eq!(texts(30, 2), ["two", "three"]);
        // Everything when the limit is large; nothing before the first.
        assert_eq!(texts(1_000, 10), ["one", "two", "three", "four"]);
        assert!(texts(5, 10).is_empty());
        // The default (whole stream, filtered) agrees with the index.
        let all = s.events(id).unwrap();
        let upto: Vec<String> = all
            .iter()
            .filter(|e| e.ts <= 30)
            .map(|e| e.payload.clone())
            .collect();
        assert_eq!(upto[upto.len() - 2..], texts(30, 2));
    }

    #[test]
    fn summaries_join_session_and_result_newest_first() {
        let (s, sess) = store();
        let a = JobRepository::insert(&s, &new_job(sess.id, "first"), 1).unwrap();
        let b = JobRepository::insert(&s, &new_job(sess.id, "second"), 2).unwrap();
        s.set_status(a, JobStatus::Running, 3).unwrap();
        s.save_result(
            a,
            &JobResult {
                tokens_in: 10,
                tokens_out: 20,
                cache_read: 0,
                cache_creation: None,
                cost_usd: 0.5,
                duration_ms: 1500,
                outcome: "failed".into(),
                error: Some("boom".into()),
                text: String::new(),
            },
        )
        .unwrap();
        s.set_status(a, JobStatus::Failed, 4).unwrap();

        let rows = s.summaries(10).unwrap();
        let ids: Vec<JobId> = rows.iter().map(|r| r.job_id).collect();
        assert_eq!(ids, [b, a]);
        assert_eq!(rows[0].duration_ms, None);
        assert_eq!(rows[0].cost_usd, 0.0);
        let r = &rows[1];
        assert_eq!(
            (r.session_name.as_str(), r.status, r.cost_usd, r.tokens_out),
            ("Demo", JobStatus::Failed, 0.5, 20)
        );
        assert_eq!(
            (r.duration_ms, r.started_at, r.error.as_deref()),
            (Some(1500), Some(3), Some("boom"))
        );
        assert_eq!(s.summaries(1).unwrap().len(), 1);
        assert_eq!(s.summary(a).unwrap().unwrap().prompt, "first");
        assert_eq!(s.summary(JobId(99)).unwrap(), None);
    }

    #[test]
    fn review_requests_links_and_states_round_trip() {
        use swarmux_domain::{Review, ReviewVerdict};
        let (s, sess) = store();
        let mut new = new_job(sess.id, "fix");
        new.wants_review = true;
        let job = JobRepository::insert(&s, &new, 1).unwrap();
        let mut review = new_job(sess.id, "swarmux review of job #1");
        review.review_of = Some(job);
        let rid = JobRepository::insert(&s, &review, 2).unwrap();

        let got = JobRepository::get(&s, job).unwrap().unwrap();
        assert!(got.wants_review && got.review_of.is_none());
        assert_eq!(
            JobRepository::get(&s, rid).unwrap().unwrap().review_of,
            Some(job)
        );
        assert_eq!(s.summary(rid).unwrap().unwrap().review_of, Some(job));

        assert_eq!(s.review(job).unwrap(), None);
        let state = ReviewState::Done {
            review: Review {
                verdict: ReviewVerdict::Concerns,
                concerns: vec!["no test for the \"empty\" case".into()],
                diffs_checked: 3,
            },
            job: rid,
            cost_usd: 0.031,
        };
        s.set_review(job, &state).unwrap();
        assert_eq!(s.review(job).unwrap(), Some(state));
        assert_eq!(s.review(JobId(99)).unwrap(), None);
    }

    #[test]
    fn long_result_text_is_capped() {
        let (s, sess) = store();
        let id = JobRepository::insert(&s, &new_job(sess.id, "a"), 1).unwrap();
        let mut r = JobResult {
            tokens_in: 0,
            tokens_out: 0,
            cache_read: 0,
            cache_creation: None,
            cost_usd: 0.0,
            duration_ms: 0,
            outcome: "success".into(),
            error: None,
            text: "x".repeat(TEXT_CAP + 10),
        };
        s.save_result(id, &r).unwrap();
        r = s.result(id).unwrap().unwrap();
        assert!(r.text.ends_with(TRUNCATED));
    }
}
