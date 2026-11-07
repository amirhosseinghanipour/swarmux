//! Knowledge events (append-only) and the projection folded from them.

use rusqlite::params;
use swarmux_application::{AppError, AppResult, KnowledgeStore};
use swarmux_domain::knowledge::{Event, Item, ItemId};

use super::{OrStorage, SqliteStore};
use crate::knowledge::{change_from_json, change_json, item_from_json, item_json};

fn bad(why: String) -> AppError {
    AppError::Storage(format!("knowledge: {why}"))
}

impl KnowledgeStore for SqliteStore {
    fn append(&self, event: &Event, item: &Item) -> AppResult<u64> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        tx.execute(
            "INSERT INTO knowledge_events (at, item, actor, change, ledger_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.at,
                event.item.as_str(),
                event.actor,
                change_json(&event.change),
                event.ledger.map(|s| s as i64),
            ],
        )
        .or_storage()?;
        let seq = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO knowledge_items (id, home, kind, status, item) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET home = ?2, kind = ?3, status = ?4, item = ?5",
            params![
                item.id.as_str(),
                item.home.key(),
                item.kind.as_str(),
                item.status.as_str(),
                item_json(item),
            ],
        )
        .or_storage()?;
        tx.commit().or_storage()?;
        Ok(seq as u64)
    }

    fn events(&self) -> AppResult<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, at, item, actor, change, ledger_seq FROM knowledge_events ORDER BY seq",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            })
            .or_storage()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .or_storage()?;
        rows.into_iter()
            .map(|(seq, at, item, actor, change, ledger)| {
                Ok(Event {
                    seq: seq as u64,
                    at,
                    item: ItemId::parse(&item).ok_or_else(|| bad(format!("bad id {item:?}")))?,
                    actor,
                    change: change_from_json(&change)
                        .map_err(|e| bad(format!("event {seq}: {e}")))?,
                    ledger: ledger.map(|s| s as u64),
                })
            })
            .collect()
    }

    fn last_seq(&self) -> AppResult<u64> {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM knowledge_events",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .or_storage()
    }

    fn items(&self) -> AppResult<Vec<Item>> {
        let mut stmt = self
            .conn
            .prepare("SELECT item FROM knowledge_items ORDER BY id")
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .or_storage()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .or_storage()?;
        rows.iter()
            .map(|text| item_from_json(text).map_err(bad))
            .collect()
    }

    fn replace_items(&self, items: &[Item]) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        tx.execute("DELETE FROM knowledge_items", []).or_storage()?;
        for item in items {
            tx.execute(
                "INSERT INTO knowledge_items (id, home, kind, status, item) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    item.id.as_str(),
                    item.home.key(),
                    item.kind.as_str(),
                    item.status.as_str(),
                    item_json(item),
                ],
            )
            .or_storage()?;
        }
        tx.commit().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use swarmux_application::KnowledgeService;
    use swarmux_domain::knowledge::{Draft, Home, Kind, Status};

    use super::*;
    use crate::SystemClock;

    #[test]
    fn events_survive_a_reopen_and_rebuild_the_projection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.db");
        let store = Rc::new(SqliteStore::open(&path).unwrap());
        let service = KnowledgeService::new(store.clone(), Rc::new(SystemClock)).unwrap();
        let home = Home::repo("/w");
        let item = service
            .add(Draft::new(home.clone(), Kind::Fact, "a fact"))
            .unwrap();
        service.retire(&item.id).unwrap();
        drop(service);
        drop(store);

        let store = Rc::new(SqliteStore::open(&path).unwrap());
        assert_eq!(store.events().unwrap().len(), 2);
        assert_eq!(store.last_seq().unwrap(), 2);
        let stored = store.items().unwrap();
        assert_eq!(stored[0].status, Status::Retired);
        let service = KnowledgeService::new(store.clone(), Rc::new(SystemClock)).unwrap();
        assert_eq!(service.list(), stored);
        assert!(service.rebuild().unwrap().differed.is_empty());
        let raw = rusqlite::Connection::open(&path).unwrap();
        assert!(
            raw.execute("DELETE FROM knowledge_events", []).is_err(),
            "append-only"
        );
        assert!(
            raw.execute("UPDATE knowledge_events SET actor = 'x'", [])
                .is_err()
        );
    }
}
