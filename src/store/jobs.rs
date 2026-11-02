use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use super::{Store, now_ms};

/// Max bytes of an event payload kept in the DB (the full text stays in Claude Code's transcript).
pub const PAYLOAD_CAP: usize = 8192;
const TRUNCATED: &str = "…[truncated]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Interrupted,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<JobStatus> {
        Some(match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "failed" => JobStatus::Failed,
            "interrupted" => JobStatus::Interrupted,
            _ => return None,
        })
    }

    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Failed | JobStatus::Interrupted
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobResult {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub outcome: String,
    pub error: Option<String>,
}

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

impl Store {
    pub fn insert_job(&self, session_id: Uuid, body: &str, model: &str) -> anyhow::Result<i64> {
        self.conn.execute(
            "INSERT INTO jobs (body_rendered, session_id, model, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                body,
                session_id.to_string(),
                model,
                JobStatus::Queued.as_str(),
                now_ms()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_job_status(&self, id: i64, s: JobStatus) -> anyhow::Result<()> {
        let now = now_ms();
        match s {
            JobStatus::Running => self.conn.execute(
                "UPDATE jobs SET status = ?1, started_at = ?2 WHERE id = ?3",
                params![s.as_str(), now, id],
            )?,
            _ if s.is_finished() => self.conn.execute(
                "UPDATE jobs SET status = ?1, ended_at = ?2 WHERE id = ?3",
                params![s.as_str(), now, id],
            )?,
            _ => self.conn.execute(
                "UPDATE jobs SET status = ?1 WHERE id = ?2",
                params![s.as_str(), id],
            )?,
        };
        Ok(())
    }

    pub fn job_status(&self, id: i64) -> anyhow::Result<JobStatus> {
        let s: String =
            self.conn
                .query_row("SELECT status FROM jobs WHERE id = ?1", [id], |r| r.get(0))?;
        JobStatus::parse(&s).ok_or_else(|| anyhow::anyhow!("unknown job status {s:?}"))
    }

    #[cfg(test)]
    pub(crate) fn job_times(&self, id: i64) -> anyhow::Result<(Option<i64>, Option<i64>)> {
        Ok(self.conn.query_row(
            "SELECT started_at, ended_at FROM jobs WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    pub fn save_result(&self, job_id: i64, r: &JobResult) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO job_results
             (job_id, tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                job_id,
                r.tokens_in as i64,
                r.tokens_out as i64,
                r.cache_read as i64,
                r.cost_usd,
                r.duration_ms as i64,
                r.outcome,
                r.error
            ],
        )?;
        Ok(())
    }

    pub fn job_result(&self, job_id: i64) -> anyhow::Result<Option<JobResult>> {
        Ok(self
            .conn
            .query_row(
                "SELECT tokens_in, tokens_out, cache_read, cost_usd, duration_ms, outcome, error
                 FROM job_results WHERE job_id = ?1",
                [job_id],
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
            .optional()?)
    }

    pub fn insert_event(
        &self,
        job_id: i64,
        kind: &str,
        actor: &str,
        payload: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO events (job_id, ts, kind, actor, payload_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, now_ms(), kind, actor, cap(payload)],
        )?;
        Ok(())
    }

    pub fn events_for_job(&self, job_id: i64) -> anyhow::Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, actor, payload_json FROM events WHERE job_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([job_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
