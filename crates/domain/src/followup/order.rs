//! Standing orders: the owner's playbook, matched without a model.
//!
//! An order says "for follow-ups like this, do that": approve them
//! (`auto-approve`), always ask, or park them. The first order that matches
//! decides. Auto-approval is fenced: it never applies to questions,
//! decisions, risks or blockers unless the order says `allow_risky`, never
//! to urgent items, and never to a follow-up that names a protected path.

use super::{FollowKind, FollowUp};
use crate::guard::glob::{glob_match, is_windows, normalize};

/// What an order does with the follow-ups it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderAction {
    AutoApprove,
    /// Leave it for the owner, and let no later order approve it.
    AlwaysAsk,
    /// Hold it out of the briefs; it stays listed.
    Park,
}

impl OrderAction {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderAction::AutoApprove => "auto-approve",
            OrderAction::AlwaysAsk => "always-ask",
            OrderAction::Park => "park",
        }
    }

    pub fn parse(s: &str) -> Option<OrderAction> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto-approve" | "auto_approve" | "approve" => Some(OrderAction::AutoApprove),
            "always-ask" | "always_ask" | "ask" => Some(OrderAction::AlwaysAsk),
            "park" => Some(OrderAction::Park),
            _ => None,
        }
    }
}

/// One owner-written rule. Every condition that is set must hold.
#[derive(Clone, Debug, PartialEq)]
pub struct StandingOrder {
    pub id: String,
    /// Empty: any kind.
    pub kinds: Vec<FollowKind>,
    /// Globs on the repository or workspace name. Empty: any.
    pub repos: Vec<String>,
    /// Globs; when set, the follow-up must name files and all must match.
    pub paths: Vec<String>,
    /// Any of these words in the text (case-insensitive). Empty: any.
    pub keywords: Vec<String>,
    /// The follow-up's estimated cost must be known and at most this.
    pub max_cost_usd: Option<f64>,
    pub action: OrderAction,
    /// Lets `auto-approve` apply to decisions, risks and blockers.
    pub allow_risky: bool,
}

impl StandingOrder {
    pub fn new(id: &str, action: OrderAction) -> StandingOrder {
        StandingOrder {
            id: id.to_string(),
            kinds: Vec::new(),
            repos: Vec::new(),
            paths: Vec::new(),
            keywords: Vec::new(),
            max_cost_usd: None,
            action,
            allow_risky: false,
        }
    }

    /// No condition at all: it would match everything.
    pub fn is_unconditional(&self) -> bool {
        self.kinds.is_empty()
            && self.repos.is_empty()
            && self.paths.is_empty()
            && self.keywords.is_empty()
            && self.max_cost_usd.is_none()
    }

    /// Whether the follow-up fits every condition.
    pub fn matches(&self, f: &FollowUp) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&f.kind) {
            return false;
        }
        if !self.repos.is_empty()
            && !self
                .repos
                .iter()
                .any(|r| glob_match(&r.to_ascii_lowercase(), &f.repo.to_ascii_lowercase(), true))
        {
            return false;
        }
        if !self.paths.is_empty()
            && (f.paths.is_empty()
                || !f.paths.iter().all(|p| {
                    let p = normalize(p).unwrap_or_else(|| p.replace('\\', "/"));
                    self.paths.iter().any(|g| glob_match(g, &p, is_windows(&p)))
                }))
        {
            return false;
        }
        if !self.keywords.is_empty() {
            let text = f.text.to_lowercase();
            if !self
                .keywords
                .iter()
                .any(|k| text.contains(&k.to_lowercase()))
            {
                return false;
            }
        }
        if let Some(max) = self.max_cost_usd
            && !f.est_cost_usd.is_some_and(|c| c <= max)
        {
            return false;
        }
        true
    }
}

/// Paths no standing order may approve work on, whatever it says: secrets,
/// credentials, CI definitions, git internals and agent settings.
pub const PROTECTED_GLOBS: [&str; 14] = [
    "**/.env",
    "**/.env.*",
    "**/.git/**",
    "**/.github/workflows/**",
    "**/.ssh/**",
    "**/*.pem",
    "**/*.key",
    "**/secrets/**",
    "**/.claude/**",
    "**/.swarmux/**",
    "**/id_rsa*",
    "**/credentials*",
    "**/.npmrc",
    "**/.netrc",
];

