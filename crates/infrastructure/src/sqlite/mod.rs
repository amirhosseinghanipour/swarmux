//! SQLite adapter for the storage ports.

mod jobs;
mod prompts;
mod sessions;

use std::path::Path;

use rusqlite::Connection;
use swarmux_application::{AppError, AppResult};

pub use jobs::PAYLOAD_CAP;

pub struct SqliteStore {
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

/// Maps adapter errors into the application's error type.
pub(crate) trait OrStorage<T> {
    fn or_storage(self) -> AppResult<T>;
}

impl<T> OrStorage<T> for rusqlite::Result<T> {
    fn or_storage(self) -> AppResult<T> {
        self.map_err(|e| AppError::Storage(e.to_string()))
    }
}

impl SqliteStore {
    pub fn open(path: &Path) -> AppResult<SqliteStore> {
        let conn = Connection::open(path)
            .map_err(|e| AppError::Storage(format!("cannot open {}: {e}", path.display())))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .or_storage()?;
        SqliteStore::init(conn)
            .map_err(|e| AppError::Storage(format!("database {}: {e}", path.display())))
    }

    pub fn open_in_memory() -> AppResult<SqliteStore> {
        SqliteStore::init(Connection::open_in_memory().or_storage()?)
    }

    fn init(conn: Connection) -> AppResult<SqliteStore> {
        conn.pragma_update(None, "foreign_keys", "ON")
            .or_storage()?;
        let mut store = SqliteStore { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> AppResult<()> {
        let current = self.schema_version()? as usize;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction().or_storage()?;
            tx.execute_batch(sql).map_err(|e| {
                AppError::Storage(format!("migration to version {} failed: {e}", i + 1))
            })?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)
                .or_storage()?;
            tx.commit().or_storage()?;
        }
        Ok(())
    }

    /// Current schema version (`PRAGMA user_version`).
    pub fn schema_version(&self) -> AppResult<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ports::{JobRepository, SessionRepository};
    use swarmux_domain::{JobId, JobResult, JobStatus, Session, SessionId};

    fn session(name: &str, now: i64) -> Session {
        Session::new(SessionId::random(), name, "/w".into(), "sonnet", now).unwrap()
    }

    fn store_with_job() -> (SqliteStore, Session, JobId) {
        let s = SqliteStore::open_in_memory().unwrap();
        let sess = session("a", 1);
        SessionRepository::insert(&s, &sess).unwrap();
        let job = JobRepository::insert(&s, sess.id, None, "hi", "sonnet", 2).unwrap();
        (s, sess, job)
    }

    fn names(s: &SqliteStore) -> Vec<String> {
        s.list_active()
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect()
    }

    #[test]
    fn migrate_sets_schema_version_1() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(SqliteStore::open(&path).unwrap());
        assert_eq!(
            SqliteStore::open(&path).unwrap().schema_version().unwrap(),
            1
        );
    }

    #[test]
    fn sessions_round_trip_and_order_by_activity() {
        let s = SqliteStore::open_in_memory().unwrap();
        let a = session("a", 10);
        let b = session("b", 20);
        SessionRepository::insert(&s, &a).unwrap();
        SessionRepository::insert(&s, &b).unwrap();
        assert_eq!(names(&s), ["b", "a"]);
        s.touch(a.id, 30).unwrap();
        assert_eq!(names(&s), ["a", "b"]);
        let got = s.get(a.id).unwrap().unwrap();
        assert_eq!(got.last_used_at, Some(30));
        assert_eq!(got.cwd, a.cwd);
        assert_eq!(s.get(SessionId::random()).unwrap(), None);
    }

    #[test]
    fn has_started_needs_an_init_event() {
        let (s, sess, job) = store_with_job();
        assert!(!s.has_started(sess.id).unwrap());
        s.append_event(job, "init", "lead", "m", 3).unwrap();
        assert!(s.has_started(sess.id).unwrap());
    }

    #[test]
    fn job_lifecycle_sets_timestamps() {
        let (s, _, job) = store_with_job();
        assert_eq!(s.status(job).unwrap(), JobStatus::Queued);
        s.set_status(job, JobStatus::Running, 5).unwrap();
        assert_eq!(s.job_times(job), (Some(5), None));
        s.set_status(job, JobStatus::Done, 9).unwrap();
        assert_eq!(s.job_times(job), (Some(5), Some(9)));
        assert_eq!(s.status(job).unwrap(), JobStatus::Done);
    }

    #[test]
    fn event_payload_is_capped() {
        let (s, _, job) = store_with_job();
        s.append_event(job, "text", "lead", &"x".repeat(20_000), 3)
            .unwrap();
        let (_, _, payload) = s.events_for_job(job).remove(0);
        assert!(payload.ends_with("…[truncated]"));
        assert!(payload.len() <= PAYLOAD_CAP + "…[truncated]".len());
    }

    #[test]
    fn event_payload_cap_respects_utf8() {
        let (s, _, job) = store_with_job();
        s.append_event(job, "text", "lead", &"é".repeat(5000), 3)
            .unwrap();
        let (_, _, payload) = s.events_for_job(job).remove(0);
        assert!(
            payload
                .trim_end_matches("…[truncated]")
                .chars()
                .all(|c| c == 'é')
        );
    }

    #[test]
    fn result_round_trips() {
        let (s, _, job) = store_with_job();
        let r = JobResult {
            tokens_in: 4,
            tokens_out: 385,
            cache_read: 49753,
            cost_usd: 0.0108,
            duration_ms: 44116,
            outcome: "success".into(),
            error: Some("x".into()),
        };
        s.save_result(job, &r).unwrap();
        assert_eq!(s.result(job).unwrap(), Some(r));
    }

    #[test]
    fn interrupt_running_jobs() {
        let (s, _, job) = store_with_job();
        s.set_status(job, JobStatus::Running, 3).unwrap();
        assert_eq!(s.interrupt_running(4).unwrap(), 1);
        assert_eq!(s.status(job).unwrap(), JobStatus::Interrupted);
    }
}
