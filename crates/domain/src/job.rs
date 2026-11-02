use std::fmt;

use crate::DomainError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub i64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Interrupted,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Result<JobStatus, DomainError> {
        Ok(match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "failed" => JobStatus::Failed,
            "interrupted" => JobStatus::Interrupted,
            other => return Err(DomainError::UnknownJobStatus(other.to_string())),
        })
    }

    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Failed | JobStatus::Interrupted
        )
    }
}

/// Final numbers of a job, as reported by Claude Code (or by swarmux on failure).
#[derive(Clone, Debug, PartialEq)]
pub struct JobResult {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub outcome: String,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        for s in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Done,
            JobStatus::Failed,
            JobStatus::Interrupted,
        ] {
            assert_eq!(JobStatus::parse(s.as_str()), Ok(s));
        }
        assert!(JobStatus::parse("nope").is_err());
    }

    #[test]
    fn finished_states() {
        assert!(!JobStatus::Running.is_finished());
        assert!(JobStatus::Interrupted.is_finished());
    }
}
