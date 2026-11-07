//! Enforcers: what makes a rule more than text.
//!
//! A guard rule is a deterministic matcher on tool calls. Rules allow by
//! default and only ever forbid (deny, ask, warn); the strictest match
//! wins, and a rule that can't be evaluated forbids (fails closed). The
//! Guard's own allow and deny rules are not changed by them.

use regex::Regex;

use crate::gate::GateCheck;
use crate::guard::glob::glob_match;
use crate::mission::check_in;

use super::ItemId;

/// Longest content pattern, in characters.
pub const PATTERN_MAX: usize = 500;

/// How a rule is enforced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enforcer {
    /// The Guard, on every tool call.
    Guard(GuardRule),
    /// A hidden check the done-gate runs for missions in scope.
    Gate(CheckRule),
    /// A visible command the done-gate runs (an analyzer, a linter).
    Lint(CheckRule),
    /// Advisory text only.
    Prompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EnforcerKind {
    Guard,
    Gate,
    Lint,
    Prompt,
}

impl EnforcerKind {
    pub const ALL: [EnforcerKind; 4] = [
        EnforcerKind::Guard,
        EnforcerKind::Gate,
        EnforcerKind::Lint,
        EnforcerKind::Prompt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EnforcerKind::Guard => "guard",
            EnforcerKind::Gate => "gate",
            EnforcerKind::Lint => "lint",
            EnforcerKind::Prompt => "prompt",
        }
    }

    pub fn parse(s: &str) -> Option<EnforcerKind> {
        EnforcerKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

impl Enforcer {
    pub fn kind(&self) -> EnforcerKind {
        match self {
            Enforcer::Guard(_) => EnforcerKind::Guard,
            Enforcer::Gate(_) => EnforcerKind::Gate,
            Enforcer::Lint(_) => EnforcerKind::Lint,
            Enforcer::Prompt => EnforcerKind::Prompt,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Enforcer::Guard(g) => g.validate(),
            Enforcer::Gate(c) | Enforcer::Lint(c) => c.validate(),
            Enforcer::Prompt => Ok(()),
        }
    }
}

/// What a guard rule does to a call it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Refused, with the rule as the reason.
    Deny,
    /// Held for the owner (a P1 approval that shows the rule).
    Ask,
    /// Let through, with the rule added to the agent's context.
    Warn,
}

impl Action {
    pub const ALL: [Action; 3] = [Action::Deny, Action::Ask, Action::Warn];

    pub fn as_str(self) -> &'static str {
        match self {
            Action::Deny => "deny",
            Action::Ask => "ask",
            Action::Warn => "warn",
        }
    }

    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Smaller is stricter.
    fn strictness(self) -> u8 {
        match self {
            Action::Deny => 0,
            Action::Ask => 1,
            Action::Warn => 2,
        }
    }
}

/// Which tool calls a guard rule matches. Empty lists match everything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardRule {
    /// Tool names; `Edit` also covers `Write`, `MultiEdit` and `NotebookEdit`.
    pub tools: Vec<String>,
    /// Globs relative to the job's root (the workspace's layout).
    pub paths: Vec<String>,
    /// A regular expression over the command, or the text being written.
    pub pattern: Option<String>,
    pub action: Action,
    /// What the agent is told besides the rule; may be empty.
    pub message: String,
}

impl GuardRule {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(p) = &self.pattern {
            if p.chars().count() > PATTERN_MAX {
                return Err(format!(
                    "the pattern is longer than {PATTERN_MAX} characters"
                ));
            }
            Regex::new(p).map_err(|e| format!("the pattern is not a regular expression: {e}"))?;
        }
        if self.tools.iter().any(|t| t.trim().is_empty()) {
            return Err("a tool name is empty".into());
        }
        Ok(())
    }

    /// The matcher, compiled once.
    pub fn compile(&self) -> CompiledGuard {
        let regex = match &self.pattern {
            None => Ok(None),
            Some(p) => Regex::new(p).map(Some).map_err(|e| e.to_string()),
        };
        CompiledGuard {
            rule: self.clone(),
            regex,
        }
    }
}

/// One tool call, as rules see it.
#[derive(Clone, Copy, Debug)]
pub struct ToolCall<'a> {
    pub tool: &'a str,
    /// The file it acts on, relative to the job's root; `None` for calls
    /// without one, or outside the root.
    pub path: Option<&'a str>,
    /// The command, or the text being written.
    pub text: &'a str,
}

/// A guard rule ready to match.
#[derive(Clone, Debug)]
pub struct CompiledGuard {
    pub rule: GuardRule,
    regex: Result<Option<Regex>, String>,
}

/// Whether `rule` (a rule's tool name) covers the tool `tool`.
fn covers(rule: &str, tool: &str) -> bool {
    rule == tool
        || (rule == "Edit" && matches!(tool, "Write" | "MultiEdit" | "NotebookEdit"))
        || (rule == "Bash" && tool == "PowerShell")
}

impl CompiledGuard {
    /// What the rule does to `call`, if it matches. A pattern that can't
    /// be evaluated denies every call it would have looked at.
    pub fn hit(&self, call: &ToolCall<'_>, fold: bool) -> Option<Action> {
        let r = &self.rule;
        if !r.tools.is_empty() && !r.tools.iter().any(|t| covers(t, call.tool)) {
            return None;
        }
        if !r.paths.is_empty() {
            let path = call.path?;
            if !r.paths.iter().any(|g| glob_match(g, path, fold)) {
                return None;
            }
        }
        match &self.regex {
            Err(_) => Some(Action::Deny),
            Ok(Some(re)) if !re.is_match(call.text) => None,
            Ok(_) => Some(r.action),
        }
    }

    /// Why it can't be evaluated, when it can't.
    pub fn broken(&self) -> Option<&str> {
        self.regex.as_ref().err().map(String::as_str)
    }
}

/// The strictest of `hits`; the lowest id among equals.
pub fn strictest(hits: &[(ItemId, Action)]) -> Option<&(ItemId, Action)> {
    hits.iter()
        .min_by(|a, b| a.1.strictness().cmp(&b.1.strictness()).then(a.0.cmp(&b.0)))
}

/// A check command bound to a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRule {
    /// A command line, run without a shell; `repo: cmd` runs in that
    /// repository of a workspace.
    pub command: String,
}

impl CheckRule {
    pub fn validate(&self) -> Result<(), String> {
        crate::mission::check_of(&self.command)
            .map(drop)
            .ok_or_else(|| "the check command is empty".to_string())
    }

    /// The check for rule `id`, its `repo:` prefix resolved by `folder`.
    /// A hidden check's name says nothing about what it runs.
    pub fn check(
        &self,
        id: &ItemId,
        hidden: bool,
        folder: &dyn Fn(&str) -> Option<String>,
    ) -> Option<GateCheck> {
        let mut check = check_in(&self.command, folder)?;
        check.name = if hidden {
            format!("rule {id}")
        } else {
            format!("rule {id}: {}", check.name)
        };
        Some(check)
    }
}
