//! Jobs a usage limit stopped, waiting for their window to reset.

use rusqlite::params;
use swarmux_application::{AppResult, PendingResume, ResumeBook};
use swarmux_domain::JobId;

use super::{OrStorage, SqliteStore};

impl ResumeBook for SqliteStore {
    fn save(&self, resume: &PendingResume) -> AppResult<()> {
        self.conn
            .execute(
                "INSERT INTO pending_resumes (job_id, resume_at, window) VALUES (?1, ?2, ?3)
                 ON CONFLICT(job_id) DO UPDATE SET
                   resume_at = excluded.resume_at, window = excluded.window",
                params![resume.job.0, resume.at, resume.window],
            )
            .or_storage()?;
        Ok(())
    }

    fn remove(&self, job: JobId) -> AppResult<()> {
        self.conn
            .execute("DELETE FROM pending_resumes WHERE job_id = ?1", [job.0])
            .or_storage()?;
        Ok(())
    }

    fn pending(&self) -> AppResult<Vec<PendingResume>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT job_id, resume_at, window FROM pending_resumes
                 ORDER BY resume_at, job_id",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PendingResume {
                    job: JobId(r.get(0)?),
                    at: r.get(1)?,
                    window: r.get(2)?,
                })
            })
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resume(job: i64, at: i64, window: &str) -> PendingResume {
        PendingResume {
            job: JobId(job),
            at,
            window: window.into(),
        }
    }

    #[test]
    fn resumes_round_trip_and_replace() {
        let s = SqliteStore::open_in_memory().unwrap();
        assert!(s.pending().unwrap().is_empty());
        s.save(&resume(2, 200, "7d")).unwrap();
        s.save(&resume(1, 100, "5h")).unwrap();
        s.save(&resume(2, 150, "5h")).unwrap();
        assert_eq!(
            s.pending().unwrap(),
            [resume(1, 100, "5h"), resume(2, 150, "5h")]
        );
        s.remove(JobId(1)).unwrap();
        s.remove(JobId(9)).unwrap();
        assert_eq!(s.pending().unwrap(), [resume(2, 150, "5h")]);
    }

    #[test]
    fn resumes_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.db");
        SqliteStore::open(&path)
            .unwrap()
            .save(&resume(3, 300, "5h"))
            .unwrap();
        let again = SqliteStore::open(&path).unwrap();
        assert_eq!(again.pending().unwrap(), [resume(3, 300, "5h")]);
    }
}
