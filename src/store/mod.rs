//! SQLite persistence (spec section 9).

mod jobs;
mod sessions;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::Connection;

pub use jobs::{JobResult, JobStatus, PAYLOAD_CAP};
pub use sessions::Session;

pub struct Store {
    conn: Connection,
}

const SCHEMA_V1: &str = "
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    cwd TEXT NOT NULL,
    model TEXT NOT NULL,
    permission_mode TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    archived INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE prompts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    body TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    prompt_id INTEGER REFERENCES prompts(id),
    body_rendered TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    model TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    review TEXT,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER
);
CREATE TABLE job_results (
    job_id INTEGER PRIMARY KEY REFERENCES jobs(id),
    tokens_in INTEGER NOT NULL,
    tokens_out INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cost_usd REAL NOT NULL,
    duration_ms INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    error TEXT
);
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    job_id INTEGER NOT NULL REFERENCES jobs(id),
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE INDEX events_job ON events(job_id);
CREATE TABLE decisions (
    id INTEGER PRIMARY KEY,
    job_id INTEGER REFERENCES jobs(id),
    fork TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    output TEXT NOT NULL,
    confidence REAL,
    rule TEXT,
    latency_us INTEGER NOT NULL,
    ts INTEGER NOT NULL
);
";

/// Migrations in order; index + 1 is the resulting `user_version`.
const MIGRATIONS: &[&str] = &[SCHEMA_V1];

impl Store {
    pub fn open(path: &Path) -> anyhow::Result<Store> {
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Store::init(conn).with_context(|| format!("database {}", path.display()))
    }

    pub fn open_in_memory() -> anyhow::Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Store> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        let current = self.user_version()? as usize;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)
                .with_context(|| format!("migration to version {} failed", i + 1))?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Current schema version (`PRAGMA user_version`).
    pub fn user_version(&self) -> anyhow::Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Jobs left `running` by a crash or quit become `interrupted`.
    pub fn recover(&self) -> anyhow::Result<usize> {
        Ok(self.conn.execute(
            "UPDATE jobs SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
            [now_ms()],
        )?)
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_job() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        let sess = s.create_session("a", Path::new("/work"), "sonnet").unwrap();
        let job = s.insert_job(sess.id, "hi", "sonnet").unwrap();
        (s, job)
    }

    fn names(s: &Store) -> Vec<String> {
        s.list_sessions()
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect()
    }

    #[test]
    fn migrate_sets_user_version_1() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.user_version().unwrap(), 1);
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(Store::open(&path).unwrap());
        let s = Store::open(&path).unwrap();
        assert_eq!(s.user_version().unwrap(), 1);
    }

    #[test]
    fn create_and_list_sessions() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_session("a", Path::new("/w"), "sonnet").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.create_session("b", Path::new("/w"), "sonnet").unwrap();
        assert_eq!(names(&s), ["b", "a"]);
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.touch_session(a.id).unwrap();
        assert_eq!(names(&s), ["a", "b"]);
        assert_eq!(a.permission_mode, "default");
    }

    #[test]
    fn job_lifecycle_sets_timestamps() {
        let (s, job) = store_with_job();
        assert_eq!(s.job_status(job).unwrap(), JobStatus::Queued);
        s.set_job_status(job, JobStatus::Running).unwrap();
        let (started, ended) = s.job_times(job).unwrap();
        assert!(started.is_some() && ended.is_none());
        s.set_job_status(job, JobStatus::Done).unwrap();
        let (_, ended) = s.job_times(job).unwrap();
        assert!(ended.is_some());
        assert_eq!(s.job_status(job).unwrap(), JobStatus::Done);
    }

    #[test]
    fn session_has_jobs_reflects_inserts() {
        let s = Store::open_in_memory().unwrap();
        let sess = s.create_session("a", Path::new("/w"), "sonnet").unwrap();
        assert!(!s.session_has_jobs(sess.id).unwrap());
        s.insert_job(sess.id, "x", "sonnet").unwrap();
        assert!(s.session_has_jobs(sess.id).unwrap());
    }

    #[test]
    fn event_payload_is_capped() {
        let (s, job) = store_with_job();
        s.insert_event(job, "text", "lead", &"x".repeat(20_000))
            .unwrap();
        let (_, _, payload) = s.events_for_job(job).unwrap().remove(0);
        assert!(payload.ends_with("…[truncated]"));
        assert!(payload.len() <= PAYLOAD_CAP + "…[truncated]".len());
    }

    #[test]
    fn event_payload_cap_respects_utf8() {
        let (s, job) = store_with_job();
        s.insert_event(job, "text", "lead", &"é".repeat(5000))
            .unwrap();
        let (_, _, payload) = s.events_for_job(job).unwrap().remove(0);
        assert!(payload.ends_with("…[truncated]"));
        assert!(
            payload
                .trim_end_matches("…[truncated]")
                .chars()
                .all(|c| c == 'é')
        );
    }

    #[test]
    fn save_result_round_trips() {
        let (s, job) = store_with_job();
        let r = JobResult {
            tokens_in: 4,
            tokens_out: 385,
            cache_read: 49753,
            cost_usd: 0.0108,
            duration_ms: 44116,
            outcome: "success".into(),
            error: None,
        };
        s.save_result(job, &r).unwrap();
        assert_eq!(s.job_result(job).unwrap(), Some(r));
    }

    #[test]
    fn recover_marks_running_jobs_interrupted() {
        let (s, job) = store_with_job();
        s.set_job_status(job, JobStatus::Running).unwrap();
        assert_eq!(s.recover().unwrap(), 1);
        assert_eq!(s.job_status(job).unwrap(), JobStatus::Interrupted);
    }
}
