//! Which JEV providers the user agreed to.

use rusqlite::{OptionalExtension, params};
use swarmux_application::AppResult;
use swarmux_application::ports::ConsentStore;
use swarmux_domain::Millis;

use super::{OrStorage, SqliteStore};

impl ConsentStore for SqliteStore {
    fn has_consent(&self, provider: &str) -> AppResult<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT granted_at FROM jev_consent WHERE provider = ?1",
                [provider],
                |r| r.get(0),
            )
            .optional()
            .or_storage()?;
        Ok(found.is_some())
    }

    fn grant_consent(&self, provider: &str, now: Millis) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO jev_consent (provider, granted_at) VALUES (?1, ?2)
                 ON CONFLICT(provider) DO UPDATE SET granted_at = excluded.granted_at",
                params![provider, now],
            )
            .or_storage()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_is_per_provider_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.db");
        let s = SqliteStore::open(&path).unwrap();
        assert!(!s.has_consent("typesafe").unwrap());
        s.grant_consent("typesafe", 5).unwrap();
        s.grant_consent("typesafe", 6).unwrap();
        assert!(s.has_consent("typesafe").unwrap());
        assert!(!s.has_consent("openrouter").unwrap());
        drop(s);
        assert!(
            SqliteStore::open(&path)
                .unwrap()
                .has_consent("typesafe")
                .unwrap()
        );
    }
}
