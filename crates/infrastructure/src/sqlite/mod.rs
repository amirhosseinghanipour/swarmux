//! SQLite adapter for the storage ports.

mod consents;
mod decisions;
mod jobs;
mod ledger;
mod prompts;
mod sessions;
mod telemetry;

use std::path::Path;

use rusqlite::Connection;
use swarmux_application::{AppError, AppResult};

pub use jobs::PAYLOAD_CAP;
pub use ledger::SqliteLedger;

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

/// Fork jobs, final answer text, and indexes for the queue and history.
const SCHEMA_V2: &str = "
ALTER TABLE jobs ADD COLUMN fork INTEGER NOT NULL DEFAULT 0;
ALTER TABLE job_results ADD COLUMN text TEXT NOT NULL DEFAULT '';
CREATE INDEX jobs_status ON jobs(status);
CREATE INDEX jobs_created ON jobs(created_at);
";

/// What each guard decision was about.
const SCHEMA_V3: &str = "
ALTER TABLE decisions ADD COLUMN tool TEXT NOT NULL DEFAULT '';
ALTER TABLE decisions ADD COLUMN subject TEXT NOT NULL DEFAULT '';
CREATE INDEX decisions_ts ON decisions(ts);
";

/// Lean sessions (jobs without the user's own Claude Code settings).
const SCHEMA_V4: &str = "
ALTER TABLE sessions ADD COLUMN lean INTEGER NOT NULL DEFAULT 0;
";

/// JEV providers the user agreed to send decision context to.
const SCHEMA_V5: &str = "
CREATE TABLE jev_consent (
    provider TEXT PRIMARY KEY,
    granted_at INTEGER NOT NULL
);
";

/// Review step: which jobs want a review and which jobs are reviews.
/// The review itself goes in the existing `jobs.review` column.
const SCHEMA_V6: &str = "
ALTER TABLE jobs ADD COLUMN wants_review INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN review_of INTEGER REFERENCES jobs(id);
";

/// Telemetry that outlives a run: the latest usage-limit reading per window
/// and the latest environment per Claude session.
const SCHEMA_V7: &str = "
CREATE TABLE rate_limits (
    window TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    utilization REAL,
    resets_at INTEGER,
    surpassed_threshold REAL,
    observed_at INTEGER NOT NULL
);
CREATE TABLE session_environments (
    session_id TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    seen_at INTEGER NOT NULL
);
";

/// The hash-chained ledger. Rows are only ever inserted; the triggers
/// refuse changing or removing them.
const SCHEMA_V8: &str = "
CREATE TABLE ledger (
    seq INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    job_id INTEGER,
    session TEXT,
    payload_json TEXT NOT NULL,
    prev_hash BLOB NOT NULL,
    hash BLOB NOT NULL
);
CREATE TRIGGER ledger_no_update BEFORE UPDATE ON ledger
BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
CREATE TRIGGER ledger_no_delete BEFORE DELETE ON ledger
BEGIN SELECT RAISE(ABORT, 'the ledger is append-only'); END;
";

/// Migrations in order; index + 1 is the resulting `user_version`.
const MIGRATIONS: &[&str] = &[
    SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
];

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
pub(crate) mod tests {
    use super::*;
    use swarmux_application::ports::{JobRepository, NewJob, SessionRepository};
    use swarmux_domain::{JobId, JobResult, JobStatus, Session, SessionId};

    pub(crate) fn new_job(session: SessionId, prompt: &str) -> NewJob {
        NewJob {
            session_id: session,
            prompt_id: None,
            prompt: prompt.into(),
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        }
    }

    fn session(name: &str, now: i64) -> Session {
        Session::new(SessionId::random(), name, "/w".into(), "sonnet", now).unwrap()
    }

    fn store_with_job() -> (SqliteStore, Session, JobId) {
        let s = SqliteStore::open_in_memory().unwrap();
        let sess = session("a", 1);
        SessionRepository::insert(&s, &sess).unwrap();
        let job = JobRepository::insert(&s, &new_job(sess.id, "hi"), 2).unwrap();
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
    fn migrate_sets_latest_schema_version() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn version_1_database_is_upgraded_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions VALUES ('00000000-0000-4000-8000-000000000001', 's', '/w', 'sonnet', 'default', 1, NULL, 0);
             INSERT INTO jobs (id, body_rendered, session_id, model, status, created_at)
             VALUES (1, 'old', '00000000-0000-4000-8000-000000000001', 'sonnet', 'done', 2);",
        )
        .unwrap();
        drop(conn);
        let s = SqliteStore::open(&path).unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let job = JobRepository::get(&s, JobId(1)).unwrap().unwrap();
        assert_eq!((job.prompt.as_str(), job.fork), ("old", false));
    }

    #[test]
    fn open_twice_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(SqliteStore::open(&path).unwrap());
        assert_eq!(
            SqliteStore::open(&path).unwrap().schema_version().unwrap(),
            MIGRATIONS.len() as i64
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
        let got = SessionRepository::get(&s, a.id).unwrap().unwrap();
        assert_eq!(got.last_used_at, Some(30));
        assert_eq!(got.cwd, a.cwd);
        assert!(!got.lean);
        s.set_lean(a.id, true).unwrap();
        assert!(SessionRepository::get(&s, a.id).unwrap().unwrap().lean);
        assert_eq!(
            SessionRepository::get(&s, SessionId::random()).unwrap(),
            None
        );
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
        let payload = s.events(job).unwrap().remove(0).payload;
        assert!(payload.ends_with("…[truncated]"));
        assert!(payload.len() <= PAYLOAD_CAP + "…[truncated]".len());
    }

    #[test]
    fn event_payload_cap_respects_utf8() {
        let (s, _, job) = store_with_job();
        s.append_event(job, "text", "lead", &"é".repeat(5000), 3)
            .unwrap();
        let payload = s.events(job).unwrap().remove(0).payload;
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
            text: "answer é".into(),
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
