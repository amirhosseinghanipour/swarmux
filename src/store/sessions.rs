use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, Row, params};
use uuid::Uuid;

use super::{Store, now_ms};

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub cwd: PathBuf,
    pub model: String,
    pub permission_mode: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub archived: bool,
}

const COLS: &str = "id, name, cwd, model, permission_mode, created_at, last_used_at, archived";

fn from_row(r: &Row) -> rusqlite::Result<Session> {
    let id: String = r.get(0)?;
    let cwd: String = r.get(2)?;
    let id = Uuid::parse_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Session {
        id,
        name: r.get(1)?,
        cwd: PathBuf::from(cwd),
        model: r.get(3)?,
        permission_mode: r.get(4)?,
        created_at: r.get(5)?,
        last_used_at: r.get(6)?,
        archived: r.get(7)?,
    })
}

impl Store {
    pub fn create_session(&self, name: &str, cwd: &Path, model: &str) -> anyhow::Result<Session> {
        let s = Session {
            id: Uuid::new_v4(),
            name: name.to_string(),
            cwd: cwd.to_path_buf(),
            model: model.to_string(),
            permission_mode: "default".to_string(),
            created_at: now_ms(),
            last_used_at: None,
            archived: false,
        };
        self.conn.execute(
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
        )?;
        Ok(s)
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM sessions WHERE archived = 0
             ORDER BY COALESCE(last_used_at, created_at) DESC, rowid DESC"
        ))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_session(&self, id: Uuid) -> anyhow::Result<Option<Session>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM sessions WHERE id = ?1"),
                [id.to_string()],
                from_row,
            )
            .optional()?)
    }

    pub fn touch_session(&self, id: Uuid) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET last_used_at = ?1 WHERE id = ?2",
            params![now_ms(), id.to_string()],
        )?;
        Ok(())
    }

    pub fn session_has_jobs(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE session_id = ?1)",
            [id.to_string()],
            |r| r.get(0),
        )?)
    }
}
