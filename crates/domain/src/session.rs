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

    /// Parses Claude Code's textual session id.
    pub fn parse(s: &str) -> Option<SessionId> {
        Uuid::parse_str(s.trim()).ok().map(SessionId)
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
    /// Run jobs without the user's own Claude Code settings (hooks, plugins).
    pub lean: bool,
    /// Run each job in its own git worktree. `None` follows the folder:
    /// on in a git repository, off elsewhere.
    pub worktree: Option<bool>,
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
            lean: false,
            worktree: None,
        })
    }

    /// Name for a session forked from this one, kept within the length limit.
    pub fn fork_name(&self) -> String {
        const SUFFIX: &str = " fork";
        let keep = NAME_MAX - SUFFIX.len();
        let base: String = self.name.chars().take(keep).collect();
        format!("{}{SUFFIX}", base.trim_end())
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
        assert!(!s.lean);
    }

    #[test]
    fn fork_name_fits_the_limit() {
        assert_eq!(make("demo").unwrap().fork_name(), "demo fork");
        let long = make(&"x".repeat(64)).unwrap().fork_name();
        assert_eq!(long.chars().count(), 64);
        assert!(long.ends_with(" fork"));
    }

    #[test]
    fn parse_session_id() {
        let id = SessionId::random();
        assert_eq!(SessionId::parse(&id.to_string()), Some(id));
        assert_eq!(SessionId::parse("nope"), None);
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
