//! Allow rules the user added while swarmux ran: where they apply, who
//! made them, when they stop, and which calls may be allowed "always" at
//! all.

use std::fmt;

use super::glob::{is_windows, normalize};
use super::shell::{has_substitution, split_commands};
use super::{Pattern, needs_typed_confirmation};
use crate::Millis;
use crate::ledger::{hex, sha256};

/// One day, in milliseconds.
pub const DAY_MS: Millis = 86_400_000;
/// How long an "always allow" lasts unless the settings say otherwise.
pub const DEFAULT_EXPIRY_DAYS: u32 = 30;

/// Where a rule applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RuleScope {
    /// Jobs of one repository (the default).
    #[default]
    Repo,
    /// Jobs of one workspace.
    Workspace,
    /// Everywhere.
    Global,
}

impl RuleScope {
    pub const ALL: [RuleScope; 3] = [RuleScope::Repo, RuleScope::Workspace, RuleScope::Global];

    pub fn as_str(self) -> &'static str {
        match self {
            RuleScope::Repo => "repo",
            RuleScope::Workspace => "workspace",
            RuleScope::Global => "global",
        }
    }

    pub fn parse(s: &str) -> Option<RuleScope> {
        RuleScope::ALL.into_iter().find(|x| x.as_str() == s.trim())
    }
}

/// How a rule came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuleSource {
    /// The user pressed "always allow" on a held call.
    AlwaysAllow,
    /// The user took a rule swarmux offered after many identical approvals.
    Suggestion,
    /// Written in `config.toml` by hand.
    Config,
}

impl RuleSource {
    pub const ALL: [RuleSource; 3] = [
        RuleSource::AlwaysAllow,
        RuleSource::Suggestion,
        RuleSource::Config,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RuleSource::AlwaysAllow => "always_allow",
            RuleSource::Suggestion => "suggestion",
            RuleSource::Config => "config",
        }
    }

    pub fn parse(s: &str) -> Option<RuleSource> {
        RuleSource::ALL.into_iter().find(|x| x.as_str() == s.trim())
    }
}

/// An allow rule with where it applies and how long it lasts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRule {
    /// Short and stable: what `swarmux guard rules remove` takes.
    pub id: String,
    /// The rule in Claude Code's syntax.
    pub text: String,
    pub scope: RuleScope,
    /// The repository or workspace folder; `None` for a global rule.
    pub root: Option<String>,
    pub source: RuleSource,
    pub created_at: Millis,
    /// `None`: never.
    pub expires_at: Option<Millis>,
}

impl StoredRule {
    pub fn new(
        text: &str,
        scope: RuleScope,
        root: Option<&str>,
        source: RuleSource,
        created_at: Millis,
        expires_at: Option<Millis>,
    ) -> StoredRule {
        let root = match scope {
            RuleScope::Global => None,
            _ => root.map(str::to_string),
        };
        let key = format!(
            "{text}\0{}\0{}\0{created_at}",
            scope.as_str(),
            root.as_deref().unwrap_or("")
        );
        let id = hex(&sha256(key.as_bytes()))[..8].to_string();
        StoredRule {
            id,
            text: text.to_string(),
            scope,
            root,
            source,
            created_at,
            expires_at,
        }
    }

    pub fn expired(&self, now: Millis) -> bool {
        self.expires_at.is_some_and(|at| now >= at)
    }

    /// Whether the rule applies to a call made in the repository
    /// `repo_root` of the workspace `workspace_root` (either may be
    /// unknown, and then a scoped rule does not apply).
    pub fn applies_to(&self, repo_root: Option<&str>, workspace_root: Option<&str>) -> bool {
        let same = |a: &str, b: Option<&str>| b.is_some_and(|b| same_folder(a, b));
        match (self.scope, self.root.as_deref()) {
            (RuleScope::Global, _) => true,
            (RuleScope::Repo, Some(root)) => same(root, repo_root),
            (RuleScope::Workspace, Some(root)) => same(root, workspace_root),
            _ => false,
        }
    }

    /// The expiry `days` from `now`; `None` (never) for no days.
    pub fn expiry_after(now: Millis, days: Option<u32>) -> Option<Millis> {
        days.map(|d| now.saturating_add(Millis::from(d).saturating_mul(DAY_MS)))
    }
}

/// Whether two folders are the same one, ignoring slash direction, a
/// trailing slash and (for drive paths) case.
pub fn same_folder(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let n = normalize(s).unwrap_or_else(|| s.replace('\\', "/"));
        let n = n.trim_end_matches('/').to_string();
        if is_windows(&n) {
            n.to_ascii_lowercase()
        } else {
            n
        }
    };
    norm(a) == norm(b)
}

/// Why a call can't be "always allowed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlwaysRefusal {
    /// It can't be taken back: only a typed confirmation, once.
    Irreversible,
    /// A shell line of several commands (or with substitution): a rule for
    /// the whole line would never match again, and one per command would
    /// allow more than was shown.
    Compound,
    /// The rule would be the bare tool name: the whole tool, forever.
    BareTool(String),
}

impl fmt::Display for AlwaysRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AlwaysRefusal::Irreversible => f.write_str(
                "irreversible calls are never always-allowed: confirm each one by naming its target",
            ),
            AlwaysRefusal::Compound => f.write_str(
                "this line runs several commands; allow it once, or add a rule per command to config.toml",
            ),
            AlwaysRefusal::BareTool(tool) => write!(
                f,
                "a rule for this call would allow every use of {tool}; allow it once, or write the rule you want in config.toml"
            ),
        }
    }
}

