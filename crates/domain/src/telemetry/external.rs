//! Claude Code sessions swarmux didn't start, as `claude agents` lists them.

use crate::Millis;

/// What the session is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalState {
    Working,
    /// Waiting for the user (a permission prompt, input, a dialog).
    Blocked,
    Idle,
    Done,
    Failed,
    Stopped,
    Unknown,
}

impl ExternalState {
    /// Reads a state name leniently; Claude Code uses `state` for
    /// background sessions and `status` for interactive ones.
    pub fn parse(name: &str) -> ExternalState {
        match name.trim().to_ascii_lowercase().as_str() {
            "working" | "busy" | "running" => ExternalState::Working,
            "blocked" | "waiting" | "requires_action" => ExternalState::Blocked,
            "idle" => ExternalState::Idle,
            "done" | "completed" => ExternalState::Done,
            "failed" | "error" => ExternalState::Failed,
            "stopped" | "cancelled" | "canceled" => ExternalState::Stopped,
            _ => ExternalState::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ExternalState::Working => "working",
            ExternalState::Blocked => "blocked",
            ExternalState::Idle => "idle",
            ExternalState::Done => "done",
            ExternalState::Failed => "failed",
            ExternalState::Stopped => "stopped",
            ExternalState::Unknown => "unknown",
        }
    }
}

/// `interactive` (a terminal) or `background`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalKind {
    Interactive,
    Background,
    Other,
}

/// One session found by `claude agents --json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalSession {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    pub kind: ExternalKind,
    pub pid: Option<u32>,
    pub state: ExternalState,
    /// Why it is blocked: a permission prompt, input needed, a dialog, …
    pub waiting_for: Option<String>,
    pub started_at: Option<Millis>,
}

impl ExternalSession {
    /// Needs the user: blocked, or says what it waits for.
    pub fn is_waiting(&self) -> bool {
        self.state == ExternalState::Blocked || self.waiting_for.is_some()
    }

    /// Still running (not finished, failed or stopped).
    pub fn is_live(&self) -> bool {
        !matches!(
            self.state,
            ExternalState::Done | ExternalState::Failed | ExternalState::Stopped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_read_from_either_vocabulary() {
        assert_eq!(ExternalState::parse("busy"), ExternalState::Working);
        assert_eq!(ExternalState::parse("Blocked"), ExternalState::Blocked);
        assert_eq!(ExternalState::parse("done"), ExternalState::Done);
        assert_eq!(ExternalState::parse("???"), ExternalState::Unknown);
    }

    #[test]
    fn waiting_means_blocked_or_a_stated_reason() {
        let mut s = ExternalSession {
            session_id: "x".into(),
            name: String::new(),
            cwd: String::new(),
            kind: ExternalKind::Background,
            pid: None,
            state: ExternalState::Working,
            waiting_for: None,
            started_at: None,
        };
        assert!(!s.is_waiting() && s.is_live());
        s.waiting_for = Some("permission prompt".into());
        assert!(s.is_waiting());
        s.waiting_for = None;
        s.state = ExternalState::Blocked;
        assert!(s.is_waiting());
        s.state = ExternalState::Done;
        assert!(!s.is_live());
    }
}
