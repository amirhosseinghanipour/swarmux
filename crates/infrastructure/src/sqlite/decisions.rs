//! The guard's decision log.

use rusqlite::{Row, params};
use swarmux_application::ports::DecisionRepository;
use swarmux_application::{AppResult, DecisionRecord, Outcome, Source};
use swarmux_domain::JobId;

use super::{OrStorage, SqliteStore};

/// Longest subject kept, in characters.
const SUBJECT_MAX: usize = 2000;

/// FNV-1a of tool and subject, to group identical calls.
fn input_hash(tool: &str, subject: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in tool.bytes().chain([0]).chain(subject.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn from_row(r: &Row) -> rusqlite::Result<DecisionRecord> {
    let output: String = r.get(3)?;
    let outcome = Outcome::parse(&output).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("unknown outcome {output:?}").into(),
        )
    })?;
    Ok(DecisionRecord {
        job_id: r.get::<_, Option<i64>>(0)?.map(JobId),
        tool: r.get(1)?,
        subject: r.get(2)?,
        outcome,
        rule: r.get(4)?,
        latency_us: r.get::<_, i64>(5)?.max(0) as u64,
        ts: r.get(6)?,
        // Unknown sources (from a newer swarmux) read as the guard's.
        source: Source::parse(&r.get::<_, String>(7)?).unwrap_or_default(),
        confidence: r.get(8)?,
        extra: swarmux_application::DecisionExtra::default(),
    })
}

impl DecisionRepository for SqliteStore {
    fn record(&self, d: &DecisionRecord) -> AppResult<()> {
        let subject: String = d.subject.chars().take(SUBJECT_MAX).collect();
        self.conn
            .execute(
                // A job id that isn't stored (another process's job) is kept as NULL.
                "INSERT INTO decisions
                   (job_id, fork, input_hash, output, rule, latency_us, ts, tool, subject,
                    confidence)
                 VALUES ((SELECT id FROM jobs WHERE id = ?1), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    ?10)",
                params![
                    d.job_id.map(|j| j.0),
                    d.source.as_str(),
                    input_hash(&d.tool, &d.subject),
                    d.outcome.as_str(),
                    d.rule,
                    i64::try_from(d.latency_us).unwrap_or(i64::MAX),
                    d.ts,
                    d.tool,
                    subject,
                    d.confidence,
                ],
            )
            .or_storage()?;
        Ok(())
    }

    fn recent_decisions(&self, limit: usize) -> AppResult<Vec<DecisionRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT job_id, tool, subject, output, rule, latency_us, ts, fork, confidence
                 FROM decisions
                 ORDER BY ts DESC, id DESC LIMIT ?1",
            )
            .or_storage()?;
        let rows = stmt
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], from_row)
            .or_storage()?;
        rows.collect::<Result<_, _>>().or_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::new_job;
    use swarmux_application::ports::{JobRepository, SessionRepository};
    use swarmux_domain::{Session, SessionId};

    fn record(job: Option<JobId>, outcome: Outcome, ts: i64) -> DecisionRecord {
        DecisionRecord {
            job_id: job,
            tool: "Bash".into(),
            subject: "cargo test".into(),
            outcome,
            rule: Some("Bash(cargo test:*)".into()),
            latency_us: 42,
            ts,
            source: Source::Guard,
            confidence: None,
            extra: swarmux_application::DecisionExtra::default(),
        }
    }

    #[test]
    fn decisions_round_trip_newest_first() {
        let s = SqliteStore::open_in_memory().unwrap();
        let sess = Session::new(SessionId::random(), "a", "/w".into(), "sonnet", 1).unwrap();
        SessionRepository::insert(&s, &sess).unwrap();
        let job = JobRepository::insert(&s, &new_job(sess.id, "p"), 2).unwrap();
        s.record(&record(Some(job), Outcome::Allow, 10)).unwrap();
        let stop = DecisionRecord {
            source: Source::Loop,
            ..record(Some(JobId(999)), Outcome::Stop, 20)
        };
        s.record(&stop).unwrap();
        let rows = s.recent_decisions(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].outcome, rows[0].source),
            (Outcome::Stop, Source::Loop)
        );
        assert_eq!(rows[0].job_id, None, "unknown jobs are kept as NULL");
        assert_eq!(rows[1], record(Some(job), Outcome::Allow, 10));
        assert_eq!(s.recent_decisions(1).unwrap().len(), 1);
    }

    #[test]
    fn records_made_by_the_removed_jev_keep_source_and_confidence() {
        let s = SqliteStore::open_in_memory().unwrap();
        let jev = DecisionRecord {
            source: Source::Jev,
            confidence: Some(0.91),
            ..record(None, Outcome::Allow, 5)
        };
        let route = DecisionRecord {
            tool: "route".into(),
            source: Source::Haiku,
            ..record(None, Outcome::Route, 6)
        };
        s.record(&jev).unwrap();
        s.record(&route).unwrap();
        assert_eq!(s.recent_decisions(10).unwrap(), [route, jev]);
    }

    #[test]
    fn rows_an_older_swarmux_wrote_for_jev_still_read() {
        // As the removed code wrote them: the source in `fork`, the model's
        // confidence beside it, a rule text naming JEV.
        let s = SqliteStore::open_in_memory().unwrap();
        s.conn
            .execute_batch(
                "INSERT INTO decisions (fork, input_hash, output, rule, latency_us, ts, tool, subject, confidence)
                   VALUES ('jev', 'h', 'allow', 'jev 0.93', 40000, 10, 'Bash', 'make deploy', 0.93);
                 INSERT INTO decisions (fork, input_hash, output, rule, latency_us, ts, tool, subject, confidence)
                   VALUES ('jev-error', 'h', 'ask', 'jev: HTTP 401; asking you', 9000, 11, 'Bash', 'make', NULL);",
            )
            .unwrap();
        let rows = s.recent_decisions(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].source, rows[0].outcome, rows[0].confidence),
            (Source::JevError, Outcome::Ask, None)
        );
        assert_eq!(
            (rows[1].source, rows[1].outcome, rows[1].confidence),
            (Source::Jev, Outcome::Allow, Some(0.93))
        );
        assert_eq!(rows[1].rule.as_deref(), Some("jev 0.93"));
    }

    #[test]
    fn hash_is_stable_and_separates_fields() {
        assert_eq!(input_hash("Bash", "ls"), input_hash("Bash", "ls"));
        assert_ne!(input_hash("Bas", "hls"), input_hash("Bash", "ls"));
        assert_eq!(input_hash("", "").len(), 16);
    }
}
