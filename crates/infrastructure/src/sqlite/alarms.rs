//! The inbox's memory in SQLite: the alarm log, shelved items and the tally
//! of how each kind of ask was answered.

use rusqlite::params;
use swarmux_application::{AlarmEvent, AlarmKind, AlarmStore, AppResult, ShelvedItem, TallyRow};
use swarmux_domain::alarm::Tally;
use swarmux_domain::{AttentionId, JobId, Millis, Priority};

use super::{OrStorage, SqliteStore};

fn priority_of(s: &str) -> Option<Priority> {
    match s {
        "P1" => Some(Priority::P1),
        "P2" => Some(Priority::P2),
        "P3" => Some(Priority::P3),
        _ => None,
    }
}

impl AlarmStore for SqliteStore {
    fn log(&self, event: &AlarmEvent) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO alarm_log (at, kind, key, priority, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.at,
                    event.kind.as_str(),
                    event.key,
                    event.priority.map(Priority::as_str),
                    event.detail
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn events(&self, since: Millis) -> AppResult<Vec<AlarmEvent>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at, kind, key, priority, detail FROM alarm_log
                 WHERE at >= ?1 ORDER BY at, id",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([since], |r| {
                let kind: String = r.get(1)?;
                let priority: Option<String> = r.get(3)?;
                Ok((
                    AlarmKind::parse(&kind),
                    AlarmEvent {
                        at: r.get(0)?,
                        kind: AlarmKind::Raised,
                        key: r.get(2)?,
                        priority: priority.as_deref().and_then(priority_of),
                        detail: r.get(4)?,
                    },
                ))
            })
            .or_storage()?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, event) = row.or_storage()?;
            // A kind from a newer swarmux is skipped, not an error.
            if let Some(kind) = kind {
                out.push(AlarmEvent { kind, ..event });
            }
        }
        Ok(out)
    }

    fn save_shelved(&self, item: &ShelvedItem) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO alarm_shelved (item, job_id, shelved_at, until_at, until_job_ends)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(item) DO UPDATE SET
                   job_id = excluded.job_id, shelved_at = excluded.shelved_at,
                   until_at = excluded.until_at, until_job_ends = excluded.until_job_ends",
                params![
                    item.id.as_str(),
                    item.job.map(|j| j.0),
                    item.shelved_at,
                    item.until,
                    item.until_job_ends
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn remove_shelved(&self, id: &AttentionId) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM alarm_shelved WHERE item = ?1", [id.as_str()])
            .or_storage()?;
        Ok(())
    }

    fn shelved(&self) -> AppResult<Vec<ShelvedItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT item, job_id, shelved_at, until_at, until_job_ends
                 FROM alarm_shelved ORDER BY shelved_at, item",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ShelvedItem {
                    id: AttentionId::new(r.get::<_, String>(0)?),
                    job: r.get::<_, Option<i64>>(1)?.map(JobId),
                    shelved_at: r.get(2)?,
                    until: r.get(3)?,
                    until_job_ends: r.get(4)?,
                })
            })
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn tallies(&self) -> AppResult<Vec<TallyRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT scope, signature, approved, rejected, suggested
                 FROM alarm_tally ORDER BY scope, signature",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TallyRow {
                    scope: r.get(0)?,
                    signature: r.get(1)?,
                    tally: Tally {
                        approved: r.get(2)?,
                        rejected: r.get(3)?,
                        suggested: r.get(4)?,
                    },
                })
            })
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn save_tally(&self, row: &TallyRow) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO alarm_tally (scope, signature, approved, rejected, suggested)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(scope, signature) DO UPDATE SET
                   approved = excluded.approved, rejected = excluded.rejected,
                   suggested = excluded.suggested",
                params![
                    row.scope,
                    row.signature,
                    row.tally.approved,
                    row.tally.rejected,
                    row.tally.suggested
                ],
            )
            .or_storage()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at: Millis, kind: AlarmKind, key: &str, p: Option<Priority>) -> AlarmEvent {
        AlarmEvent {
            at,
            kind,
            key: key.into(),
            priority: p,
            detail: "x".into(),
        }
    }

    #[test]
    fn the_log_comes_back_in_order_from_a_point_in_time() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.log(&event(
            30,
            AlarmKind::Attended,
            "job:1:failed",
            Some(Priority::P2),
        ))
        .unwrap();
        s.log(&event(
            10,
            AlarmKind::Raised,
            "job:1:failed",
            Some(Priority::P2),
        ))
        .unwrap();
        s.log(&event(20, AlarmKind::Shelved, "approval:3", None))
            .unwrap();
        let all = s.events(0).unwrap();
        assert_eq!(
            all.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [AlarmKind::Raised, AlarmKind::Shelved, AlarmKind::Attended]
        );
        assert_eq!(all[0].priority, Some(Priority::P2));
        assert_eq!(all[1].priority, None);
        assert_eq!(s.events(25).unwrap().len(), 1);
    }

    #[test]
    fn unknown_kinds_are_skipped() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.conn
            .execute(
                "INSERT INTO alarm_log (at, kind, key, detail) VALUES (1, 'future', 'k', '')",
                [],
            )
            .unwrap();
        s.log(&event(2, AlarmKind::Raised, "k", Some(Priority::P3)))
            .unwrap();
        assert_eq!(s.events(0).unwrap().len(), 1);
    }

    #[test]
    fn shelved_items_round_trip_replace_and_go() {
        let s = SqliteStore::open_in_memory().unwrap();
        let timed = ShelvedItem {
            id: AttentionId::new("job:1:failed"),
            job: Some(JobId(1)),
            shelved_at: 5,
            until: Some(900_005),
            until_job_ends: false,
        };
        let by_job = ShelvedItem {
            id: AttentionId::new("approval:2"),
            job: Some(JobId(7)),
            shelved_at: 6,
            until: None,
            until_job_ends: true,
        };
        s.save_shelved(&timed).unwrap();
        s.save_shelved(&by_job).unwrap();
        s.save_shelved(&ShelvedItem {
            until: Some(1),
            ..timed.clone()
        })
        .unwrap();
        let got = s.shelved().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].until, Some(1));
        assert_eq!(got[1], by_job);
        s.remove_shelved(&timed.id).unwrap();
        assert_eq!(s.shelved().unwrap(), [by_job]);
    }

    #[test]
    fn tallies_and_shelves_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let row = TallyRow {
            scope: "C:/work".into(),
            signature: "Bash\u{1f}cargo test".into(),
            tally: Tally {
                approved: 12,
                rejected: 0,
                suggested: true,
            },
        };
        {
            let s = SqliteStore::open(&path).unwrap();
            s.save_tally(&row).unwrap();
            s.save_tally(&TallyRow {
                tally: Tally {
                    approved: 11,
                    ..row.tally
                },
                ..row.clone()
            })
            .unwrap();
            s.save_tally(&row).unwrap();
        }
        let again = SqliteStore::open(&path).unwrap();
        assert_eq!(again.tallies().unwrap(), [row]);
    }

    #[test]
    fn the_migration_is_the_next_version() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.schema_version().unwrap() >= 13);
    }
}
