//! Project knowledge: rules, facts and decisions the owner keeps for the
//! agents of a workspace or a repository, and how they reach them.
//!
//! Everything about an item is an [`Event`]; the current view is a
//! [`Projection`] folded from them, so it can always be rebuilt. An item is
//! anchored to the code it talks about: when an anchor's content changes,
//! the item turns [`Status::Stale`] and is withheld from agents until the
//! owner confirms or edits it.
//!
//! Rules have an [`Enforcer`]: the Guard on tool calls, a hidden check of
//! the done-gate, a visible lint command, or prompt text only. Only items
//! that are active, verified and in scope ever reach an agent, quoted and
//! tagged with where they came from ([`deliver`]).

pub mod anchor;
pub mod deliver;
pub mod map;
pub mod rule;

use std::collections::BTreeMap;
use std::fmt;

use crate::guard::glob::{glob_match, normalize};
use crate::{Millis, WorkspaceId};

pub use anchor::{Anchor, content_hash, valid_for};
pub use rule::{Action, CheckRule, CompiledGuard, Enforcer, EnforcerKind, GuardRule, ToolCall};

/// The pushed core of a workspace (its map), in estimated tokens.
pub const CORE_TOKENS: usize = 1500;
/// The prompt-class rules pushed with the core, in estimated tokens.
pub const PUSH_RULE_TOKENS: usize = 500;
/// What one triggered pull may add, in estimated tokens.
pub const PULL_TOKENS: usize = 300;
/// Longest item text, in characters.
pub const TEXT_MAX: usize = 400;
/// Longest "why", in characters.
pub const WHY_MAX: usize = 240;
/// How the reason of a guard decision made by a rule starts.
pub const RULE_REASON: &str = "knowledge rule";

/// Whether a guard decision's reason comes from a knowledge rule.
pub fn is_rule_reason(reason: &str) -> bool {
    reason.starts_with(RULE_REASON)
}

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Rule,
    Fact,
    Decision,
    /// Reserved: an interface between repositories (extracted later).
    Contract,
    /// Reserved: something learned from a correction (later).
    Lesson,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Rule,
        Kind::Fact,
        Kind::Decision,
        Kind::Contract,
        Kind::Lesson,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Rule => "rule",
            Kind::Fact => "fact",
            Kind::Decision => "decision",
            Kind::Contract => "contract",
            Kind::Lesson => "lesson",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The letter its ids start with.
    pub fn letter(self) -> char {
        match self {
            Kind::Rule => 'R',
            Kind::Fact => 'F',
            Kind::Decision => 'D',
            Kind::Contract => 'C',
            Kind::Lesson => 'L',
        }
    }

    /// Kinds kept for later milestones: nothing can add them yet.
    pub fn is_reserved(self) -> bool {
        matches!(self, Kind::Contract | Kind::Lesson)
    }
}

/// An item's id: its kind's letter and a number, `R3`, `F12`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(String);

impl ItemId {
    pub fn new(kind: Kind, n: u32) -> ItemId {
        ItemId(format!("{}{n}", kind.letter()))
    }