/// Whether any of `paths` is protected: by [`PROTECTED_GLOBS`] or by one of
/// `extra` (the Guard's and the done-gate's protected paths).
pub fn touches_protected(paths: &[String], extra: &[String]) -> bool {
    paths.iter().any(|p| {
        let p = normalize(p).unwrap_or_else(|| p.replace('\\', "/"));
        let fold = is_windows(&p);
        PROTECTED_GLOBS
            .iter()
            .copied()
            .chain(extra.iter().map(String::as_str))
            .any(|g| glob_match(g, &p, fold))
    })
}

/// What the orders decided for one follow-up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Matched {
    /// No order applies: the owner decides at the next brief.
    None,
    /// The order's id.
    Approve(String),
    /// Matched, but left for the owner (an `always-ask` order, or an
    /// `auto-approve` order that is not allowed to approve this one).
    Ask {
        order: String,
        why: &'static str,
    },
    Park(String),
}

/// The first order that matches decides. `urgent` follow-ups are never
/// approved by an order; `extra_protected` adds to [`PROTECTED_GLOBS`].
pub fn decide(
    f: &FollowUp,
    orders: &[StandingOrder],
    urgent: bool,
    extra_protected: &[String],
) -> Matched {
    for o in orders {
        if o.action == OrderAction::AutoApprove && o.is_unconditional() {
            continue;
        }
        if !o.matches(f) {
            continue;
        }
        return match o.action {
            OrderAction::AlwaysAsk => Matched::Ask {
                order: o.id.clone(),
                why: "the order says always ask",
            },
            OrderAction::Park => Matched::Park(o.id.clone()),
            OrderAction::AutoApprove => {
                let why = if f.kind == FollowKind::Question {
                    Some("a question needs an answer, not an approval")
                } else if f.kind.needs_owner() && !o.allow_risky {
                    Some("decisions, risks and blockers need allow_risky")
                } else if urgent {
                    Some("urgent items are never approved by an order")
                } else if touches_protected(&f.paths, extra_protected) {
                    Some("it names a protected path")
                } else {
                    None
                };
                match why {
                    Some(why) => Matched::Ask {
                        order: o.id.clone(),
                        why,
                    },
                    None => Matched::Approve(o.id.clone()),
                }
            }
        };
    }
    Matched::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobId;
    use crate::followup::{FollowUp, Source};

    fn f(kind: FollowKind, text: &str, paths: &[&str]) -> FollowUp {
        let mut f = FollowUp::new(
            kind,
            text,
            Source {
                job: Some(JobId(1)),
                ..Source::default()
            },
            0,
        );
        f.paths = paths.iter().map(|p| p.to_string()).collect();
        f.repo = "api".into();
        f
    }

    fn docs() -> StandingOrder {
        let mut o = StandingOrder::new("docs", OrderAction::AutoApprove);
        o.kinds = vec![FollowKind::SuggestedTask];
        o.paths = vec!["docs/**".into(), "**/*.md".into()];
        o
    }

    #[test]
    fn a_matching_order_approves_a_suggested_task() {
        let task = f(
            FollowKind::SuggestedTask,
            "fix typos",
            &["docs/a.md", "README.md"],
        );
        assert_eq!(
            decide(&task, &[docs()], false, &[]),
            Matched::Approve("docs".into())
        );
    }

    #[test]
    fn every_path_must_match_and_files_must_be_named() {
        let mixed = f(FollowKind::SuggestedTask, "x", &["docs/a.md", "src/lib.rs"]);
        assert_eq!(decide(&mixed, &[docs()], false, &[]), Matched::None);
        let none = f(FollowKind::SuggestedTask, "x", &[]);
        assert_eq!(decide(&none, &[docs()], false, &[]), Matched::None);
    }

    #[test]
    fn decisions_and_risks_are_never_auto_approved_unless_the_order_says_so() {
        for kind in [FollowKind::Decision, FollowKind::Risk, FollowKind::Blocker] {
            let mut o = StandingOrder::new("any", OrderAction::AutoApprove);
            o.kinds = vec![kind];
            let item = f(kind, "pick A or B", &[]);
            assert!(matches!(
                decide(&item, std::slice::from_ref(&o), false, &[]),
                Matched::Ask { .. }
            ));
            o.allow_risky = true;
            assert_eq!(
                decide(&item, &[o], false, &[]),
                Matched::Approve("any".into())
            );
        }
    }

    #[test]
    fn a_question_is_never_approved() {
        let mut o = StandingOrder::new("q", OrderAction::AutoApprove);
        o.kinds = vec![FollowKind::Question];
        o.allow_risky = true;
        let q = f(FollowKind::Question, "which db?", &[]);
        assert!(matches!(decide(&q, &[o], false, &[]), Matched::Ask { .. }));
    }

    #[test]
    fn protected_paths_are_never_approved_even_when_the_order_matches() {
        let mut o = StandingOrder::new("all-tasks", OrderAction::AutoApprove);
        o.kinds = vec![FollowKind::SuggestedTask];
        for path in [
            ".env",
            "svc/.env.production",
            ".github/workflows/ci.yml",
            "C:\\work\\repo\\.git\\config",
            "keys/server.pem",
        ] {
            let item = f(FollowKind::SuggestedTask, "tidy", &[path]);
            assert!(
                matches!(
                    decide(&item, std::slice::from_ref(&o), false, &[]),
                    Matched::Ask { .. }
                ),
                "{path}"
            );
        }
        let extra = vec!["infra/**".to_string()];
        let item = f(FollowKind::SuggestedTask, "tidy", &["infra/main.tf"]);
        assert!(matches!(
            decide(&item, &[o], false, &extra),
            Matched::Ask { .. }
        ));
    }

    #[test]
    fn urgent_items_are_not_approved() {
        let mut o = StandingOrder::new("b", OrderAction::AutoApprove);
        o.kinds = vec![FollowKind::Blocker];
        o.allow_risky = true;
        let item = f(FollowKind::Blocker, "needs migration", &[]);
        assert!(matches!(
            decide(&item, &[o], true, &[]),
            Matched::Ask { .. }
        ));
    }

    #[test]
    fn an_order_with_no_conditions_approves_nothing() {
        let o = StandingOrder::new("everything", OrderAction::AutoApprove);
        assert!(o.is_unconditional());
        let item = f(FollowKind::SuggestedTask, "x", &[]);
        assert_eq!(decide(&item, &[o], false, &[]), Matched::None);
    }

    #[test]
    fn the_first_match_wins_so_always_ask_shields_later_orders() {
        let mut ask = StandingOrder::new("ask-migrations", OrderAction::AlwaysAsk);
        ask.keywords = vec!["Migration".into()];
        let mut approve = StandingOrder::new("tasks", OrderAction::AutoApprove);
        approve.kinds = vec![FollowKind::SuggestedTask];
        let item = f(FollowKind::SuggestedTask, "add a migration for users", &[]);
        assert!(matches!(
            decide(&item, &[ask, approve], false, &[]),
            Matched::Ask { order, .. } if order == "ask-migrations"
        ));
    }

    #[test]
    fn repo_cost_and_park() {
        let mut o = StandingOrder::new("cheap", OrderAction::AutoApprove);
        o.repos = vec!["API".into()];
        o.max_cost_usd = Some(0.50);
        let mut item = f(FollowKind::SuggestedTask, "x", &[]);
        assert_eq!(
            decide(&item, std::slice::from_ref(&o), false, &[]),
            Matched::None,
            "cost unknown"
        );
        item.est_cost_usd = Some(0.75);
        assert_eq!(
            decide(&item, std::slice::from_ref(&o), false, &[]),
            Matched::None
        );
        item.est_cost_usd = Some(0.25);
        assert_eq!(
            decide(&item, std::slice::from_ref(&o), false, &[]),
            Matched::Approve("cheap".into())
        );
        item.repo = "web".into();
        assert_eq!(
            decide(&item, std::slice::from_ref(&o), false, &[]),
            Matched::None
        );
        let mut p = StandingOrder::new("later", OrderAction::Park);
        p.keywords = vec!["someday".into()];
        item.text = "Someday rewrite it".into();
        assert_eq!(
            decide(&item, &[o, p], false, &[]),
            Matched::Park("later".into())
        );
    }
}
