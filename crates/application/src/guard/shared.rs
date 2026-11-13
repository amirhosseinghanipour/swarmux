//! The policy, shared with the hook transport so it can answer at once.

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Instant;

use swarmux_domain::{
    Decision, GuardPolicy, JobId, Millis, Pattern, StoredRule, Verdict, agent_cli_refusal,
    agent_config_write, needs_typed_confirmation, touches_fenced,
};

use crate::gate::WriteFence;
use crate::knowledge::RuleGuard;
use crate::ports::{HookRequest, RepoRoots};
use crate::watchdog::NudgeGate;

/// The reason of a call held because it writes a file that steers agents.
pub const PROTECTED_REASON: &str = "protected path";
/// The reason of a call held because only a typed confirmation allows it.
pub const IRREVERSIBLE_REASON: &str = "irreversible";

/// Whether a held call's reason makes it the owner's alone: no automated
/// decider may take it from the owner and no rule can be made from it.
pub(crate) fn owner_only(reason: Option<&str>) -> bool {
    reason.is_some_and(|r| {
        swarmux_domain::knowledge::is_rule_reason(r)
            || r.starts_with(PROTECTED_REASON)
            || r.starts_with(IRREVERSIBLE_REASON)
    })
}

/// The jobs nobody is watching: missions, reviewers and what continues
/// them. Shared between the job service (which knows) and the Guard.
#[derive(Clone, Default)]
pub struct UnattendedJobs(Arc<Mutex<HashSet<JobId>>>);

impl UnattendedJobs {
    pub fn new() -> UnattendedJobs {
        UnattendedJobs::default()
    }

    pub fn mark(&self, job: JobId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(job);
    }

    pub fn contains(&self, job: JobId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&job)
    }
}

/// Follows a path through symlinks the way the file system does.
pub type Resolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What a job's tool calls may not reach, as data.
#[derive(Clone)]
pub struct Protection {
    /// Absolute paths (a file, or a folder with everything below it) no
    /// tool call may read or write: the database, key, config, hidden
    /// checks and spools.
    pub fenced: Vec<String>,
    /// The user's home, for `~` and `$HOME`.
    pub home: String,
    /// The path of the running swarmux binary.
    pub exe: Option<String>,
    /// Files whose writes are held, as globs from the repository root.
    pub protected_globs: Vec<String>,
    /// Follows symlinks the way the file system does.
    pub resolve: Resolver,
}

impl Protection {
    pub fn new(fenced: Vec<String>, home: &str) -> Protection {
        Protection {
            fenced,
            home: home.to_string(),
            exe: None,
            protected_globs: Vec::new(),
            resolve: Arc::new(|_| None),
        }
    }
}

/// An allow rule that may be used: parsed, and where it applies.
type Learned = (StoredRule, Pattern);

/// One policy for the UI thread and the transport's threads.
#[derive(Clone)]
pub struct SharedPolicy {
    policy: Arc<RwLock<GuardPolicy>>,
    /// Refuse writes (protected paths, a task's scope) before any rule
    /// is consulted, in the order they were added.
    fences: Vec<Arc<dyn WriteFence>>,
    /// Watchdog messages waiting for the agent's next tool call.
    nudges: Option<Arc<dyn NudgeGate>>,
    /// The owner's knowledge rules: they only ever forbid.
    rules: Option<Arc<dyn RuleGuard>>,
    /// Allow rules added while swarmux ran, each with where it applies.
    learned: Arc<RwLock<Vec<Learned>>>,
    roots: Option<Arc<dyn RepoRoots>>,
    protection: Option<Arc<Protection>>,
    unattended: UnattendedJobs,
    now: Arc<dyn Fn() -> Millis + Send + Sync>,
}

impl Default for SharedPolicy {
    fn default() -> SharedPolicy {
        SharedPolicy::new(GuardPolicy::default())
    }
}

impl fmt::Debug for SharedPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedPolicy")
            .field("policy", &self.policy)
            .field("fences", &self.fences.len())
            .field("nudges", &self.nudges.is_some())
            .field("rules", &self.rules.is_some())
            .finish()
    }
}

fn system_now() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| {
            Millis::try_from(d.as_millis()).unwrap_or(Millis::MAX)
        })
}

impl SharedPolicy {
    pub fn new(policy: GuardPolicy) -> SharedPolicy {
        SharedPolicy {
            policy: Arc::new(RwLock::new(policy)),
            fences: Vec::new(),
            nudges: None,
            rules: None,
            learned: Arc::default(),
            roots: None,
            protection: None,
            unattended: UnattendedJobs::new(),
            now: Arc::new(system_now),
        }
    }

