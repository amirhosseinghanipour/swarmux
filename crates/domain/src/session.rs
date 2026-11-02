use std::fmt;
use std::path::PathBuf;

use uuid::Uuid;

use crate::{DomainError, Millis};

pub const DEFAULT_MODEL: &str = "sonnet";
pub const DEFAULT_PERMISSION_MODE: &str = "default";
const NAME_MAX: usize = 64;

/// Also the Claude Code session id, so a swarmux session maps 1:1 to a Claude session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn random() -> SessionId {
        SessionId(Uuid::new_v4())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A named Claude Code conversation bound to a project folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub cwd: PathBuf,
    pub model: String,
    pub permission_mode: String,
    pub created_at: Millis,
    pub last_used_at: Option<Millis>,
    pub archived: bool,
}

impl Session {
    /// Validates the name (trimmed, non-empty, at most 64 characters).
    pub fn new(
        id: SessionId,
        name: &str,
        cwd: PathBuf,
        model: &str,
        now: Millis,
    ) -> Result<Session, DomainError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DomainError::EmptySessionName);
        }
        if name.chars().count() > NAME_MAX {
            return Err(DomainError::SessionNameTooLong { max: NAME_MAX });
        }
        Ok(Session {
            id,
            name: name.to_string(),
            cwd,
            model: model.to_string(),
            permission_mode: DEFAULT_PERMISSION_MODE.to_string(),
            created_at: now,
            last_used_at: None,
            archived: false,
        })
    }

    /// When the session was last active (used for ordering).
    pub fn activity(&self) -> Millis {
        self.last_used_at.unwrap_or(self.created_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(name: &str) -> Result<Session, DomainError> {
        Session::new(SessionId::random(), name, "/w".into(), DEFAULT_MODEL, 5)
    }

    #[test]
    fn name_is_trimmed_and_defaults_apply() {
        let s = make("  demo ").unwrap();
        assert_eq!(s.name, "demo");
        assert_eq!(s.permission_mode, "default");
        assert_eq!(s.activity(), 5);
    }

    #[test]
    fn empty_or_long_names_are_rejected() {
        assert_eq!(make("   "), Err(DomainError::EmptySessionName));
        assert_eq!(
            make(&"x".repeat(65)),
            Err(DomainError::SessionNameTooLong { max: 64 })
        );
    }
}
