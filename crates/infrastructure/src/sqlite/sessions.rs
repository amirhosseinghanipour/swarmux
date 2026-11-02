use std::path::PathBuf;

use rusqlite::{OptionalExtension, Row, params};
use swarmux_application::AppResult;
use swarmux_application::ports::SessionRepository;
use swarmux_domain::{Millis, Session, SessionId};
use uuid::Uuid;

use super::{OrStorage, SqliteStore};

const COLS: &str = "id, name, cwd, model, permission_mode, created_at, last_used_at, archived";

fn from_row(r: &Row) -> rusqlite::Result<Session> {
    let id: String = r.get(0)?;
    let id = Uuid::parse_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let cwd: String = r.get(2)?;
    Ok(Session {
        id: SessionId(id),
        name: r.get(1)?,
        cwd: PathBuf::from(cwd),
        model: r.get(3)?,
        permission_mode: r.get(4)?,
        created_at: r.get(5)?,
        last_used_at: r.get(6)?,
        archived: r.get(7)?,
    })
}

impl SessionRepository for SqliteStore {
    fn insert(&self, s: &Session) -> AppResult<()> {
        self.conn
            .execute(
                &format!("INSERT INTO sessions ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                params![
                    s.id.to_string(),
                    s.name,
                    s.cwd.to_string_lossy(),
                    s.model,
                    s.permission_mode,
                    s.created_at,
                    s.last_used_at,
                    s.archived
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn list_active(&self) -> AppResult<Vec<Session>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLS} FROM sessions WHERE archived = 0
                 ORDER BY COALESCE(last_used_at, created_at) DESC, rowid DESC"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], from_row).or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn get(&self, id: SessionId) -> AppResult<Option<Session>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM sessions WHERE id = ?1"),
                [id.to_string()],
                from_row,
            )
            .optional()
            .or_storage()
    }

    fn touch(&self, id: SessionId, now: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE sessions SET last_used_at = ?1 WHERE id = ?2",
                params![now, id.to_string()],
            )
            .or_storage()?;
        Ok(())
    }

    fn has_started(&self, id: SessionId) -> AppResult<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events e JOIN jobs j ON e.job_id = j.id
                               WHERE j.session_id = ?1 AND e.kind = 'init')",
                [id.to_string()],
                |r| r.get(0),
            )
            .or_storage()
    }
}
