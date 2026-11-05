//! The Board's entries in the database.

use rusqlite::{Row, params};
use swarmux_application::{AppResult, BoardStore};
use swarmux_domain::JobId;
use swarmux_domain::board::{BoardEntry, BoardKind};

use super::{OrStorage, SqliteStore};

const COLUMNS: &str =
    "id, mission, job, session, task, kind, text, detail, areas, at, answer, after_minutes, closed";

impl BoardStore for SqliteStore {
    fn append(&self, e: &BoardEntry) -> AppResult<i64> {
        let areas = serde_json::to_string(&e.areas).unwrap_or_else(|_| "[]".into());
        self.conn
            .execute(
                "INSERT INTO board_entries
                    (mission, job, session, task, kind, text, detail, areas, at, answer,
                     after_minutes, closed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    e.mission,
                    e.job.0,
                    e.session,
                    e.task,
                    e.kind.as_str(),
                    e.text,
                    e.detail,
                    areas,
                    e.at,
                    e.answer,
                    e.after_minutes,
                    e.closed,
                ],
            )
            .or_storage()?;
        Ok(self.conn.last_insert_rowid())
    }

    fn recent(&self, limit: usize) -> AppResult<Vec<BoardEntry>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM board_entries ORDER BY id DESC LIMIT ?1"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map([limit as i64], entry)
            .or_storage()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .or_storage()?;
        // A row of a kind this version doesn't know is left out.
        let mut out: Vec<BoardEntry> = rows.into_iter().flatten().collect();
        out.reverse();
        Ok(out)
    }

    fn update(&self, e: &BoardEntry) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE board_entries SET session = ?2, answer = ?3, closed = ?4 WHERE id = ?1",
                params![e.id, e.session, e.answer, e.closed],
            )
            .or_storage()?;
        Ok(())
    }
}

fn entry(r: &Row<'_>) -> rusqlite::Result<Option<BoardEntry>> {
    let kind: String = r.get(5)?;
    let areas: String = r.get(8)?;
    let Some(kind) = BoardKind::parse(&kind) else {
        return Ok(None);
    };
    Ok(Some(BoardEntry {
        id: r.get(0)?,
        mission: r.get(1)?,
        job: JobId(r.get(2)?),
        session: r.get(3)?,
        task: r.get(4)?,
        kind,
        text: r.get(6)?,
        detail: r.get(7)?,
        areas: serde_json::from_str(&areas).unwrap_or_default(),
        at: r.get(9)?,
        answer: r.get(10)?,
        after_minutes: r.get(11)?,
        closed: r.get(12)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_update() {
        let s = SqliteStore::open_in_memory().unwrap();
        let mut a = BoardEntry::new("m", JobId(3), BoardKind::Claim, "note", 10);
        a.areas = vec!["src/**".into(), "a b".into()];
        a.task = Some("t1".into());
        a.session = "sess".into();
        a.after_minutes = Some(5);
        a.id = s.append(&a).unwrap();
        let b = BoardEntry::new("m", JobId(4), BoardKind::Question, "q?", 20);
        let id = s.append(&b).unwrap();
        assert_eq!((a.id, id), (1, 2));
        let all = s.recent(10).unwrap();
        assert_eq!(all[0], a);
        assert_eq!(all[1].text, "q?");
        assert_eq!(s.recent(1).unwrap()[0].id, 2, "the newest");
        let mut c = all[1].clone();
        c.answer = Some("yes".into());
        c.closed = true;
        s.update(&c).unwrap();
        assert_eq!(s.recent(10).unwrap()[1], c);
    }

    #[test]
    fn the_board_is_migration_fifteen() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.schema_version().unwrap() >= 15);
    }
}
