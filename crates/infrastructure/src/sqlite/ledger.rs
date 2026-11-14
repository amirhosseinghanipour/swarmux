//! The ledger table: append-only, hash-chained.
//!
//! This adapter only ever reads and inserts rows; the schema also refuses
//! changes and removals of stored rows.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use swarmux_application::ledger::Ledger;
use swarmux_application::{AppError, AppResult};
use swarmux_domain::JobId;
use swarmux_domain::ledger::{Hash, LedgerDraft, LedgerEntry, LedgerKind};

use super::{OrStorage, SqliteStore};

/// How long an append waits for another writer (another swarmux process).
const BUSY_WAIT: Duration = Duration::from_secs(5);

const COLUMNS: &str = "seq, ts, kind, actor, job_id, session, payload_json, prev_hash, hash";

/// The ledger in swarmux's database, on its own connection.
pub struct SqliteLedger {
    conn: Connection,
}

impl SqliteLedger {
    /// Opens (and migrates) the database at `path`.
    pub fn open(path: &Path) -> AppResult<SqliteLedger> {
        SqliteLedger::from_store(SqliteStore::open(path)?)
    }

    pub fn open_in_memory() -> AppResult<SqliteLedger> {
        SqliteLedger::from_store(SqliteStore::open_in_memory()?)
    }

    fn from_store(store: SqliteStore) -> AppResult<SqliteLedger> {
        let conn = store.conn;
        conn.busy_timeout(BUSY_WAIT).or_storage()?;
        // With WAL, NORMAL skips the flush on every commit: a crash of
        // swarmux loses nothing, a power cut may lose the newest entries but
        // never breaks the chain. A flush per append costs milliseconds.
        conn.pragma_update(None, "synchronous", "NORMAL")
            .or_storage()?;
        Ok(SqliteLedger { conn })
    }

    fn head_in(conn: &Connection) -> rusqlite::Result<Option<LedgerEntry>> {
        conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM ledger ORDER BY seq DESC LIMIT 1"
        ))?
        .query_row([], from_row)
        .optional()
    }

    fn insert(tx: &Transaction, e: &LedgerEntry) -> rusqlite::Result<()> {
        let d = &e.draft;
        tx.prepare_cached(&format!(
            "INSERT INTO ledger ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        ))?
        .execute(params![
            i64::try_from(e.seq).unwrap_or(i64::MAX),
            d.ts,
            d.kind.as_str(),
            d.actor,
            d.job.map(|j| j.0),
            d.session,
            d.payload_json,
            &e.prev_hash[..],
            &e.hash[..],
        ])?;
        Ok(())
    }
}

/// A stored hash; one of the wrong length reads as zeros, which never
/// verifies.
fn hash_of(r: &Row, i: usize) -> rusqlite::Result<Hash> {
    let bytes: Vec<u8> = r.get(i)?;
    Ok(Hash::try_from(bytes.as_slice()).unwrap_or_default())
}

fn from_row(r: &Row) -> rusqlite::Result<LedgerEntry> {
    let kind: String = r.get(2)?;
    let kind = LedgerKind::parse(&kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("unknown ledger kind {kind:?}").into(),
        )
    })?;
    Ok(LedgerEntry {
        seq: r.get::<_, i64>(0)?.max(0) as u64,
        draft: LedgerDraft {
            ts: r.get(1)?,
            kind,
            actor: r.get(3)?,
            job: r.get::<_, Option<i64>>(4)?.map(JobId),
            session: r.get(5)?,
            payload_json: r.get(6)?,
        },
        prev_hash: hash_of(r, 7)?,
        hash: hash_of(r, 8)?,
    })
}

impl SqliteLedger {
    /// The newest entry's `seq`, 0 when there is none: a cheap way to see
    /// whether anything was appended since.
    pub fn head_seq(&self) -> AppResult<u64> {
        self.conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM ledger", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n.max(0) as u64)
            .or_storage()
    }