    /// `R3`, `r3` or `F12`; `None` for anything else.
    pub fn parse(s: &str) -> Option<ItemId> {
        let s = s.trim();
        let mut chars = s.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let digits = chars.as_str();
        Kind::ALL.iter().find(|k| k.letter() == letter)?;
        let n: u32 = digits.parse().ok()?;
        Some(ItemId(format!("{letter}{n}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> Option<Kind> {
        let letter = self.0.chars().next()?;
        Kind::ALL.into_iter().find(|k| k.letter() == letter)
    }

    pub fn number(&self) -> u32 {
        self.0[1..].parse().unwrap_or(0)
    }

    fn key(&self) -> (char, u32) {
        (self.0.chars().next().unwrap_or('?'), self.number())
    }
}

impl Ord for ItemId {
    fn cmp(&self, other: &ItemId) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for ItemId {
    fn partial_cmp(&self, other: &ItemId) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// Whose knowledge it is: a workspace's, or a repository's (by its folder).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Home {
    Workspace(WorkspaceId),
    /// A repository worked on alone, by its folder (`/`-separated).
    Repo(String),
}

impl Home {
    /// The home of a repository at `path`, normalised.
    pub fn repo(path: &str) -> Home {
        let norm = normalize(path).unwrap_or_else(|| path.replace('\\', "/"));
        let norm = norm.trim_end_matches('/');
        Home::Repo(if norm.is_empty() { "/" } else { norm }.to_string())
    }

    /// `workspace:<id>` or `repo:<path>`.
    pub fn key(&self) -> String {
        match self {
            Home::Workspace(id) => format!("workspace:{id}"),
            Home::Repo(path) => format!("repo:{path}"),
        }
    }

    pub fn parse(key: &str) -> Option<Home> {
        if let Some(id) = key.strip_prefix("workspace:") {
            return WorkspaceId::parse(id).map(Home::Workspace);
        }
        key.strip_prefix("repo:")
            .filter(|p| !p.is_empty())
            .map(Home::repo)
    }

    /// Whether `other` names the same home (repository folders compared
    /// without case on Windows).
    pub fn same(&self, other: &Home) -> bool {
        match (self, other) {
            (Home::Repo(a), Home::Repo(b)) if crate::guard::glob::is_windows(a) => {
                a.eq_ignore_ascii_case(b)
            }
            _ => self == other,
        }
    }
}

/// Where an item stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    /// Drafted, waiting for the owner.
    Proposed,
    /// A rule that logs what it would have done, without acting.
    Shadow,
    /// A rule whose enforcer acts.
    Enforced,
    /// Delivered as text only.
    Advisory,
    /// An anchor changed: withheld until the owner confirms or edits it.
    Stale,
    Retired,
}

impl Status {
    pub const ALL: [Status; 6] = [
        Status::Proposed,
        Status::Shadow,
        Status::Enforced,
        Status::Advisory,
        Status::Stale,
        Status::Retired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proposed => "proposed",
            Status::Shadow => "shadow",
            Status::Enforced => "enforced",
            Status::Advisory => "advisory",
            Status::Stale => "stale",
            Status::Retired => "retired",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Delivered to agents (when verified and in scope).
    pub fn is_active(self) -> bool {
        matches!(self, Status::Enforced | Status::Advisory)
    }
}

/// Why `kind` may not go from `from` to `to` by the owner's hand, if it may not.
/// Staleness comes from anchors and leaves only through a confirmation.
pub fn check_move(kind: Kind, from: Status, to: Status) -> Result<(), String> {
    use Status::*;
    if from == to {
        return Err(format!("it is {} already", to.as_str()));
    }
    let rule_only = matches!(to, Shadow | Enforced);
    if rule_only && kind != Kind::Rule {
        return Err(format!("only rules can be {}", to.as_str()));
    }
    match (from, to) {
        (Retired, _) => Err("it is retired; add it again instead".into()),
        (_, Stale) => Err("items turn stale when their anchors change".into()),
        (Stale, Retired) => Ok(()),
        (Stale, _) => Err("it is stale: confirm or edit it first".into()),
        (_, Proposed) => Err("nothing goes back to proposed".into()),
        _ => Ok(()),
    }
}

/// Who or what an item came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    Owner,
    /// The owner's correction of an agent's work.
    Correction,
    /// Something the done-gate caught.
    Gate,
    /// A deterministic extractor (the workspace scan).
    Extractor,
    /// An agent's question the owner answered.
    AgentAsk,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::Owner,
        Source::Correction,
        Source::Gate,
        Source::Extractor,
        Source::AgentAsk,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Owner => "owner",
            Source::Correction => "correction",
            Source::Gate => "gate",
            Source::Extractor => "extractor",
            Source::AgentAsk => "agent-ask",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The trust an item from it starts with: text an agent wrote is
    /// untrusted until the owner says otherwise.
    pub fn default_trust(self) -> Trust {
        match self {
            Source::AgentAsk => Trust::Untrusted,
            _ => Trust::Verified,
        }
    }
}

/// Whether an item may reach agents at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Trust {
    Verified,
    Untrusted,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Verified => "verified",
            Trust::Untrusted => "untrusted",
        }
    }

    pub fn parse(s: &str) -> Option<Trust> {
        match s {
            "verified" => Some(Trust::Verified),
            "untrusted" => Some(Trust::Untrusted),
            _ => None,
        }
    }
}

