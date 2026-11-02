//! Usage-limit readings and session environments that outlive a run.

use rusqlite::params;
use swarmux_application::AppResult;
use swarmux_application::ports::TelemetryStore;
use swarmux_domain::Millis;
use swarmux_domain::telemetry::{Environment, RateLimit};

use super::{OrStorage, SqliteStore};

impl TelemetryStore for SqliteStore {
    fn save_limit(&self, limit: &RateLimit, observed_at: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO rate_limits
                 (window, status, utilization, resets_at, surpassed_threshold, observed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(window) DO UPDATE SET
                   status = excluded.status, utilization = excluded.utilization,
                   resets_at = excluded.resets_at,
                   surpassed_threshold = excluded.surpassed_threshold,
                   observed_at = excluded.observed_at
                 WHERE excluded.observed_at >= rate_limits.observed_at",
                params![
                    limit.window,
                    limit.status,
                    limit.utilization,
                    limit.resets_at,
                    limit.surpassed_threshold,
                    observed_at
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn limits(&self) -> AppResult<Vec<(RateLimit, Millis)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT window, status, utilization, resets_at, surpassed_threshold, observed_at
                 FROM rate_limits ORDER BY window",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    RateLimit {
                        window: r.get(0)?,
                        status: r.get(1)?,
                        utilization: r.get(2)?,
                        resets_at: r.get(3)?,
                        surpassed_threshold: r.get(4)?,
                    },
                    r.get(5)?,
                ))
            })
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }

    fn save_environment(&self, env: &Environment, seen_at: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO session_environments (session_id, environment, seen_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                   environment = excluded.environment, seen_at = excluded.seen_at",
                params![env.session_id, env.encode(), seen_at],
            )
            .or_storage()?;
        Ok(())
    }

    fn environments(&self, limit: usize) -> AppResult<Vec<(Environment, Millis)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT environment, seen_at FROM session_environments
                 ORDER BY seen_at DESC LIMIT ?1",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Millis>(1)?))
            })
            .or_storage()?;
        let rows: Vec<(String, Millis)> = rows.collect::<Result<_, _>>().or_storage()?;
        // A row this version can't read is skipped, not fatal.
        Ok(rows
            .into_iter()
            .filter_map(|(text, at)| Environment::decode(&text).map(|e| (e, at)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::telemetry::McpServer;

    fn limit(window: &str, used: f64) -> RateLimit {
        RateLimit {
            window: window.into(),
            status: "allowed_warning".into(),
            utilization: Some(used),
            resets_at: Some(1_793_786_400_000),
            surpassed_threshold: Some(0.75),
        }
    }

    #[test]
    fn latest_limit_per_window_survives() {
        let s = SqliteStore::open_in_memory().unwrap();
        s.save_limit(&limit("seven_day", 0.8), 10).unwrap();
        s.save_limit(&limit("five_hour", 0.1), 10).unwrap();
        s.save_limit(&limit("seven_day", 0.83), 20).unwrap();
        // An older reading doesn't overwrite a newer one.
        s.save_limit(&limit("seven_day", 0.5), 15).unwrap();
        let got = s.limits().unwrap();
        assert_eq!(
            got,
            [
                (limit("five_hour", 0.1), 10),
                (limit("seven_day", 0.83), 20)
            ]
        );
        let mut unknown = limit("x", 0.0);
        unknown.utilization = None;
        unknown.resets_at = None;
        s.save_limit(&unknown, 30).unwrap();
        assert!(s.limits().unwrap().contains(&(unknown, 30)));
    }

    #[test]
    fn environments_round_trip_newest_first() {
        let s = SqliteStore::open_in_memory().unwrap();
        let env = |id: &str, version: &str| Environment {
            session_id: id.into(),
            claude_code_version: Some(version.into()),
            mcp_servers: vec![McpServer {
                name: "docs".into(),
                status: "connected".into(),
            }],
            tools: vec!["Read".into()],
            ..Environment::default()
        };
        s.save_environment(&env("a", "1"), 1).unwrap();
        s.save_environment(&env("b", "1"), 2).unwrap();
        s.save_environment(&env("a", "2"), 3).unwrap();
        let got = s.environments(10).unwrap();
        assert_eq!(got, [(env("a", "2"), 3), (env("b", "1"), 2)]);
        assert_eq!(s.environments(1).unwrap().len(), 1);
    }
}
