//! The Claude Code lifecycle events swarmux listens to through hooks.

/// A hook event, named exactly as Claude Code names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    Stop,
    SubagentStop,
    SessionStart,
    PreCompact,
    Notification,
    PermissionDenied,
    StopFailure,
}

impl HookEvent {
    pub const ALL: [HookEvent; 10] = [
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PostToolUseFailure,
        HookEvent::Stop,
        HookEvent::SubagentStop,
        HookEvent::SessionStart,
        HookEvent::PreCompact,
        HookEvent::Notification,
        HookEvent::PermissionDenied,
        HookEvent::StopFailure,
    ];

    /// Claude Code's name, as it appears in settings and hook input.
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::Notification => "Notification",
            HookEvent::PermissionDenied => "PermissionDenied",
            HookEvent::StopFailure => "StopFailure",
        }
    }

    /// The name used on the command line: `swarmux hook post-tool-use`.
    pub fn kebab(self) -> String {
        let mut out = String::new();
        for (i, c) in self.as_str().chars().enumerate() {
            if c.is_ascii_uppercase() {
                if i > 0 {
                    out.push('-');
                }
                out.push(c.to_ascii_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Reads the exact name, the kebab-case name, or `pre-tool` (the name
    /// the first hook used). Unknown names give `None`.
    pub fn parse(name: &str) -> Option<HookEvent> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("pre-tool") {
            return Some(HookEvent::PreToolUse);
        }
        HookEvent::ALL
            .into_iter()
            .find(|e| e.as_str() == name || e.kebab() == name)
    }

    /// Events whose answer Claude Code waits for. The others are registered
    /// asynchronous and never delay the agent.
    pub fn is_blocking(self) -> bool {
        matches!(
            self,
            HookEvent::PreToolUse
                | HookEvent::Stop
                | HookEvent::SubagentStop
                | HookEvent::SessionStart
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_round_trips_by_both_names() {
        for e in HookEvent::ALL {
            assert_eq!(HookEvent::parse(e.as_str()), Some(e));
            assert_eq!(HookEvent::parse(&e.kebab()), Some(e));
        }
        assert_eq!(
            HookEvent::PostToolUseFailure.kebab(),
            "post-tool-use-failure"
        );
    }

    #[test]
    fn the_first_hooks_name_still_works() {
        assert_eq!(HookEvent::parse("pre-tool"), Some(HookEvent::PreToolUse));
    }

    #[test]
    fn unknown_names_are_none() {
        assert_eq!(HookEvent::parse("UserPromptSubmit"), None);
        assert_eq!(HookEvent::parse(""), None);
    }

    #[test]
    fn blocking_events() {
        let blocking: Vec<_> = HookEvent::ALL
            .into_iter()
            .filter(|e| e.is_blocking())
            .collect();
        assert_eq!(
            blocking,
            [
                HookEvent::PreToolUse,
                HookEvent::Stop,
                HookEvent::SubagentStop,
                HookEvent::SessionStart
            ]
        );
    }
}