/// Where an item came from, and the ledger entries that recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    pub source: Source,
    /// Ledger `seq`s of the events that made and changed it.
    pub events: Vec<u64>,
}

impl Provenance {
    pub fn of(source: Source) -> Provenance {
        Provenance {
            source,
            events: Vec::new(),
        }
    }
}

/// What an item applies to. Empty lists mean everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// Repository names of the workspace.
    pub repos: Vec<String>,
    /// Globs relative to the home's root (the workspace's layout), `/`.
    pub paths: Vec<String>,
    /// Task classes: `work`, `review`, `mission`.
    pub classes: Vec<String>,
}

impl Scope {
    /// Whether it applies to a job of `job`'s repositories and classes. A
    /// job that names no repository touches them all.
    pub fn applies_to(&self, job: &JobScope) -> bool {
        let repos = self.repos.is_empty()
            || job.repos.is_empty()
            || self.repos.iter().any(|r| job.repos.contains(r));
        let classes =
            self.classes.is_empty() || self.classes.iter().any(|c| job.classes.contains(c));
        repos && classes
    }

    /// Whether one of its path globs matches `rel` (relative, `/`).
    pub fn matches_path(&self, rel: &str, fold: bool) -> bool {
        self.paths.iter().any(|g| glob_match(g, rel, fold))
    }
}

/// What a running job is, for scope matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScope {
    pub home: Home,
    /// The repositories it touches; empty: all.
    pub repos: Vec<String>,
    pub classes: Vec<String>,
}

/// How often an item was used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Times it was delivered or its enforcer acted.
    pub hits: u64,
    /// Times a shadow rule would have acted.
    pub shadow_hits: u64,
    pub last_used: Option<Millis>,
}

/// One rule, fact or decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub home: Home,
    pub kind: Kind,
    /// One to three sentences.
    pub text: String,
    /// One sentence, written when it was decided.
    pub why: String,
    pub scope: Scope,
    pub anchors: Vec<Anchor>,
    /// The hash over the anchors it is valid for.
    pub valid_for: String,
    pub status: Status,
    /// What it was before it turned stale; a confirmation goes back there.
    pub prior: Option<Status>,
    pub provenance: Provenance,
    pub trust: Trust,
    /// Part of the human-curated core pushed to every session.
    pub core: bool,
    /// Rules only.
    pub enforcer: Option<Enforcer>,
    pub counters: Counters,
    /// The last result of its gate or lint check.
    pub last_check: Option<bool>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

impl Item {
    /// Whether it may reach an agent: active, verified and of a kind in use.
    pub fn deliverable(&self) -> bool {
        self.status.is_active() && self.trust == Trust::Verified && !self.kind.is_reserved()
    }

    pub fn enforcer_kind(&self) -> Option<EnforcerKind> {
        self.enforcer.as_ref().map(Enforcer::kind)
    }

    /// A rule that reaches agents as text only: a prompt rule, or one
    /// made advisory.
    pub fn is_text_rule(&self) -> bool {
        self.kind == Kind::Rule
            && (self.status == Status::Advisory
                || self.enforcer_kind() == Some(EnforcerKind::Prompt))
    }

    /// A rule whose enforcer acts or logs (not prompt-only, not advisory).
    pub fn acts(&self) -> bool {
        self.kind == Kind::Rule
            && matches!(self.status, Status::Enforced | Status::Shadow)
            && self.trust == Trust::Verified
            && !matches!(self.enforcer_kind(), None | Some(EnforcerKind::Prompt))
    }
}

/// What the owner (or a tool) states when adding or editing an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub home: Home,
    pub kind: Kind,
    pub text: String,
    pub why: String,
    pub scope: Scope,
    /// Paths (and symbols) only: the hashes are taken when it is saved.
    pub anchors: Vec<Anchor>,
    pub core: bool,
    pub enforcer: Option<Enforcer>,
    /// `None`: proposed for drafts from tools, advisory or shadow from the owner.
    pub status: Option<Status>,
    pub source: Source,
}

impl Draft {
    /// A draft of `kind` with `text` from the owner, everything else empty.
    pub fn new(home: Home, kind: Kind, text: &str) -> Draft {
        Draft {
            home,
            kind,
            text: text.to_string(),
            why: String::new(),
            scope: Scope::default(),
            anchors: Vec::new(),
            core: false,
            enforcer: None,
            status: None,
            source: Source::Owner,
        }
    }

