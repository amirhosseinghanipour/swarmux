//! Follow-ups, intents and follow-through state in the database.

use rusqlite::{OptionalExtension, Row, params};
use serde_json::{Value, json};
use swarmux_application::{AppResult, FollowStore, Intent, IntentStore};
use swarmux_domain::JobId;
use swarmux_domain::followup::{DecidedBy, FollowKind, FollowStatus, FollowUp, Source, Urgency};

use super::{OrStorage, SqliteStore};

const COLUMNS: &str = "id, kind, text, rationale, against, repo, paths, urgency, est_cost, \
     sources, created, status, decided_by, decided_at, due_at";
/// Follow-ups loaded: older ones are history.
const KEPT: i64 = 3000;

fn sources_json(sources: &[Source]) -> String {
    let v: Vec<Value> = sources
        .iter()
        .map(|s| json!({"job": s.job.map(|j| j.0), "mission": s.mission, "board": s.board_entry}))
        .collect();
    Value::Array(v).to_string()
}

fn sources_of(text: &str) -> Vec<Source> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|v| Source {
            job: v.get("job").and_then(Value::as_i64).map(JobId),
            mission: v.get("mission").and_then(Value::as_str).map(String::from),
            board_entry: v.get("board").and_then(Value::as_i64),
        })
        .collect()
}

fn strings(text: &str) -> Vec<String> {
    serde_json::from_str(text).unwrap_or_default()
}

fn follow(r: &Row<'_>) -> rusqlite::Result<Option<FollowUp>> {
    let kind: String = r.get(1)?;
    let urgency: String = r.get(7)?;
    let status: String = r.get(11)?;
    let decided_by: Option<String> = r.get(12)?;
    let (Some(kind), Some(status)) = (FollowKind::parse(&kind), FollowStatus::parse(&status))
    else {
        // A row of a kind or status this version doesn't know is left out.
        return Ok(None);
    };
    Ok(Some(FollowUp {
        id: r.get(0)?,
        kind,
        text: r.get(2)?,
        rationale: r.get(3)?,
        against: r.get(4)?,
        repo: r.get(5)?,
        paths: strings(&r.get::<_, String>(6)?),
        urgency: Urgency::parse(&urgency).unwrap_or_default(),
        est_cost_usd: r.get(8)?,
        sources: sources_of(&r.get::<_, String>(9)?),
        created: r.get(10)?,
        status,
        decided_by: decided_by.as_deref().and_then(DecidedBy::parse),
        decided_at: r.get(13)?,
        due_at: r.get(14)?,
    }))
}

impl FollowStore for SqliteStore {
    fn insert(&self, f: &FollowUp) -> AppResult<i64> {
        self.conn
            .execute(
                "INSERT INTO followups (kind, text, rationale, against, repo, paths, urgency,
                    est_cost, sources, created, status, decided_by, decided_at, due_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    f.kind.as_str(),
                    f.text,
                    f.rationale,
                    f.against,
                    f.repo,
                    serde_json::to_string(&f.paths).unwrap_or_else(|_| "[]".into()),
                    f.urgency.as_str(),
                    f.est_cost_usd,
                    sources_json(&f.sources),
                    f.created,
                    f.status.as_str(),
                    f.decided_by.as_ref().map(DecidedBy::to_text),
                    f.decided_at,
                    f.due_at,
                ],
            )
            .or_storage()?;
        Ok(self.conn.last_insert_rowid())
    }

    fn update(&self, f: &FollowUp) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE followups SET rationale = ?2, against = ?3, repo = ?4, paths = ?5,
                    urgency = ?6, est_cost = ?7, sources = ?8, created = ?9, status = ?10,
                    decided_by = ?11, decided_at = ?12, due_at = ?13 WHERE id = ?1",
                params![
                    f.id,
                    f.rationale,
                    f.against,
                    f.repo,
                    serde_json::to_string(&f.paths).unwrap_or_else(|_| "[]".into()),
                    f.urgency.as_str(),
                    f.est_cost_usd,
                    sources_json(&f.sources),
                    f.created,
                    f.status.as_str(),
                    f.decided_by.as_ref().map(DecidedBy::to_text),
                    f.decided_at,
                    f.due_at,
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn get(&self, id: i64) -> AppResult<Option<FollowUp>> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM followups WHERE id = ?1"),
                [id],
                follow,
            )
            .optional()
            .or_storage()?;
        Ok(row.flatten())
    }

    fn all(&self) -> AppResult<Vec<FollowUp>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM followups ORDER BY id DESC LIMIT ?1"
            ))
            .or_storage()?;
        let rows = stmt
            .query_map([KEPT], follow)
            .or_storage()?
            .collect::<rusqlite::Result<Vec<_>>>()
            .or_storage()?;
        let mut out: Vec<FollowUp> = rows.into_iter().flatten().collect();
        out.reverse();
        Ok(out)
    }

    fn state(&self, key: &str) -> AppResult<Option<i64>> {
        self.conn
            .query_row(
                "SELECT value FROM followup_state WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .or_storage()
    }

    fn set_state(&self, key: &str, value: i64) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO followup_state (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .or_storage()?;
        Ok(())
    }
}