    /// A job with a watchdog message waiting gets it as the reason its next
    /// matching call is not run: the one way hooks can tell a running
    /// agent something.
    pub fn with_nudges(mut self, nudges: Arc<dyn NudgeGate>) -> SharedPolicy {
        self.nudges = Some(nudges);
        self
    }

    /// Consults the knowledge rules after the fences and before the
    /// policy: a rule may deny or hold a call, never allow one, and the
    /// policy's deny still wins over a rule's ask.
    pub fn with_rules(mut self, rules: Arc<dyn RuleGuard>) -> SharedPolicy {
        self.rules = Some(rules);
        self
    }

    /// Writes `fence` refuses are denied whatever the rules say. Fences
    /// add up: the first that refuses gives the reason.
    pub fn with_fence(mut self, fence: Arc<dyn WriteFence>) -> SharedPolicy {
        self.fences.push(fence);
        self
    }

    /// Where calls are made: scoped rules apply by repository.
    pub fn with_roots(mut self, roots: Arc<dyn RepoRoots>) -> SharedPolicy {
        self.roots = Some(roots);
        self
    }

    /// Refuses swarmux's own commands, its state files and writes to the
    /// files that steer agents.
    pub fn with_protection(mut self, protection: Protection) -> SharedPolicy {
        self.protection = Some(Arc::new(protection));
        self
    }

    /// Which jobs are unattended (their protected writes are denied, not asked).
    pub fn with_unattended(mut self, unattended: UnattendedJobs) -> SharedPolicy {
        self.unattended = unattended;
        self
    }

    /// The clock rules expire by.
    pub fn with_now(mut self, now: Arc<dyn Fn() -> Millis + Send + Sync>) -> SharedPolicy {
        self.now = now;
        self
    }

    pub fn unattended(&self) -> &UnattendedJobs {
        &self.unattended
    }

    /// The repositories calls are made in, when known.
    pub fn roots(&self) -> Option<&Arc<dyn RepoRoots>> {
        self.roots.as_ref()
    }

    /// The decision for `request`, and how long it took in microseconds.
    pub fn decide(&self, request: &HookRequest) -> (Decision, u64) {
        let start = Instant::now();
        let decision = self.verdict(request);
        // A call the rules deny anyway keeps its reason and the message waits.
        let nudge = match (&self.nudges, request.job_id, decision.verdict) {
            (_, _, Verdict::Deny) | (None, _, _) | (_, None, _) => None,
            (Some(n), Some(job), _) => n.take(job, &request.tool_name, &request.subject),
        };
        let decision = match nudge {
            Some(text) => Decision::new(Verdict::Deny, Some(text)),
            None => decision,
        };
        let micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        (decision, micros)
    }

    fn verdict(&self, request: &HookRequest) -> Decision {
        let fenced = self.fences.iter().find_map(|f| {
            f.refuses(
                request.job_id,
                &request.tool_name,
                &request.subject,
                &request.cwd,
            )
        });
        if let Some(rule) = fenced {
            return Decision::new(Verdict::Deny, Some(rule));
        }
        if let Some(refusal) = self.refused(request) {
            return Decision::new(Verdict::Deny, Some(refusal));
        }
        let ruled = self.rules.as_ref().and_then(|r| r.check(request));
        let extra = self.applicable(request);
        let policy = self
            .policy
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .decide_with(&request.tool_name, &request.subject, &request.cwd, &extra);
        let decision = match ruled {
            Some(r) if r.verdict == Verdict::Deny || policy.verdict != Verdict::Deny => r,
            _ => policy,
        };
        let decision = self.protected(request, decision);
        self.irreversible(request, decision)
    }

    /// swarmux's own commands and files: refused whatever the rules say.
    fn refused(&self, request: &HookRequest) -> Option<String> {
        let p = self.protection.as_ref()?;
        if let Some(why) = agent_cli_refusal(&request.tool_name, &request.subject, p.exe.as_deref())
        {
            return Some(why);
        }
        let touched = touches_fenced(
            &p.fenced,
            &request.tool_name,
            &request.subject,
            &request.cwd,
            &p.home,
            &*p.resolve,
        )?;
        Some(format!(
            "{touched} belongs to swarmux and its owner, not to an agent's tools"
        ))
    }