    /// Checks the draft. A rule with no enforcer is a prompt rule.
    pub fn validate(&self) -> Result<(), String> {
        if self.kind.is_reserved() {
            return Err(format!(
                "{} items are not available yet",
                self.kind.as_str()
            ));
        }
        let text = self.text.trim();
        if text.is_empty() {
            return Err("the text is empty".into());
        }
        if text.chars().count() > TEXT_MAX {
            return Err(format!("the text is longer than {TEXT_MAX} characters"));
        }
        if self.why.chars().count() > WHY_MAX {
            return Err(format!("the why is longer than {WHY_MAX} characters"));
        }
        if self.enforcer.is_some() && self.kind != Kind::Rule {
            return Err("only rules have enforcers".into());
        }
        if let Some(e) = &self.enforcer {
            e.validate()?;
        }
        for a in &self.anchors {
            if a.path.trim().is_empty() || crate::guard::glob::is_absolute(&a.path) {
                return Err(format!(
                    "anchor {:?} must be a path relative to the root",
                    a.path
                ));
            }
        }
        match self.status {
            Some(Status::Stale | Status::Retired) => {
                Err("a new item can't start stale or retired".into())
            }
            Some(s @ (Status::Shadow | Status::Enforced)) if self.kind != Kind::Rule => {
                Err(format!("only rules can be {}", s.as_str()))
            }
            _ => Ok(()),
        }
    }

    /// The status a new item starts in.
    pub fn first_status(&self) -> Status {
        if let Some(s) = self.status {
            return s;
        }
        match (self.source, self.kind) {
            (Source::Owner, Kind::Rule) => {
                match self
                    .enforcer
                    .as_ref()
                    .map_or(EnforcerKind::Prompt, Enforcer::kind)
                {
                    // A rule that acts proves itself in shadow first.
                    EnforcerKind::Guard | EnforcerKind::Gate | EnforcerKind::Lint => Status::Shadow,
                    EnforcerKind::Prompt => Status::Enforced,
                }
            }
            (Source::Owner, _) => Status::Advisory,
            _ => Status::Proposed,
        }
    }

    /// The item it makes, with `anchors` hashed by the caller.
    pub fn into_item(self, id: ItemId, anchors: Vec<Anchor>, now: Millis) -> Item {
        let status = self.first_status();
        let enforcer = match self.kind {
            Kind::Rule => Some(self.enforcer.unwrap_or(Enforcer::Prompt)),
            _ => None,
        };
        Item {
            id,
            home: self.home,
            kind: self.kind,
            text: self.text.trim().to_string(),
            why: self.why.trim().to_string(),
            scope: self.scope,
            valid_for: valid_for(&anchors),
            anchors,
            status,
            prior: None,
            provenance: Provenance::of(self.source),
            trust: self.source.default_trust(),
            core: self.core,
            enforcer,
            counters: Counters::default(),
            last_check: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// One change of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Its place in the store's log; 0 before it is stored.
    pub seq: u64,
    pub at: Millis,
    pub item: ItemId,
    /// Who made it: `owner`, `swarmux`, …
    pub actor: String,
    pub change: Change,
    /// The ledger entry that records it.
    pub ledger: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Added, edited or imported: the item's content as it is now.
    Put(Box<Item>),
    Status {
        to: Status,
        note: String,
    },
    /// The owner confirmed it still holds: these are its anchors now.
    Confirmed {
        anchors: Vec<Anchor>,
        valid_for: String,
    },
    /// Delivered or acted `hits` times (or would have, in shadow).
    Used {
        hits: u64,
        shadow: bool,
    },
    /// Its gate or lint check ran.
    Checked {
        passed: bool,
    },
    /// The owner said whether it may reach agents.
    Trusted(Trust),
}

impl Change {
    /// The name the ledger records.
    pub fn name(&self) -> &'static str {
        match self {
            Change::Put(_) => "put",
            Change::Status { .. } => "status",
            Change::Confirmed { .. } => "confirmed",
            Change::Used { .. } => "used",
            Change::Checked { .. } => "checked",
            Change::Trusted(_) => "trusted",
        }
    }
}

/// The items as the events left them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Projection {
    items: BTreeMap<ItemId, Item>,
}

