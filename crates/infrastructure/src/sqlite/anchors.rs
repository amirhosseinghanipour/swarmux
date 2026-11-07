//! Signed checkpoints of the ledger's head, kept in the database. The
//! table refuses changes and removals; the signatures, not the table, are
//! what make a checkpoint worth anything.

use rusqlite::params;
use swarmux_application::AppResult;
use swarmux_application::anchor::AnchorStore;
use swarmux_domain::anchor::Checkpoint;
use swarmux_domain::ledger::Hash;

use super::{OrStorage, SqliteStore};

impl AnchorStore for SqliteStore {
    fn save(&self, c: &Checkpoint) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO ledger_anchors (seq, head, at, key, signature)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    i64::try_from(c.seq).unwrap_or(i64::MAX),
                    &c.head[..],
                    c.at,
                    c.key,
                    c.signature
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<Checkpoint>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT seq, head, at, key, signature FROM ledger_anchors ORDER BY seq, at",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                let head: Vec<u8> = r.get(1)?;
                Ok(Checkpoint {
                    seq: r.get::<_, i64>(0)?.max(0) as u64,
                    head: Hash::try_from(head.as_slice()).unwrap_or_default(),
                    at: r.get(2)?,
                    key: r.get(3)?,
                    signature: r.get(4)?,
                })
            })
            .or_storage()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoints_are_kept_in_order_and_cannot_be_changed() {
        let store = SqliteStore::open_in_memory().unwrap();
        let cp = |seq: u64, at: i64| Checkpoint {
            seq,
            head: [seq as u8; 32],
            at,
            key: "RWQk".into(),
            signature: "sig".into(),
        };
        store.save(&cp(9, 20)).unwrap();
        store.save(&cp(3, 10)).unwrap();
        store.save(&cp(3, 10)).unwrap();
        let all = store.list().unwrap();
        assert_eq!(all, vec![cp(3, 10), cp(9, 20)]);
        assert!(
            store
                .conn
                .execute("UPDATE ledger_anchors SET seq = 1", [])
                .is_err()
        );
        assert!(
            store
                .conn
                .execute("DELETE FROM ledger_anchors", [])
                .is_err()
        );
    }
}
