use std::path::PathBuf;

use rusqlite::{OptionalExtension, Row, params};
use swarmux_application::AppResult;
use swarmux_application::WorktreeStore;
use swarmux_domain::{
    Checkpoint, CheckpointReason, Handoff, JobId, Millis, SetMember, WorktreeRecord,
};

use super::sessions::session_id_at;
use super::{OrStorage, SqliteStore};

const WORKTREE_COLS: &str = "job_id, session_id, repo, path, branch, slot, port_base, \
                             port_count, created_at, removed_at, seq, repo_name, task_dir";
const CHECKPOINT_COLS: &str =
    "id, job_id, session_id, commit_sha, worktree, branch, reason, ledger_seq, handoff, at";

fn worktree_from_row(r: &Row) -> rusqlite::Result<WorktreeRecord> {
    let repo: String = r.get(2)?;
    let path: String = r.get(3)?;
    Ok(WorktreeRecord {
        job: JobId(r.get(0)?),
        session: session_id_at(r, 1)?,
        repo: PathBuf::from(repo),
        path: PathBuf::from(path),
        branch: r.get(4)?,
        slot: r.get(5)?,
        port_base: r.get(6)?,
        port_count: r.get(7)?,
        created_at: r.get(8)?,
        removed_at: r.get(9)?,
        member: match (
            r.get::<_, Option<String>>(11)?,
            r.get::<_, Option<String>>(12)?,
        ) {
            (Some(repo), Some(task_dir)) => Some(SetMember {
                repo,
                task_dir: PathBuf::from(task_dir),
                seq: r.get(10)?,
            }),
            _ => None,
        },
    })
}

fn handoff_to_text(h: &Handoff) -> String {
    h.to_json()
}

