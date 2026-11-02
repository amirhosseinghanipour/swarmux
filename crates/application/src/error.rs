use swarmux_domain::DomainError;

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
    #[error("session {0} is still running a job")]
    SessionBusy(String),
}

pub type AppResult<T> = Result<T, AppError>;
