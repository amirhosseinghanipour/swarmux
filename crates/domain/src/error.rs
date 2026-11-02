#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("session name can't be empty")]
    EmptySessionName,
    #[error("session name is longer than {max} characters")]
    SessionNameTooLong { max: usize },
    #[error("prompt can't be empty")]
    EmptyPrompt,
    #[error("prompt name can't be empty")]
    EmptyPromptName,
    #[error("prompt name is longer than {max} characters")]
    PromptNameTooLong { max: usize },
    #[error("prompt body can't be empty")]
    EmptyPromptBody,
    #[error("no value for {{{{{0}}}}}")]
    MissingVariable(String),
    #[error("unknown job status {0:?}")]
    UnknownJobStatus(String),
    #[error("invalid rule {pattern:?}: {reason}")]
    InvalidPattern { pattern: String, reason: String },
    #[error("invalid stored review: {0}")]
    InvalidReview(String),
}
