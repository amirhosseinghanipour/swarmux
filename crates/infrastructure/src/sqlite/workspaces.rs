//! Workspaces and their repositories. A repository's row is keyed by its
//! workspace and its name, which a rescan keeps, so whatever refers to a
//! repository can use that pair.

use std::path::PathBuf;

use rusqlite::{OptionalExtension, Row, params};
use serde_json::{Value, json};
use swarmux_application::{AppResult, WorkspaceStore};
use swarmux_domain::workspace::RepoCommand;
use swarmux_domain::{Repo, Stack, Workspace, WorkspaceId};

use super::{OrStorage, SqliteStore};

const COLS: &str = "id, slug, name, root, created_at, updated_at";
const REPO_COLS: &str = "name, path, git_root, default_branch, stacks, commands";

fn conversion(idx: usize, why: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, why.into())
}

fn from_row(r: &Row) -> rusqlite::Result<Workspace> {
    let id: String = r.get(0)?;
    let root: String = r.get(3)?;
    Ok(Workspace {
        id: WorkspaceId::parse(&id).ok_or_else(|| conversion(0, format!("bad id {id:?}")))?,
        slug: r.get(1)?,
        name: r.get(2)?,
        root: PathBuf::from(root),
        repos: Vec::new(),
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn commands_to_json(commands: &[RepoCommand]) -> String {
    Value::Array(
        commands
            .iter()
            .map(|c| json!({"name": c.name, "line": c.line, "source": c.source}))
            .collect(),
    )
    .to_string()
}

fn commands_from_json(text: &str) -> Vec<RepoCommand> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let s = |c: &Value, k: &str| {
        c.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|c| RepoCommand {
                    name: s(c, "name"),
                    line: s(c, "line"),
                    source: s(c, "source"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn repo_from_row(r: &Row) -> rusqlite::Result<Repo> {
    let git_root: String = r.get(2)?;
    let stacks: String = r.get(4)?;
    let commands: String = r.get(5)?;
    Ok(Repo {
        name: r.get(0)?,
        path: r.get(1)?,
        git_root: PathBuf::from(git_root),
        default_branch: r.get(3)?,
        stacks: stacks.split(',').filter_map(Stack::parse).collect(),
        commands: commands_from_json(&commands),
    })
}

impl SqliteStore {
    fn repos_of(&self, id: WorkspaceId) -> AppResult<Vec<Repo>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {REPO_COLS} FROM workspace_repos WHERE workspace_id = ?1 ORDER BY ord, name"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map([id.to_string()], repo_from_row)
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }
}

impl WorkspaceStore for SqliteStore {
    fn save(&self, w: &Workspace) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        tx.execute(
            &format!(
                "INSERT INTO workspaces ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    slug = excluded.slug,
                    name = excluded.name,
                    root = excluded.root,
                    updated_at = excluded.updated_at"
            ),
            params![
                w.id.to_string(),
                w.slug,
                w.name,
                w.root.to_string_lossy(),
                w.created_at,
                w.updated_at
            ],
        )
        .or_storage()?;
        // Rows of repositories that are gone go; the others are updated in
        // place, so their keys stay.
        let names: Vec<String> = w.repos.iter().map(|r| r.name.clone()).collect();
        let kept = serde_json::to_string(&names).unwrap_or_else(|_| "[]".into());
        tx.execute(
            "DELETE FROM workspace_repos WHERE workspace_id = ?1
             AND name NOT IN (SELECT value FROM json_each(?2))",
            params![w.id.to_string(), kept],
        )
        .or_storage()?;
        for (ord, r) in w.repos.iter().enumerate() {
            let stacks: Vec<&str> = r.stacks.iter().map(|s| s.as_str()).collect();
            tx.execute(
                "INSERT INTO workspace_repos
                    (workspace_id, name, path, git_root, default_branch, stacks, commands, ord)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(workspace_id, name) DO UPDATE SET
                    path = excluded.path,
                    git_root = excluded.git_root,
                    default_branch = excluded.default_branch,
                    stacks = excluded.stacks,
                    commands = excluded.commands,
                    ord = excluded.ord",
                params![
                    w.id.to_string(),
                    r.name,
                    r.path,
                    r.git_root.to_string_lossy(),
                    r.default_branch,
                    stacks.join(","),
                    commands_to_json(&r.commands),
                    i64::try_from(ord).unwrap_or(i64::MAX),
                ],
            )
            .or_storage()?;
        }
        tx.commit().or_storage()
    }

    fn list(&self) -> AppResult<Vec<Workspace>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLS} FROM workspaces ORDER BY name COLLATE NOCASE, slug"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], from_row).or_storage()?;
        let mut all: Vec<Workspace> = rows.collect::<Result<_, _>>().or_storage()?;
        for w in &mut all {
            w.repos = self.repos_of(w.id)?;
        }
        Ok(all)
    }

    fn get(&self, id: WorkspaceId) -> AppResult<Option<Workspace>> {
        let found = self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM workspaces WHERE id = ?1"),
                [id.to_string()],
                from_row,
            )
            .optional()
            .or_storage()?;
        let Some(mut w) = found else {
            return Ok(None);
        };
        w.repos = self.repos_of(id)?;
        Ok(Some(w))
    }

    fn remove(&self, id: WorkspaceId) -> AppResult<()> {
        let tx = self.conn.unchecked_transaction().or_storage()?;
        let id = id.to_string();
        tx.execute(
            "UPDATE sessions SET workspace_id = NULL WHERE workspace_id = ?1",
            [&id],
        )
        .or_storage()?;
        tx.execute(
            "UPDATE missions SET workspace_id = NULL WHERE workspace_id = ?1",
            [&id],
        )
        .or_storage()?;
        tx.execute("DELETE FROM workspaces WHERE id = ?1", [&id])
            .or_storage()?;
        tx.commit().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ports::SessionRepository;
    use swarmux_domain::{Session, SessionId};

    fn repo(name: &str, path: &str) -> Repo {
        Repo {
            name: name.into(),
            path: path.into(),
            git_root: PathBuf::from("/p").join(path),
            default_branch: Some("main".into()),
            stacks: vec![Stack::Flutter, Stack::Supabase],
            commands: vec![RepoCommand {
                name: "test".into(),
                line: "flutter test".into(),
                source: "flutter".into(),
            }],
        }
    }

    fn workspace() -> Workspace {
        let mut w = Workspace::new(WorkspaceId::random(), "Product", "/p".into(), 1).unwrap();
        w.set_repos(vec![repo("app", "app"), repo("backend", "backend")], 2);
        w
    }

    #[test]
    fn workspaces_round_trip_with_their_repositories() {
        let s = SqliteStore::open_in_memory().unwrap();
        let mut w = workspace();
        s.save(&w).unwrap();
        assert_eq!(WorkspaceStore::get(&s, w.id).unwrap(), Some(w.clone()));
        // A rescan: one gone, one new, one changed; the order is kept.
        w.repos.remove(0);
        w.repos.push(repo("admin", "admin"));
        w.repos[0].default_branch = None;
        w.updated_at = 3;
        s.save(&w).unwrap();
        assert_eq!(s.list().unwrap(), [w.clone()]);
        assert_eq!(
            WorkspaceStore::get(&s, WorkspaceId::random()).unwrap(),
            None
        );
    }

    #[test]
    fn removing_a_workspace_leaves_its_sessions_as_plain_ones() {
        let s = SqliteStore::open_in_memory().unwrap();
        let w = workspace();
        s.save(&w).unwrap();
        let mut session = Session::new(SessionId::random(), "p", "/p".into(), "sonnet", 1).unwrap();
        session.workspace = Some(w.id);
        SessionRepository::insert(&s, &session).unwrap();
        assert_eq!(
            SessionRepository::get(&s, session.id)
                .unwrap()
                .unwrap()
                .workspace,
            Some(w.id)
        );
        s.remove(w.id).unwrap();
        assert!(s.list().unwrap().is_empty());
        let left = SessionRepository::get(&s, session.id).unwrap().unwrap();
        assert_eq!(left.workspace, None);
        assert_eq!(left.cwd, PathBuf::from("/p"));
        let rows: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM workspace_repos", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "its repositories go with it");
    }
}
