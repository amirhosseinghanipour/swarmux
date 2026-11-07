//! Prompt-cache usage per finished job, for `swarmux stats cache`.

use swarmux_application::AppResult;
use swarmux_application::cache::{CacheRow, CacheSource};
use swarmux_domain::Millis;

use super::{OrStorage, SqliteStore};

impl CacheSource for SqliteStore {
    fn cache_rows(&self, since: Millis) -> AppResult<Vec<CacheRow>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT j.model, r.tokens_in, r.cache_read, r.cache_creation
                 FROM job_results r JOIN jobs j ON j.id = r.job_id
                 WHERE j.created_at >= ?1 AND j.review_of IS NULL",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([since], |r| {
                Ok(CacheRow {
                    model: r.get(0)?,
                    tokens_in: r.get::<_, i64>(1)?.max(0) as u64,
                    cache_read: r.get::<_, i64>(2)?.max(0) as u64,
                    cache_creation: r.get::<_, Option<i64>>(3)?.map(|n| n.max(0) as u64),
                })
            })
            .or_storage()?;
        rows.collect::<rusqlite::Result<Vec<_>>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::new_job;
    use swarmux_application::cache::cache_report;
    use swarmux_application::ports::{JobRepository, SessionRepository};
    use swarmux_domain::{JobResult, Session, SessionId};

    fn result(input: u64, read: u64, created: Option<u64>) -> JobResult {
        JobResult {
            tokens_in: input,
            tokens_out: 1,
            cache_read: read,
            cache_creation: created,
            cost_usd: 0.0,
            duration_ms: 1,
            outcome: "success".into(),
            error: None,
            text: String::new(),
        }
    }

    #[test]
    fn stored_cache_usage_comes_back_per_job_and_old_rows_are_unknown() {
        let store = SqliteStore::open_in_memory().unwrap();
        let session = Session::new(SessionId::random(), "s", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&store, &session).unwrap();
        for (input, read, created) in [(10, 80, Some(10)), (10, 5, None)] {
            let id = JobRepository::insert(&store, &new_job(session.id, "p"), 1_000).unwrap();
            JobRepository::save_result(&store, id, &result(input, read, created)).unwrap();
        }
        let report = cache_report(&store, 1_000, 7).unwrap();
        assert_eq!((report.all.jobs, report.all.known), (2, 1));
        assert_eq!(report.all.ratio(), Some(0.8));
    }
}