/// The narrowest rule that allows exactly this call again, or why there
/// is none.
pub fn always_rule(tool: &str, subject: &str, cwd: &str) -> Result<Pattern, AlwaysRefusal> {
    if needs_typed_confirmation(tool, subject, cwd) {
        return Err(AlwaysRefusal::Irreversible);
    }
    if matches!(tool, "Bash" | "PowerShell")
        && (has_substitution(subject) || split_commands(subject).len() > 1)
    {
        return Err(AlwaysRefusal::Compound);
    }
    let rule = Pattern::exact_for(tool, subject, cwd);
    if rule.is_bare() {
        return Err(AlwaysRefusal::BareTool(tool.to_string()));
    }
    Ok(rule)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{GuardPolicy, Unmatched, Verdict};

    fn rule(scope: RuleScope, root: Option<&str>) -> StoredRule {
        StoredRule::new(
            "Bash(make)",
            scope,
            root,
            RuleSource::AlwaysAllow,
            1_000,
            Some(5_000),
        )
    }

    #[test]
    fn ids_are_short_stable_and_differ_by_scope() {
        let a = rule(RuleScope::Repo, Some("/r"));
        assert_eq!(a.id.len(), 8);
        assert_eq!(a, rule(RuleScope::Repo, Some("/r")));
        assert_ne!(a.id, rule(RuleScope::Repo, Some("/other")).id);
        assert_ne!(a.id, rule(RuleScope::Global, None).id);
    }

    #[test]
    fn rules_stop_at_their_expiry() {
        let r = rule(RuleScope::Repo, Some("/r"));
        assert!(!r.expired(4_999));
        assert!(r.expired(5_000));
        let never = StoredRule {
            expires_at: None,
            ..r
        };
        assert!(!never.expired(i64::MAX));
        assert_eq!(StoredRule::expiry_after(10, Some(1)), Some(10 + DAY_MS));
        assert_eq!(StoredRule::expiry_after(10, None), None);
    }

    #[test]
    fn a_rule_applies_only_where_it_was_made() {
        let repo = rule(RuleScope::Repo, Some("/home/me/app"));
        assert!(repo.applies_to(Some("/home/me/app"), None));
        assert!(repo.applies_to(Some("/home/me/app/"), None));
        assert!(!repo.applies_to(Some("/home/me/other"), None));
        assert!(!repo.applies_to(None, Some("/home/me/app")));
        let ws = rule(RuleScope::Workspace, Some("/home/me/ws"));
        assert!(ws.applies_to(Some("/anything"), Some("/home/me/ws")));
        assert!(!ws.applies_to(Some("/home/me/ws"), None));
        let global = rule(RuleScope::Global, Some("/ignored"));
        assert_eq!(global.root, None);
        assert!(global.applies_to(None, None));
        assert!(rule(RuleScope::Repo, Some(r"C:\Proj")).applies_to(Some("c:/proj"), None));
        let no_root = rule(RuleScope::Repo, None);
        assert!(
            !no_root.applies_to(Some("/r"), Some("/r")),
            "no root, no match"
        );
    }

    #[test]
    fn scope_and_source_round_trip() {
        for s in RuleScope::ALL {
            assert_eq!(RuleScope::parse(s.as_str()), Some(s));
        }
        for s in RuleSource::ALL {
            assert_eq!(RuleSource::parse(s.as_str()), Some(s));
        }
        assert_eq!(RuleScope::parse("galaxy"), None);
    }

    #[test]
    fn the_bare_tool_name_is_refused() {
        for (tool, subject) in [
            ("mcp__github__create_issue", ""),
            ("mcp__github__create_issue", "x"),
            ("WebSearch", "rust"),
            ("SomeNewTool", "x"),
            ("WebFetch", ""),
        ] {
            assert_eq!(
                always_rule(tool, subject, "/w"),
                Err(AlwaysRefusal::BareTool(tool.to_string())),
                "{tool}"
            );
        }
        assert_eq!(
            always_rule("Bash", "cargo test", "/w").unwrap().to_string(),
            "Bash(cargo test)"
        );
        assert_eq!(
            always_rule("WebFetch", "https://docs.rs/x", "/w")
                .unwrap()
                .to_string(),
            "WebFetch(domain:docs.rs)"
        );
        assert_eq!(
            always_rule("Edit", "/w/src/a.rs", "/w")
                .unwrap()
                .to_string(),
            "Edit(/src/a.rs)"
        );
    }

    #[test]
    fn compound_lines_are_refused_and_would_never_have_matched() {
        for line in [
            "cargo build && cargo test",
            "cd x; make",
            "make | tee log",
            "echo $(date)",
            "make `whoami`",
        ] {
            assert_eq!(
                always_rule("Bash", line, "/w"),
                Err(AlwaysRefusal::Compound),
                "{line}"
            );
        }
        // The bug this replaces: the whole line as one exact rule never
        // matches the same line again, because lines are matched per command.
        let whole = Pattern::exact_for("Bash", "cargo build && cargo test", "/w");
        let mut policy = GuardPolicy::new(vec![], vec![], Unmatched::Ask);
        policy.add_allow(whole);
        assert_eq!(
            policy
                .decide("Bash", "cargo build && cargo test", "/w")
                .verdict,
            Verdict::Ask
        );
    }

    #[test]
    fn irreversible_calls_are_refused() {
        assert_eq!(
            always_rule("Bash", "rm -rf target", "/w"),
            Err(AlwaysRefusal::Irreversible)
        );
        assert_eq!(
            always_rule("Bash", "git push --force", "/w"),
            Err(AlwaysRefusal::Irreversible)
        );
    }

    #[test]
    fn refusals_say_what_to_do() {
        for r in [
            AlwaysRefusal::Irreversible,
            AlwaysRefusal::Compound,
            AlwaysRefusal::BareTool("X".into()),
        ] {
            assert!(!r.to_string().is_empty());
        }
    }
}
