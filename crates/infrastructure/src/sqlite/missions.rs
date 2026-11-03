//! Missions in the database: the contract as TOML, the lint and spend as
//! JSON, the state as its name.

use rusqlite::{Row, params};
use serde_json::{Value, json};
use swarmux_application::{AppResult, MissionStore};
use swarmux_domain::mission::{
    Finding, FindingKind, LintReport, Mission, MissionState, Question, Severity, Spend,
};
use swarmux_domain::{JobId, SessionId};

use super::{OrStorage, SqliteStore};
use crate::mission::{parse_contract, render_contract};

const COLS: &str =
    "slug, root, session_id, contract, state, job_id, lint, spend, note, created_at, updated_at";

fn lint_to_json(l: &LintReport) -> String {
    json!({
        "findings": l.findings.iter().map(|f| json!({
            "kind": f.kind.as_str(),
            "severity": f.severity.as_str(),
            "detail": f.detail,
            "protect": f.protect,
        })).collect::<Vec<_>>(),
        "questions": l.questions.iter().map(|q| json!({
            "question": q.question,
            "why": q.why,
        })).collect::<Vec<_>>(),
        "source": l.source,
        "problem": l.problem,
        "cost_usd": l.cost_usd,
    })
    .to_string()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn lint_from_json(s: &str) -> Option<LintReport> {
    let v: Value = serde_json::from_str(s).ok()?;
    let items = |key: &str| {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    Some(LintReport {
        findings: items("findings")
            .iter()
            .filter_map(|f| {
                Some(Finding {
                    kind: FindingKind::parse(f.get("kind")?.as_str()?)?,
                    severity: Severity::parse(f.get("severity")?.as_str()?)?,
                    detail: text(f, "detail"),
                    protect: f.get("protect").and_then(Value::as_str).map(str::to_string),
                })
            })
            .collect(),
        questions: items("questions")
            .iter()
            .map(|q| Question {
                question: text(q, "question"),
                why: text(q, "why"),
            })
            .collect(),
        source: text(&v, "source"),
        problem: v.get("problem").and_then(Value::as_str).map(str::to_string),
        cost_usd: v.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
    })
}

fn spend_to_json(s: &Spend) -> String {
    json!({
        "plan": s.plan,
        "implement": s.implement,
        "verify": s.verify,
        "review": s.review,
    })
    .to_string()
}

fn spend_from_json(s: &str) -> Spend {
    let v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Spend {
        plan: f("plan"),
        implement: f("implement"),
        verify: f("verify"),
        review: f("review"),
    }
}

fn conversion(idx: usize, why: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, why.into())
}

fn from_row(r: &Row) -> rusqlite::Result<Mission> {
    let contract: String = r.get(3)?;
    let state: String = r.get(4)?;
    let session: Option<String> = r.get(2)?;
    let lint: Option<String> = r.get(6)?;
    let spend: String = r.get(7)?;
    Ok(Mission {
        slug: r.get(0)?,
        root: r.get(1)?,
        session: session.as_deref().and_then(SessionId::parse),
        contract: parse_contract(&contract).map_err(|e| conversion(3, e))?,
        state: MissionState::parse(&state)
            .ok_or_else(|| conversion(4, format!("unknown mission state {state:?}")))?,
        job: r.get::<_, Option<i64>>(5)?.map(JobId),
        lint: lint.as_deref().and_then(lint_from_json),
        spend: spend_from_json(&spend),
        note: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

impl MissionStore for SqliteStore {
    fn save(&self, m: &Mission) -> AppResult<()> {
        self.conn
            .execute(
                &format!(
                    "INSERT INTO missions ({COLS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(slug) DO UPDATE SET
                        root = excluded.root,
                        session_id = excluded.session_id,
                        contract = excluded.contract,
                        state = excluded.state,
                        job_id = excluded.job_id,
                        lint = excluded.lint,
                        spend = excluded.spend,
                        note = excluded.note,
                        updated_at = excluded.updated_at"
                ),
                params![
                    m.slug,
                    m.root,
                    m.session.map(|s| s.to_string()),
                    render_contract(&m.contract),
                    m.state.as_str(),
                    m.job.map(|j| j.0),
                    m.lint.as_ref().map(lint_to_json),
                    spend_to_json(&m.spend),
                    m.note,
                    m.created_at,
                    m.updated_at,
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<Mission>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLS} FROM missions ORDER BY created_at, slug"
            ))
            .or_storage()?;
        let rows = stmt.query_map([], from_row).or_storage()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::mission::{Contract, lint_report};

    fn mission(slug: &str, at: i64) -> Mission {
        let contract = Contract {
            title: "Fast search".into(),
            goal: "go faster".into(),
            ..Contract::default()
        };
        let mut m = Mission::new(slug, "/repo", contract, at);
        m.lint = Some(lint_report(&m.contract));
        m
    }

    #[test]
    fn missions_round_trip_and_update_in_place() {
        let s = SqliteStore::open_in_memory().unwrap();
        let mut a = mission("a", 2);
        let b = mission("b", 1);
        s.save(&a).unwrap();
        s.save(&b).unwrap();
        a.state = MissionState::Running;
        a.job = Some(JobId(7));
        a.session = Some(SessionId::random());
        a.spend.implement = 0.42;
        a.note = Some("why".into());
        a.lint.as_mut().unwrap().problem = Some("offline".into());
        a.updated_at = 9;
        s.save(&a).unwrap();
        let all = s.list().unwrap();
        assert_eq!(all, [b, a], "oldest first, one row each");
    }

    #[test]
    fn a_mission_without_a_lint_or_session_reads_back() {
        let s = SqliteStore::open_in_memory().unwrap();
        let mut m = mission("m", 1);
        m.lint = None;
        s.save(&m).unwrap();
        assert_eq!(s.list().unwrap(), [m]);
    }
}
