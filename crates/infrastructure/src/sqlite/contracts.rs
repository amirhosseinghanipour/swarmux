//! What each file of a workspace yielded, for incremental contract scans.

use rusqlite::params;
use swarmux_application::{AppResult, ContractStore, FileRecord};
use swarmux_domain::WorkspaceId;
use swarmux_domain::contracts::{decode, encode};

use super::{OrStorage, SqliteStore};

impl ContractStore for SqliteStore {
    fn records(&self, workspace: WorkspaceId) -> AppResult<Vec<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT repo, path, size, mtime, hash, facts FROM contract_files
                 WHERE workspace_id = ?1 ORDER BY repo, path",
            )
            .or_storage()?;
        let rows = stmt
            .query_map(params![workspace.to_string()], |r| {
                Ok(FileRecord {
                    repo: r.get(0)?,
                    path: r.get(1)?,
                    size: r.get::<_, i64>(2)? as u64,
                    mtime: r.get(3)?,
                    hash: r.get(4)?,
                    facts: decode(&r.get::<_, String>(5)?),
                })
            })
            .or_storage()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()
    }

    fn apply(
        &self,
        workspace: WorkspaceId,
        upserts: &[FileRecord],
        removed: &[(String, String)],
    ) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        let id = workspace.to_string();
        for (repo, path) in removed {
            tx.execute(
                "DELETE FROM contract_files WHERE workspace_id = ?1 AND repo = ?2 AND path = ?3",
                params![id, repo, path],
            )
            .or_storage()?;
        }
        for r in upserts {
            tx.execute(
                "INSERT INTO contract_files (workspace_id, repo, path, size, mtime, hash, facts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(workspace_id, repo, path) DO UPDATE SET
                    size = excluded.size, mtime = excluded.mtime,
                    hash = excluded.hash, facts = excluded.facts",
                params![
                    id,
                    r.repo,
                    r.path,
                    r.size as i64,
                    r.mtime,
                    r.hash,
                    encode(&r.facts)
                ],
            )
            .or_storage()?;
        }
        tx.commit().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use swarmux_application::WorkspaceStore;
    use swarmux_domain::contracts::{Fact, Symbol};
    use swarmux_domain::{Workspace, WorkspaceId};

    use super::*;

    fn record(path: &str, hash: &str) -> FileRecord {
        FileRecord {
            repo: "backend".into(),
            path: path.into(),
            size: 10,
            mtime: 5,
            hash: hash.into(),
            facts: vec![Fact::provide(Symbol::table("orders"), 3, "")],
        }
    }

    fn workspace(store: &SqliteStore) -> WorkspaceId {
        let w = Workspace::new(WorkspaceId::random(), "shop", PathBuf::from("/shop"), 1).unwrap();
        store.save(&w).unwrap();
        w.id
    }

    #[test]
    fn records_are_kept_replaced_removed_and_follow_their_workspace() {
        let s = SqliteStore::open_in_memory().unwrap();
        let w = workspace(&s);
        assert!(s.records(w).unwrap().is_empty());
        s.apply(w, &[record("a.sql", "h1"), record("b.sql", "h2")], &[])
            .unwrap();
        s.apply(
            w,
            &[record("a.sql", "h3")],
            &[("backend".to_string(), "b.sql".to_string())],
        )
        .unwrap();
        let got = s.records(w).unwrap();
        assert_eq!(got, vec![record("a.sql", "h3")]);
        s.remove(w).unwrap();
        assert!(s.records(w).unwrap().is_empty(), "gone with the workspace");
    }
}