fn strings(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

fn handoff_from_text(text_json: &str) -> Option<Handoff> {
    let v: serde_json::Value = serde_json::from_str(text_json).ok()?;
    Some(Handoff {
        goal: text(&v, "goal"),
        state: text(&v, "state"),
        decisions: strings(&v, "decisions"),
        tried_and_failed: strings(&v, "tried_and_failed"),
        next_step: text(&v, "next_step"),
        open_questions: strings(&v, "open_questions"),
    })
}

fn checkpoint_from_row(r: &Row) -> rusqlite::Result<Checkpoint> {
    let worktree: String = r.get(4)?;
    let reason: String = r.get(6)?;
    let handoff: Option<String> = r.get(8)?;
    let seq: Option<i64> = r.get(7)?;
    Ok(Checkpoint {
        id: r.get(0)?,
        job: JobId(r.get(1)?),
        session_id: r.get(2)?,
        commit: r.get(3)?,
        worktree: PathBuf::from(worktree),
        branch: r.get(5)?,
        reason: CheckpointReason::parse(&reason).unwrap_or(CheckpointReason::Manual),
        ledger_seq: seq.and_then(|s| u64::try_from(s).ok()),
        handoff: handoff.as_deref().and_then(handoff_from_text),
        at: r.get(9)?,
    })
}

impl WorktreeStore for SqliteStore {
    fn insert(&self, w: &WorktreeRecord) -> AppResult<()> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO worktrees ({WORKTREE_COLS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
                ),
                params![
                    w.job.0,
                    w.session.to_string(),
                    w.repo.to_string_lossy(),
                    w.path.to_string_lossy(),
                    w.branch,
                    w.slot,
                    w.port_base,
                    w.port_count,
                    w.created_at,
                    w.removed_at,
                    w.member.as_ref().map_or(0, |m| m.seq),
                    w.member.as_ref().map(|m| m.repo.clone()),
                    w.member
                        .as_ref()
                        .map(|m| m.task_dir.to_string_lossy().into_owned()),
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn get(&self, job: JobId) -> AppResult<Option<WorktreeRecord>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {WORKTREE_COLS} FROM worktrees WHERE job_id = ?1 ORDER BY seq LIMIT 1"
                ),
                [job.0],
                worktree_from_row,
            )
            .optional()
            .or_storage()
    }

    fn set(&self, job: JobId) -> AppResult<Vec<WorktreeRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {WORKTREE_COLS} FROM worktrees WHERE job_id = ?1 ORDER BY seq"
            ))
            .or_storage()?;
        let rows = stmt.query_map([job.0], worktree_from_row).or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn active(&self) -> AppResult<Vec<WorktreeRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {WORKTREE_COLS} FROM worktrees WHERE removed_at IS NULL
                 ORDER BY created_at, job_id, seq"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], worktree_from_row).or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn rebind(&self, from: JobId, to: JobId) -> AppResult<bool> {
        let moved = self
            .conn
            .execute(
                "UPDATE worktrees SET job_id = ?2 WHERE job_id = ?1 AND removed_at IS NULL",
                params![from.0, to.0],
            )
            .or_storage()?;
        if moved > 0 {
            self.conn
                .execute(
                    "UPDATE checkpoints SET job_id = ?2 WHERE job_id = ?1",
                    params![from.0, to.0],
                )
                .or_storage()?;
        }
        Ok(moved > 0)
    }

    fn mark_removed(&self, job: JobId, now: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE worktrees SET removed_at = ?2 WHERE job_id = ?1 AND removed_at IS NULL",
                params![job.0, now],
            )
            .or_storage()?;
        Ok(())
    }

    fn insert_checkpoint(&self, c: &Checkpoint) -> AppResult<i64> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO checkpoints ({}) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    CHECKPOINT_COLS
                ),
                params![
                    c.job.0,
                    c.session_id,
                    c.commit,
                    c.worktree.to_string_lossy(),
                    c.branch,
                    c.reason.as_str(),
                    c.ledger_seq.map(|s| i64::try_from(s).unwrap_or(i64::MAX)),
                    c.handoff.as_ref().map(handoff_to_text),
                    c.at
                ],
            )
            .or_storage()?;
        Ok(self.conn.last_insert_rowid())
    }

    fn checkpoints(&self, job: JobId) -> AppResult<Vec<Checkpoint>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {CHECKPOINT_COLS} FROM checkpoints WHERE job_id = ?1 ORDER BY id"
            ))
            .or_storage()?;
        let rows = stmt.query_map([job.0], checkpoint_from_row).or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn recent_checkpoints(&self, limit: usize) -> AppResult<Vec<Checkpoint>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {CHECKPOINT_COLS} FROM checkpoints ORDER BY id DESC LIMIT ?1"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map(
                [i64::try_from(limit).unwrap_or(i64::MAX)],
                checkpoint_from_row,
            )
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::SessionId;

    fn record(job: i64, slot: u32) -> WorktreeRecord {
        WorktreeRecord {
            job: JobId(job),
            session: SessionId::random(),
            repo: "/repo".into(),
            path: format!("/wt/{job}").into(),
            branch: format!("swarmux/demo/{job}"),
            slot,
            port_base: 20_000 + slot as u16 * 20,
            port_count: 20,
            created_at: 5,
            removed_at: None,
            member: None,
        }
    }

    fn member(job: i64, seq: u32, repo: &str) -> WorktreeRecord {
        WorktreeRecord {
            path: format!("/t/{job}/{repo}").into(),
            repo: format!("/p/{repo}").into(),
            member: Some(SetMember {
                repo: repo.into(),
                task_dir: format!("/t/{job}").into(),
                seq,
            }),
            ..record(job, 4)
        }
    }

    #[test]
    fn a_set_shares_its_slot_and_moves_and_goes_together() {
        let s = SqliteStore::open_in_memory().unwrap();
        let a = member(1, 0, "backend");
        let b = member(1, 1, "app");
        s.insert(&b).unwrap();
        s.insert(&a).unwrap();
        assert_eq!(
            s.get(JobId(1)).unwrap(),
            Some(a.clone()),
            "the first of the set"
        );
        assert_eq!(s.set(JobId(1)).unwrap(), [a.clone(), b.clone()]);
        assert!(
            s.insert(&record(2, 4)).is_err(),
            "another job can't take the slot"
        );
        assert!(s.rebind(JobId(1), JobId(3)).unwrap());
        assert_eq!(s.set(JobId(3)).unwrap().len(), 2);
        s.mark_removed(JobId(3), 9).unwrap();
        assert!(s.active().unwrap().is_empty());
        assert!(
            s.set(JobId(3))
                .unwrap()
                .iter()
                .all(|r| r.removed_at == Some(9))
        );
    }

    #[test]
    fn worktrees_of_an_older_schema_become_sets_of_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in &super::super::MIGRATIONS[..17] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 17).unwrap();
        conn.execute_batch(
            "INSERT INTO worktrees VALUES (7, '00000000-0000-4000-8000-000000000001', '/repo',
             '/wt/7', 'swarmux/s/7', 2, 20040, 20, 5, NULL);",
        )
        .unwrap();
        drop(conn);
        let s = SqliteStore::open(&path).unwrap();
        let r = s.get(JobId(7)).unwrap().unwrap();
        assert_eq!((r.slot, r.member), (2, None));
        assert_eq!(s.set(JobId(7)).unwrap().len(), 1);
    }

    fn checkpoint(job: i64, n: u64) -> Checkpoint {
        Checkpoint {
            id: 0,
            job: JobId(job),
            session_id: "claude".into(),
            commit: "a".repeat(40),
            worktree: format!("/wt/{job}").into(),
            branch: "b".into(),
            reason: CheckpointReason::PreCompact,
            ledger_seq: Some(n),
            handoff: Some(Handoff {
                goal: "g \"quoted\"".into(),
                state: "s".into(),
                decisions: vec!["d".into()],
                tried_and_failed: vec!["t1".into(), "t2".into()],
                next_step: "n".into(),
                open_questions: vec![],
            }),
            at: 9,
        }
    }

    #[test]
    fn worktrees_round_trip_and_a_live_slot_is_unique() {
        let s = SqliteStore::open_in_memory().unwrap();
        let a = record(1, 0);
        s.insert(&a).unwrap();
        assert_eq!(s.get(JobId(1)).unwrap(), Some(a.clone()));
        assert!(s.insert(&record(2, 0)).is_err(), "slot 0 is taken");
        s.insert(&record(2, 1)).unwrap();
        assert_eq!(s.active().unwrap().len(), 2);

        s.mark_removed(JobId(1), 77).unwrap();
        assert_eq!(s.active().unwrap().len(), 1);
        assert_eq!(s.get(JobId(1)).unwrap().unwrap().removed_at, Some(77));
        // The reclaimed slot can be given out again.
        s.insert(&record(3, 0)).unwrap();
    }

    #[test]
    fn checkpoints_keep_their_handoff_and_move_with_a_rebind() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.insert(&record(1, 0)).unwrap();
        let first = s.insert_checkpoint(&checkpoint(1, 4)).unwrap();
        let second = s.insert_checkpoint(&checkpoint(1, 8)).unwrap();
        assert!(second > first);
        let all = s.checkpoints(JobId(1)).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].ledger_seq, Some(4));
        assert_eq!(all[0].handoff, checkpoint(1, 4).handoff);
        assert_eq!(all[0].reason, CheckpointReason::PreCompact);

        assert!(s.rebind(JobId(1), JobId(5)).unwrap());
        assert!(s.get(JobId(1)).unwrap().is_none());
        assert_eq!(s.get(JobId(5)).unwrap().unwrap().slot, 0);
        assert_eq!(s.checkpoints(JobId(5)).unwrap().len(), 2);
        assert!(s.checkpoints(JobId(1)).unwrap().is_empty());
        assert!(!s.rebind(JobId(1), JobId(6)).unwrap());
        assert_eq!(s.recent_checkpoints(1).unwrap()[0].id, second);
    }
}
