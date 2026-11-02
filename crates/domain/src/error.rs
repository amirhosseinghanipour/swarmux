#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("session name can't be empty")]
    EmptySessionName,
    #[error("session name is longer than {max} characters")]
    SessionNameTooLong { max: usize },
    #[error("prompt can't be empty")]
    EmptyPrompt,
    #[error("unknown job status {0:?}")]
    UnknownJobStatus(String),
}