    /// A write to a file that steers agents is held for the owner; for an
    /// unattended job it is refused. A deny stays a deny.
    fn protected(&self, request: &HookRequest, decision: Decision) -> Decision {
        let Some(p) = &self.protection else {
            return decision;
        };
        if decision.verdict == Verdict::Deny || request.cwd.is_empty() {
            return decision;
        }
        let root = self
            .roots
            .as_ref()
            .and_then(|r| r.tree_root(&request.cwd))
            .unwrap_or_else(|| request.cwd.clone());
        let Some(glob) = agent_config_write(
            &p.protected_globs,
            &root.replace('\\', "/"),
            &request.tool_name,
            &request.subject,
            &request.cwd,
        ) else {
            return decision;
        };
        let reason = format!("{PROTECTED_REASON} {glob}");
        if request.job_id.is_some_and(|j| self.unattended.contains(j)) {
            Decision::new(
                Verdict::Deny,
                Some(format!("{reason}: unattended jobs may not write it")),
            )
        } else {
            Decision::new(Verdict::Ask, Some(reason))
        }
    }

    /// Nothing but a typed confirmation allows an irreversible call: an
    /// allow (or a pass that leaves it to Claude Code's rules) becomes a
    /// hold.
    fn irreversible(&self, request: &HookRequest, decision: Decision) -> Decision {
        let lets_it_run = matches!(decision.verdict, Verdict::Allow | Verdict::Pass);
        if !lets_it_run
            || !needs_typed_confirmation(&request.tool_name, &request.subject, &request.cwd)
        {
            return decision;
        }
        let reason = match &decision.rule {
            Some(rule) => format!("{IRREVERSIBLE_REASON}: {rule} doesn't apply; name the target"),
            None => format!("{IRREVERSIBLE_REASON}: name the target to allow it"),
        };
        Decision::new(Verdict::Ask, Some(reason))
    }

    /// The learned rules that apply to this call: not expired, made for the
    /// repository (or workspace) it is in.
    fn applicable(&self, request: &HookRequest) -> Vec<Pattern> {
        let learned = self.learned.read().unwrap_or_else(PoisonError::into_inner);
        if learned.is_empty() {
            return Vec::new();
        }
        let now = (self.now)();
        // Outside a repository the folder itself is the repository, as it
        // was when the rule was made.
        let (repo, workspace) = match &self.roots {
            Some(roots) => (
                roots.repo_root(request.job_id, &request.cwd),
                roots.workspace_root(request.job_id, &request.cwd),
            ),
            None => (None, None),
        };
        let repo = repo.or_else(|| (!request.cwd.is_empty()).then(|| request.cwd.clone()));
        learned
            .iter()
            .filter(|(rule, _)| {
                !rule.expired(now) && rule.applies_to(repo.as_deref(), workspace.as_deref())
            })
            .map(|(_, pattern)| pattern.clone())
            .collect()
    }

    /// Replaces the learned rules with `rules`; ones that don't parse are
    /// left out and returned.
    pub fn set_learned(&self, rules: Vec<StoredRule>) -> Vec<StoredRule> {
        let mut bad = Vec::new();
        let mut good = Vec::new();
        for rule in rules {
            match Pattern::parse(&rule.text) {
                Ok(p) => good.push((rule, p)),
                Err(_) => bad.push(rule),
            }
        }
        *self.learned.write().unwrap_or_else(PoisonError::into_inner) = good;
        bad
    }

    /// Adds a learned rule (replacing one with the same id).
    pub fn add_learned(&self, rule: StoredRule) -> bool {
        let Ok(pattern) = Pattern::parse(&rule.text) else {
            return false;
        };
        let mut learned = self.learned.write().unwrap_or_else(PoisonError::into_inner);
        learned.retain(|(r, _)| r.id != rule.id);
        learned.push((rule, pattern));
        true
    }

    pub fn remove_learned(&self, id: &str) -> bool {
        let mut learned = self.learned.write().unwrap_or_else(PoisonError::into_inner);
        let before = learned.len();
        learned.retain(|(r, _)| r.id != id);
        learned.len() != before
    }

    /// The learned rules, expired ones included.
    pub fn learned(&self) -> Vec<StoredRule> {
        self.learned
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }

    pub fn add_allow(&self, rule: Pattern) {
        self.policy
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .add_allow(rule);
    }

    /// A copy of the current policy.
    pub fn snapshot(&self) -> GuardPolicy {
        self.policy
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
