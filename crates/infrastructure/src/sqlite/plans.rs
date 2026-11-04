//! Missions' plans in the database: one JSON record per mission.

use rusqlite::params;
use swarmux_application::{AppResult, PlanStore};
use swarmux_domain::Millis;
use swarmux_domain::plan::PlanRecord;

use super::{OrStorage, SqliteStore};

impl PlanStore for SqliteStore {
    fn save(&self, slug: &str, record: &PlanRecord, at: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO mission_plans (slug, record, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(slug) DO UPDATE SET
                    record = excluded.record,
                    updated_at = excluded.updated_at",
                params![slug, record.encode(), at],
            )
            .or_storage()?;
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<(String, PlanRecord)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT slug, record FROM mission_plans ORDER BY updated_at, slug")
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .or_storage()?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()?;
        // A record that can't be read is left out rather than stopping the app.
        Ok(rows
            .into_iter()
            .filter_map(|(slug, text)| PlanRecord::decode(&text).ok().map(|r| (slug, r)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::plan::{Plan, PlanStatus, PlanTask};

    fn record() -> PlanRecord {
        PlanRecord {
            plan_first: Some(true),
            status: PlanStatus::Proposed,
            plan: Some(Plan {
                summary: "s".into(),
                tasks: vec![PlanTask {
                    id: "t1".into(),
                    title: "Do it".into(),
                    files: vec!["src/**".into()],
                    depends_on: vec![],
                    acceptance: "cargo test".into(),
                }],
            }),
            note: String::new(),
            rounds: 1,
            cost_usd: 0.1,
            drift: None,
        }
    }

    #[test]
    fn plans_round_trip_and_update_in_place() {
        let s = SqliteStore::open_in_memory().unwrap();
        let mut r = record();
        s.save("a", &r, 2).unwrap();
        s.save("b", &PlanRecord::default(), 1).unwrap();
        r.status = PlanStatus::Approved;
        s.save("a", &r, 3).unwrap();
        let all = PlanStore::list(&s).unwrap();
        let slugs: Vec<&str> = all.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(slugs, ["b", "a"]);
        assert_eq!(all[1].1, r);
    }

    #[test]
    fn an_unreadable_record_is_skipped() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO mission_plans (slug, record, updated_at) VALUES ('x', 'not json', 1)",
                [],
            )
            .unwrap();
        s.save("a", &record(), 2).unwrap();
        assert_eq!(PlanStore::list(&s).unwrap().len(), 1);
    }
}