    /// The entries of one `kind` with `seq > after`, oldest first.
    pub fn of_kind_after(&self, kind: LedgerKind, after: u64) -> AppResult<Vec<LedgerEntry>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM ledger WHERE kind = ?1 AND seq > ?2 ORDER BY seq"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map(
                params![kind.as_str(), i64::try_from(after).unwrap_or(i64::MAX)],
                from_row,
            )
            .or_storage()?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| AppError::Storage(format!("ledger: {e}")))
    }
}

impl Ledger for SqliteLedger {
    fn append(&self, draft: LedgerDraft) -> AppResult<LedgerEntry> {
        // IMMEDIATE takes the write lock before reading the head, so two
        // writers can never chain onto the same entry.
        let tx =
            Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate).or_storage()?;
        let mut head = SqliteLedger::head_in(&tx).or_storage()?;
        if head.is_none() {
            let genesis = LedgerEntry::next(
                None,
                LedgerDraft::genesis(draft.ts, env!("CARGO_PKG_VERSION")),
            );
            SqliteLedger::insert(&tx, &genesis).or_storage()?;
            head = Some(genesis);
        }
        let entry = LedgerEntry::next(head.as_ref(), draft);
        SqliteLedger::insert(&tx, &entry).or_storage()?;
        tx.commit().or_storage()?;
        Ok(entry)
    }

    fn head(&self) -> AppResult<Option<LedgerEntry>> {
        SqliteLedger::head_in(&self.conn).or_storage()
    }

    fn range(&self, from_seq: u64, limit: usize) -> AppResult<Vec<LedgerEntry>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM ledger WHERE seq >= ?1 ORDER BY seq LIMIT ?2"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map(
                params![
                    i64::try_from(from_seq).unwrap_or(i64::MAX),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                from_row,
            )
            .or_storage()?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| AppError::Storage(format!("ledger: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ledger::{tail, verify_all};
    use swarmux_domain::ledger::{GENESIS_PREV, verify};

    fn draft(ts: i64, payload: &str) -> LedgerDraft {
        LedgerDraft {
            ts,
            kind: LedgerKind::AgentEvent,
            actor: "lead".into(),
            job: Some(JobId(4)),
            session: Some("s".into()),
            payload_json: payload.into(),
        }
    }

    #[test]
    fn first_append_creates_genesis() {
        let l = SqliteLedger::open_in_memory().unwrap();
        assert_eq!(l.head().unwrap(), None);
        let e = l.append(draft(10, "{}")).unwrap();
        assert_eq!(e.seq, 2);
        let all = l.range(1, 10).unwrap();
        assert_eq!(all.len(), 2);
        let g = &all[0];
        assert_eq!(
            (g.seq, g.draft.kind, g.prev_hash, g.draft.ts),
            (1, LedgerKind::Genesis, GENESIS_PREV, 10)
        );
        assert_eq!(
            g.draft.payload_json,
            format!(r#"{{"version":"{}"}}"#, env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(all[1], e);
        assert_eq!(l.head().unwrap(), Some(e));
    }

    #[test]
    fn entries_round_trip_and_verify() {
        let l = SqliteLedger::open_in_memory().unwrap();
        for i in 0..30 {
            l.append(LedgerDraft {
                job: None,
                session: None,
                ..draft(i, &format!(r#"{{"n":{i},"t":"é🐝"}}"#))
            })
            .unwrap();
        }
        let all = l.range(1, 100).unwrap();
        assert_eq!(all.len(), 31);
        assert_eq!(verify(&all), Ok(()));
        assert_eq!(l.range(30, 100).unwrap().len(), 2);
        assert_eq!(
            l.range(5, 3)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            [5, 6, 7]
        );
        assert_eq!(tail(&l, 2).unwrap()[1].seq, 31);
        let report = verify_all(&l).unwrap();
        assert!(report.is_ok());
        assert_eq!(report.entries, 31);
    }

    #[test]
    fn two_connections_share_one_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("l.db");
        let a = SqliteLedger::open(&path).unwrap();
        let b = SqliteLedger::open(&path).unwrap();
        for i in 0..10 {
            a.append(draft(i, "{}")).unwrap();
            b.append(draft(i, "{}")).unwrap();
        }
        assert_eq!(a.head().unwrap().unwrap().seq, 21);
        assert!(verify_all(&b).unwrap().is_ok());
    }
}
