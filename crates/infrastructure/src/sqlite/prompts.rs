use rusqlite::{OptionalExtension, Row, params};
use swarmux_application::ports::PromptRepository;
use swarmux_application::{AppError, AppResult};
use swarmux_domain::{Millis, PromptDraft, PromptId, PromptTemplate, parse_tags};

use super::{OrStorage, SqliteStore};

const COLS: &str = "id, name, body, tags, created_at, updated_at";

fn from_row(r: &Row) -> rusqlite::Result<PromptTemplate> {
    let tags: String = r.get(3)?;
    Ok(PromptTemplate {
        id: PromptId(r.get(0)?),
        name: r.get(1)?,
        body: r.get(2)?,
        tags: parse_tags(&tags),
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

/// Tags as stored: comma-separated, trimmed, no empty ones.
fn join_tags(tags: &[String]) -> String {
    parse_tags(&tags.join(",")).join(",")
}

impl PromptRepository for SqliteStore {
    fn insert(&self, d: &PromptDraft, now: Millis) -> AppResult<PromptId> {
        self.conn
            .execute(
                "INSERT INTO prompts (name, body, tags, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![d.name, d.body, join_tags(&d.tags), now],
            )
            .or_storage()?;
        Ok(PromptId(self.conn.last_insert_rowid()))
    }

    fn update(&self, id: PromptId, d: &PromptDraft, now: Millis) -> AppResult<()> {
        let changed = self
            .conn
            .execute(
                "UPDATE prompts SET name = ?1, body = ?2, tags = ?3, updated_at = ?4 WHERE id = ?5",
                params![d.name, d.body, join_tags(&d.tags), now, id.0],
            )
            .or_storage()?;
        if changed == 0 {
            return Err(AppError::PromptNotFound);
        }
        Ok(())
    }

    fn delete(&self, id: PromptId) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        tx.execute(
            "UPDATE jobs SET prompt_id = NULL WHERE prompt_id = ?1",
            [id.0],
        )
        .or_storage()?;
        tx.execute("DELETE FROM prompts WHERE id = ?1", [id.0])
            .or_storage()?;
        tx.commit().or_storage()
    }

    fn get(&self, id: PromptId) -> AppResult<Option<PromptTemplate>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM prompts WHERE id = ?1"),
                [id.0],
                from_row,
            )
            .optional()
            .or_storage()
    }

    fn list(&self) -> AppResult<Vec<PromptTemplate>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLS} FROM prompts ORDER BY updated_at DESC, id DESC"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], from_row).or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ports::{JobRepository, SessionRepository};
    use swarmux_domain::{Session, SessionId};

    fn draft(name: &str, tags: &[&str]) -> PromptDraft {
        let tags = tags.iter().map(|t| t.to_string()).collect();
        PromptDraft::new(name, "body {{x}}\nline 2 é", tags).unwrap()
    }

    fn names(s: &SqliteStore) -> Vec<String> {
        s.list().unwrap().into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn prompt_round_trips_with_clean_tags() {
        let s = SqliteStore::open_in_memory().unwrap();
        let id =
            PromptRepository::insert(&s, &draft("a", &[" rust ", "", "x,y", "rust"]), 5).unwrap();
        let p = PromptRepository::get(&s, id).unwrap().unwrap();
        assert_eq!(p.name, "a");
        assert_eq!(p.body, "body {{x}}\nline 2 é");
        assert_eq!(p.tags, ["rust", "x", "y"]);
        assert_eq!((p.created_at, p.updated_at), (5, 5));
        assert_eq!(PromptRepository::get(&s, PromptId(99)).unwrap(), None);
    }

    #[test]
    fn list_orders_by_last_update() {
        let s = SqliteStore::open_in_memory().unwrap();
        let a = PromptRepository::insert(&s, &draft("a", &[]), 1).unwrap();
        PromptRepository::insert(&s, &draft("b", &[]), 2).unwrap();
        assert_eq!(names(&s), ["b", "a"]);
        PromptRepository::update(&s, a, &draft("a2", &["t"]), 3).unwrap();
        assert_eq!(names(&s), ["a2", "b"]);
        let p = PromptRepository::get(&s, a).unwrap().unwrap();
        assert_eq!(
            (p.created_at, p.updated_at, p.tags),
            (1, 3, vec!["t".into()])
        );
    }

    #[test]
    fn update_of_missing_prompt_fails() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(matches!(
            PromptRepository::update(&s, PromptId(7), &draft("a", &[]), 1),
            Err(AppError::PromptNotFound)
        ));
    }

    #[test]
    fn delete_keeps_jobs_rendered_from_the_prompt() {
        let s = SqliteStore::open_in_memory().unwrap();
        let sess = Session::new(SessionId::random(), "s", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&s, &sess).unwrap();
        let id = PromptRepository::insert(&s, &draft("a", &[]), 1).unwrap();
        let mut new = crate::sqlite::tests::new_job(sess.id, "body");
        new.prompt_id = Some(id);
        let job = JobRepository::insert(&s, &new, 2).unwrap();
        assert_eq!(s.job_prompt(job), Some(id.0));

        PromptRepository::delete(&s, id).unwrap();
        assert!(names(&s).is_empty());
        assert_eq!(s.job_prompt(job), None);
    }
}
