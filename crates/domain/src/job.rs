use std::fmt;

use crate::{DomainError, Millis, PromptId, SessionId};

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
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Interrupted => "interrupted",
            JobStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Result<JobStatus, DomainError> {
        Ok(match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "failed" => JobStatus::Failed,
            "interrupted" => JobStatus::Interrupted,
            "cancelled" => JobStatus::Cancelled,
            other => return Err(DomainError::UnknownJobStatus(other.to_string())),
        })
    }

    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobStatus::Done | JobStatus::Failed | JobStatus::Interrupted | JobStatus::Cancelled
        )
    }
}

/// One prompt sent (or waiting to be sent) to one session.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: JobId,
    pub session_id: SessionId,
    /// The template the prompt was rendered from, if any.
    pub prompt_id: Option<PromptId>,
    pub prompt: String,
    pub model: String,
    /// Higher runs first.
    pub priority: i32,
    /// Branch the Claude session into a new one instead of continuing it.
    pub fork: bool,
    pub status: JobStatus,
    pub created_at: Millis,
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
    /// The final answer text, empty when there is none.
    pub text: String,
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
            JobStatus::Cancelled,
        ] {
            assert_eq!(JobStatus::parse(s.as_str()), Ok(s));
        }
        assert!(JobStatus::parse("nope").is_err());
    }

    #[test]
    fn finished_states() {
        assert!(!JobStatus::Running.is_finished());
        assert!(!JobStatus::Queued.is_finished());
        assert!(JobStatus::Interrupted.is_finished());
        assert!(JobStatus::Cancelled.is_finished());
    }
}
