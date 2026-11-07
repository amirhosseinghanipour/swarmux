//! The SQLite ledger against tampering, plus its append latency.

use std::path::Path;
use std::time::Instant;

use rusqlite::Connection;
use swarmux_application::ledger::{Ledger, verify_all};
use swarmux_domain::JobId;
use swarmux_domain::ledger::{BreakReason, ChainBreak, LedgerDraft, LedgerKind};
use swarmux_infrastructure::{SqliteLedger, SqliteStore};

fn draft(i: i64) -> LedgerDraft {
    LedgerDraft {
        ts: 1_700_000_000_000 + i,
        kind: LedgerKind::AgentEvent,
        actor: "lead".into(),
        job: Some(JobId(1)),
        session: Some("00000000-0000-4000-8000-000000000001".into()),
        payload_json: format!(r#"{{"event":"text","summary":"step {i}"}}"#),
    }
}

/// A ledger file with genesis plus `n` entries.
fn filled(path: &Path, n: i64) -> SqliteLedger {
    let l = SqliteLedger::open(path).unwrap();
    for i in 0..n {
        l.append(draft(i)).unwrap();
    }
    l
}

/// A raw connection with the append-only triggers removed, as someone
/// editing the file by hand would do.
fn raw(path: &Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("DROP TRIGGER ledger_no_update; DROP TRIGGER ledger_no_delete;")
        .unwrap();
    c
}

#[test]
fn fresh_ledger_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let l = filled(&dir.path().join("l.db"), 50);
    let r = verify_all(&l).unwrap();
    assert!(r.is_ok());
    assert_eq!(r.entries, 51);
    assert_eq!(r.head.map(|(seq, _)| seq), Some(51));
}

#[test]
fn schema_refuses_edits_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.db");
    drop(filled(&path, 3));
    let c = Connection::open(&path).unwrap();
    let edit = c.execute("UPDATE ledger SET payload_json = '{}' WHERE seq = 2", []);
    assert!(edit.unwrap_err().to_string().contains("append-only"));
    let delete = c.execute("DELETE FROM ledger WHERE seq = 2", []);
    assert!(delete.unwrap_err().to_string().contains("append-only"));
}

#[test]
fn edited_payload_is_reported_at_its_seq() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.db");
    let l = filled(&path, 20);
    raw(&path)
        .execute(
            r#"UPDATE ledger SET payload_json = '{"event":"text","summary":"forged"}' WHERE seq = 7"#,
            [],
        )
        .unwrap();
    let r = verify_all(&l).unwrap();
    assert_eq!(
        r.broken,
        Some(ChainBreak {
            seq: 7,
            reason: BreakReason::HashMismatch
        })
    );
    assert_eq!(r.entries, 6);
}

#[test]
fn edit_with_a_recomputed_hash_breaks_the_next_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.db");
    let l = filled(&path, 20);
    let mut e = l.range(7, 1).unwrap().remove(0);
    e.draft.actor = "guard".into();
    let rehashed = swarmux_domain::ledger::chain_hash(&e.prev_hash, 7, &e.draft);
    raw(&path)
        .execute(
            "UPDATE ledger SET actor = 'guard', hash = ?1 WHERE seq = 7",
            [&rehashed[..]],
        )
        .unwrap();
    assert_eq!(
        verify_all(&l).unwrap().broken,
        Some(ChainBreak {
            seq: 8,
            reason: BreakReason::PrevMismatch
        })
    );
}

#[test]
fn deleted_row_is_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.db");
    let l = filled(&path, 20);
    raw(&path)
        .execute("DELETE FROM ledger WHERE seq = 10", [])
        .unwrap();
    assert_eq!(
        verify_all(&l).unwrap().broken,
        Some(ChainBreak {
            seq: 11,
            reason: BreakReason::Gap { expected: 10 }
        })
    );
}

#[test]
fn damaged_hash_column_is_a_break_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.db");
    let l = filled(&path, 5);
    raw(&path)
        .execute("UPDATE ledger SET hash = x'00' WHERE seq = 3", [])
        .unwrap();
    assert_eq!(verify_all(&l).unwrap().broken.map(|b| b.seq), Some(3));
}

#[test]
fn existing_database_starts_its_ledger_at_genesis() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    // A database as the previous version left it: everything but the ledger.
    drop(SqliteStore::open(&path).unwrap());
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "DROP TRIGGER ledger_no_update; DROP TRIGGER ledger_no_delete;
         DROP TABLE ledger;
         DROP TABLE evidence;
         DROP TABLE pending_resumes;
         DROP TABLE missions;
         DROP TABLE checkpoints; DROP TABLE worktrees;
         DROP TABLE workspace_repos; DROP TABLE workspaces;
         ALTER TABLE sessions DROP COLUMN workspace_id;
         DROP TABLE ledger_anchors;
         DROP TRIGGER knowledge_events_no_update; DROP TRIGGER knowledge_events_no_delete;
         DROP TABLE knowledge_events; DROP TABLE knowledge_items;
         ALTER TABLE job_results DROP COLUMN cache_creation;
         ALTER TABLE sessions DROP COLUMN worktree;
         PRAGMA user_version = 7;",
    )
    .unwrap();
    drop(c);

    let l = SqliteLedger::open(&path).unwrap();
    assert_eq!(l.head().unwrap(), None);
    assert!(verify_all(&l).unwrap().is_ok());
    l.append(draft(0)).unwrap();
    let r = verify_all(&l).unwrap();
    assert_eq!((r.entries, r.broken), (2, None));
    assert_eq!(l.range(1, 1).unwrap()[0].draft.kind, LedgerKind::Genesis);
}

/// The adapter only reads and inserts; any statement that changes or
/// removes rows would undermine the chain.
#[test]
fn adapter_never_changes_or_removes_rows() {
    let src = include_str!("../src/sqlite/ledger.rs").to_ascii_uppercase();
    for word in ["UPDATE ", "DELETE ", "REPLACE", "UPSERT", "DROP ", "ALTER "] {
        assert!(!src.contains(word), "the ledger adapter mentions {word:?}");
    }
}

/// The agent runner gets no handle on the ledger.
#[test]
fn claude_adapter_has_no_ledger_access() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/claude_cli");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap().to_ascii_lowercase();
        assert!(
            !src.contains("ledger"),
            "{} mentions the ledger",
            path.display()
        );
    }
}

/// Appends 5k entries to a file database and prints the latency spread.
/// Run with `cargo test -p swarmux-infrastructure --release --test ledger -- --ignored --nocapture`.
#[test]
#[ignore = "measures this machine"]
fn append_p95_under_2ms() {
    const N: usize = 5_000;
    let dir = tempfile::tempdir().unwrap();
    let l = SqliteLedger::open(&dir.path().join("perf.db")).unwrap();
    let mut times: Vec<f64> = Vec::with_capacity(N);
    for i in 0..N {
        let mut d = draft(i as i64);
        d.payload_json = format!(r#"{{"event":"text","summary":"{}"}}"#, "x".repeat(400));
        let t = Instant::now();
        l.append(d).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    let pct = |p: f64| times[((N as f64 * p) as usize).min(N - 1)];
    println!(
        "ledger append over {N}: p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, max {:.3} ms",
        pct(0.50),
        pct(0.95),
        pct(0.99),
        times[N - 1]
    );
    assert!(verify_all(&l).unwrap().is_ok());
    assert!(pct(0.95) < 2.0, "p95 {:.3} ms", pct(0.95));
}