impl IntentStore for SqliteStore {
    fn add(&self, i: &Intent) -> AppResult<i64> {
        self.conn
            .execute(
                "INSERT INTO followup_intents (followup, text, repo, paths, created, mission)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    i.followup,
                    i.text,
                    i.repo,
                    serde_json::to_string(&i.paths).unwrap_or_else(|_| "[]".into()),
                    i.created,
                    i.mission,
                ],
            )
            .or_storage()?;
        Ok(self.conn.last_insert_rowid())
    }

    fn list(&self) -> AppResult<Vec<Intent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, followup, text, repo, paths, created, mission
                 FROM followup_intents ORDER BY id",
            )
            .or_storage()?;
        stmt.query_map([], |r| {
            Ok(Intent {
                id: r.get(0)?,
                followup: r.get(1)?,
                text: r.get(2)?,
                repo: r.get(3)?,
                paths: strings(&r.get::<_, String>(4)?),
                created: r.get(5)?,
                mission: r.get(6)?,
            })
        })
        .or_storage()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .or_storage()
    }

    fn mark_mission(&self, id: i64, slug: &str) -> AppResult<()> {
        self.conn
            .execute(
                "UPDATE followup_intents SET mission = ?2 WHERE id = ?1",
                params![id, slug],
            )
            .or_storage()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_ups_intents_and_state_round_trip() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.schema_version().unwrap() >= 21);
        let mut f = FollowUp::new(
            FollowKind::Risk,
            "leaks a key",
            Source {
                job: Some(JobId(4)),
                mission: Some("m".into()),
                board_entry: Some(9),
            },
            100,
        );
        f.paths = vec!["a.rs".into()];
        f.urgency = Urgency::High;
        f.est_cost_usd = Some(0.25);
        f.id = s.insert(&f).unwrap();
        assert_eq!(s.get(f.id).unwrap(), Some(f.clone()));
        f.status = FollowStatus::Approved;
        f.decided_by = Some(DecidedBy::Order("docs".into()));
        f.decided_at = Some(200);
        f.sources.push(Source::default());
        s.update(&f).unwrap();
        assert_eq!(s.all().unwrap(), vec![f.clone()]);
        assert_eq!(s.state("brief").unwrap(), None);
        s.set_state("brief", 5).unwrap();
        s.set_state("brief", 6).unwrap();
        assert_eq!(s.state("brief").unwrap(), Some(6));
        let i = Intent {
            id: 0,
            followup: f.id,
            text: "t".into(),
            repo: "r".into(),
            paths: vec!["p".into()],
            created: 1,
            mission: None,
        };
        let id = s.add(&i).unwrap();
        s.mark_mission(id, "slug").unwrap();
        let got = s.list().unwrap();
        assert_eq!(got[0].mission.as_deref(), Some("slug"));
        assert_eq!(got[0].paths, vec!["p".to_string()]);
    }
}
