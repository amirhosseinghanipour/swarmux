use swarmux_domain::{DomainError, JobId};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// A storage adapter failed; the message comes from the adapter.
    #[error("storage error: {0}")]
    Storage(String),
    #[error("session not found")]
    SessionNotFound,
    /// The workspace adapter (git) failed.
    #[error("workspace error: {0}")]
    Workspace(String),
    #[error("prompt not found")]
    PromptNotFound,
    #[error("job not found")]
    JobNotFound,
    #[error("job #{0} has already finished")]
    JobFinished(JobId),
    #[error("job #{0} hasn't finished yet")]
    JobNotFinished(JobId),
    #[error("job #{0} is no longer queued")]
    JobNotQueued(JobId),
    #[error("job #{0} did not stop early, so there is nothing to resume")]
    NotResumable(JobId),
    #[error("that approval is no longer pending")]
    ApprovalNotFound,
    /// The settings file couldn't be changed.
    #[error("config error: {0}")]
    Config(String),
    #[error("mission {0} not found")]
    MissionNotFound(String),
    /// The mission can't take that step now.
    #[error("{0}")]
    Mission(String),
}

pub type AppResult<T> = Result<T, AppError>;