impl Projection {
    pub fn fold(events: &[Event]) -> Projection {
        let mut p = Projection::default();
        for e in events {
            p.apply(e);
        }
        p
    }

    /// Applies one event. Events about an item never put are ignored.
    pub fn apply(&mut self, e: &Event) {
        if let Change::Put(item) = &e.change {
            let mut next = (**item).clone();
            next.id = e.item.clone();
            if let Some(old) = self.items.get(&e.item) {
                next.counters = old.counters;
                next.last_check = old.last_check;
                next.created_at = old.created_at;
                let mut events = old.provenance.events.clone();
                for seq in &next.provenance.events {
                    if !events.contains(seq) {
                        events.push(*seq);
                    }
                }
                next.provenance.events = events;
            }
            if let Some(seq) = e.ledger
                && !next.provenance.events.contains(&seq)
            {
                next.provenance.events.push(seq);
            }
            next.updated_at = e.at;
            self.items.insert(e.item.clone(), next);
            return;
        }
        let Some(item) = self.items.get_mut(&e.item) else {
            return;
        };
        match &e.change {
            Change::Put(_) => {}
            Change::Status { to, .. } => {
                if *to == Status::Stale {
                    if item.status != Status::Stale {
                        item.prior = Some(item.status);
                    }
                } else {
                    item.prior = None;
                }
                item.status = *to;
                item.updated_at = e.at;
                item.provenance.events.extend(e.ledger);
            }
            Change::Confirmed { anchors, valid_for } => {
                item.anchors = anchors.clone();
                item.valid_for = valid_for.clone();
                if item.status == Status::Stale {
                    item.status = item.prior.take().unwrap_or(Status::Advisory);
                }
                item.updated_at = e.at;
                item.provenance.events.extend(e.ledger);
            }
            Change::Used { hits, shadow } => {
                if *shadow {
                    item.counters.shadow_hits += hits;
                } else {
                    item.counters.hits += hits;
                }
                item.counters.last_used = Some(e.at);
            }
            Change::Checked { passed } => item.last_check = Some(*passed),
            Change::Trusted(t) => {
                item.trust = *t;
                item.updated_at = e.at;
                item.provenance.events.extend(e.ledger);
            }
        }
    }

    pub fn get(&self, id: &ItemId) -> Option<&Item> {
        self.items.get(id)
    }

    /// Every item, by id.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.values()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The id a new item of `kind` gets: one more than the highest.
    pub fn next_id(&self, kind: Kind) -> ItemId {
        let top = self
            .items
            .keys()
            .filter(|id| id.kind() == Some(kind))
            .map(ItemId::number)
            .max()
            .unwrap_or(0);
        ItemId::new(kind, top + 1)
    }
}

/// The rule registry at a glance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    /// Rules not retired.
    pub rules: usize,
    /// Rules whose guard, gate or lint enforcer acts.
    pub enforced: usize,
    /// Enforced rules whose last check failed.
    pub failing: usize,
    /// Active rules that are only text: a weakness to fix.
    pub advisory_only: usize,
    pub shadow: usize,
    pub stale: usize,
    pub proposed: usize,
}

impl Registry {
    pub fn of<'a>(items: impl IntoIterator<Item = &'a Item>) -> Registry {
        let mut r = Registry::default();
        for i in items {
            if i.kind != Kind::Rule || i.status == Status::Retired {
                continue;
            }
            r.rules += 1;
            let acts = !matches!(i.enforcer_kind(), None | Some(EnforcerKind::Prompt));
            match i.status {
                Status::Enforced if acts => {
                    r.enforced += 1;
                    if i.last_check == Some(false) {
                        r.failing += 1;
                    }
                }
                Status::Enforced | Status::Advisory => r.advisory_only += 1,
                Status::Shadow => r.shadow += 1,
                Status::Stale => r.stale += 1,
                Status::Proposed => r.proposed += 1,
                Status::Retired => {}
            }
        }
        r
    }
}

impl fmt::Display for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} rules · {} enforced · {} failing · {} advisory-only",
            self.rules, self.enforced, self.failing, self.advisory_only
        )
    }
}

#[cfg(test)]
mod tests;
