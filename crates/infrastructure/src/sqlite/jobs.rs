use rusqlite::{OptionalExtension, params};
use swarmux_application::AppResult;
use swarmux_application::ports::JobRepository;
use swarmux_domain::{JobId, JobResult, JobStatus, Millis, SessionId};

use super::{OrStorage, SqliteStore};

/// Max bytes of an event payload kept in the DB (the full text stays in Claude Code's transcript).
pub const PAYLOAD_CAP: usize = 8192;
const TRUNCATED: &str = "…[truncated]";

fn cap(payload: &str) -> String {
    if payload.len() <= PAYLOAD_CAP {
        return payload.to_string();
    }
    let mut end = PAYLOAD_CAP;
    while !payload.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &payload[..end])
}

impl JobRepository for SqliteStore {
    fn insert(
        &self,
        session: SessionId,
        prompt: &str,
        model: &str,
        now: Millis,
    ) -> AppResult<JobId> {
        self.conn
            .execute(
                "INSERT INTO jobs (body_rendered, session_id, model, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    prompt,
                    session.to_string(),
                    model,
                    JobStatus::Queued.as_str(),
                    now
                ],
            )
            .or_storage()?;
        Ok(JobId(self.conn.last_insert_rowid()))
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
        let s: String = self
            .conn
            .query_row("SELECT status FROM jobs WHERE id = ?1", [id.0], |r| {
                r.get(0)
            })
            .or_storage()?;
        Ok(JobStatus::parse(&s)?)
    }

    fn save_result(&self, id: JobId, r: &JobResult) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO job_results
                 (job_id, tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id.0,
                    r.tokens_in as i64,
                    r.tokens_out as i64,
                    r.cache_read as i64,
                    r.cost_usd,
                    r.duration_ms as i64,
                    r.outcome,
                    r.error
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn result(&self, id: JobId) -> AppResult<Option<JobResult>> {
        self.conn
            .query_row(
                "SELECT tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error
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
                params![id.0, now, kind, actor, cap(payload)],
            )
            .or_storage()?;
        Ok(())
    }

    fn interrupt_running(&self, now: Millis) -> AppResult<usize> {
        self.conn
            .execute(
                "UPDATE jobs SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
                [now],
            )
            .or_storage()
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

    pub(crate) fn events_for_job(&self, id: JobId) -> Vec<(String, String, String)> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, actor, payload_json FROM events WHERE job_id = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map([id.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}
