//! The contracts the hook transport's threads and the done-gate read:
//! the workspace indexes, what each running job's tree looked like when it
//! started, and the notes and verdicts built from them. Everything they
//! produce for the ledger waits in a queue for the service to record.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::contracts::{self, Delta, Index, PULL_TOKENS, Report};
use swarmux_domain::gate::{CheckRun, GateCheck};
use swarmux_domain::mission::check_in;
use swarmux_domain::workspace::split_check;
use swarmux_domain::{JobId, WorkspaceId, estimate_tokens};

use super::{Cache, ContractEvent, Escalation, SourceTree, scan_tree};
use crate::knowledge::{KnowledgeHub, ToolContext};
use crate::ports::HookRequest;

/// A pull is worth making when at least this many tokens remain.
const MIN_PULL_TOKENS: usize = 60;
/// The longest verdict text a check run carries, in characters.
const OUTPUT_MAX: usize = 1800;
/// Commands to run in a consumer repository, best first: what judges
/// whether code still agrees with another repository's interface.
const CONSUMER_COMMANDS: [&str; 6] = ["analyze", "typecheck", "check", "lint", "build", "test"];

/// What a running job's tree is, for contracts.
#[derive(Clone)]
pub(crate) struct JobTree {
    pub workspace: WorkspaceId,
    pub root: PathBuf,
    /// Repository name and its folder below the root.
    pub repos: Vec<(String, String)>,
    /// Each repository's detected commands (name, line).
    pub commands: Vec<(String, Vec<(String, String)>)>,
    /// The contracts when the job started.
    pub baseline: Arc<Index>,
    /// The mission's repositories (empty: all) and acceptance checks.
    pub mission: Option<(Vec<String>, Vec<String>)>,
}

/// What the cross-repo check of a mission's gate decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractJudgement {
    /// Checks of the repositories that consume what changed, to run with
    /// the others.
    pub checks: Vec<GateCheck>,
    /// The verdict as one check run named `contracts`: it fails when a
    /// removed or renamed symbol is still used.
    pub run: CheckRun,
}

/// Judges a mission job's change against the contracts.
pub trait ContractGate: Send + Sync {
    /// `None` when the job changed nothing that others consume.
    fn judge(&self, job: JobId) -> Option<ContractJudgement>;
}

#[derive(Default)]
struct State {
    caches: HashMap<WorkspaceId, Arc<Cache>>,
    indexes: HashMap<WorkspaceId, Arc<Index>>,
    jobs: HashMap<JobId, JobTree>,
    delivered: HashMap<JobId, BTreeSet<String>>,
    events: Vec<ContractEvent>,
    /// The last verdict per job, so an unchanged one is recorded once.
    last: HashMap<JobId, ContractEvent>,
    escalations: HashMap<JobId, Escalation>,
}

/// The contracts shared with other threads.
pub struct ContractHub {
    state: Mutex<State>,
    source: Arc<dyn SourceTree>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ContractHub {
    pub fn new(source: Arc<dyn SourceTree>) -> Arc<ContractHub> {
        Arc::new(ContractHub {
            state: Mutex::default(),
            source,
        })
    }

    pub(crate) fn set_cache(&self, w: WorkspaceId, cache: Arc<Cache>) {
        lock(&self.state).caches.insert(w, cache);
    }

    pub(crate) fn cache(&self, w: WorkspaceId) -> Arc<Cache> {
        lock(&self.state)
            .caches
            .get(&w)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_index(&self, w: WorkspaceId, index: Arc<Index>) {
        lock(&self.state).indexes.insert(w, index);
    }

    pub(crate) fn index(&self, w: WorkspaceId) -> Option<Arc<Index>> {
        lock(&self.state).indexes.get(&w).cloned()
    }

    pub(crate) fn register(&self, job: JobId, tree: JobTree) {
        let mut s = lock(&self.state);
        // A restarted job keeps its mission.
        let mission = s.jobs.get(&job).and_then(|t| t.mission.clone());
        s.jobs.insert(
            job,
            JobTree {
                mission: tree.mission.clone().or(mission),
                ..tree
            },
        );
        s.delivered.remove(&job);
    }

    pub(crate) fn set_mission(&self, job: JobId, repos: Vec<String>, checks: Vec<String>) {
        if let Some(t) = lock(&self.state).jobs.get_mut(&job) {
            t.mission = Some((repos, checks));
        }
    }

    /// What was checked and delivered since the last call.
    pub(crate) fn drain(&self) -> Vec<ContractEvent> {
        std::mem::take(&mut lock(&self.state).events)
    }

    /// Jobs whose change breaks repositories outside their mission.
    pub(crate) fn escalations(&self) -> Vec<Escalation> {
        let mut v: Vec<Escalation> = lock(&self.state).escalations.values().cloned().collect();
        v.sort_by_key(|e| e.job.0);
        v
    }

    /// The file a call acts on as `(repository, path in it)`.
    fn locate(tree: &JobTree, rel: &str) -> Option<(String, String)> {
        let fold = cfg!(windows);
        let eq = |a: &str, b: &str| {
            if fold {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        };
        let mut best: Option<(usize, &str, String)> = None;
        for (name, folder) in &tree.repos {
            let rest = if folder == "." {
                Some(rel.to_string())
            } else {
                let prefix = format!("{folder}/");
                (rel.len() > prefix.len() && eq(&rel[..prefix.len()], &prefix))
                    .then(|| rel[prefix.len()..].to_string())
            };
            if let Some(rest) = rest {
                let depth = if folder == "." { 0 } else { folder.len() };
                if best.as_ref().is_none_or(|(d, _, _)| depth >= *d) {
                    best = Some((depth, name, rest));
                }
            }
        }
        best.map(|(_, name, rest)| (name.to_string(), rest))
    }

    /// The note for the file `request` reads or edits, within `cap` tokens:
    /// who consumes what it provides, or the shape of what it uses. Each
    /// line reaches a job once.
    pub fn pull(&self, request: &HookRequest, cap: usize) -> Option<String> {
        let job = request.job_id?;
        let mut s = lock(&self.state);
        let tree = s.jobs.get(&job)?.clone();
        let rel =
            KnowledgeHub::relative_to(&tree.root.to_string_lossy().replace('\\', "/"), request)?;
        let (repo, file) = Self::locate(&tree, &rel)?;
        let none = BTreeSet::new();
        let skip = s.delivered.get(&job).unwrap_or(&none);
        let note = contracts::note_for(&tree.baseline, &repo, &file, skip, cap)?;
        s.delivered
            .entry(job)
            .or_default()
            .extend(note.keys.iter().cloned());
        s.events.push(ContractEvent::Injected {
            job,
            keys: note.keys.clone(),
            tokens: note.tokens,
        });
        Some(note.text)
    }

    fn consumer_checks(
        tree: &JobTree,
        mission_checks: &[String],
        repos: &BTreeSet<String>,
    ) -> Vec<GateCheck> {
        let folder = |name: &str| {
            tree.repos
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, f)| f.clone())
        };
        let mut out = Vec::new();
        for repo in repos {
            let covered = mission_checks
                .iter()
                .any(|c| split_check(c).0 == Some(repo.as_str()));
            if covered {
                continue;
            }
            let commands = tree
                .commands
                .iter()
                .find(|(n, _)| n == repo)
                .map(|(_, c)| c.as_slice())
                .unwrap_or_default();
            let line = CONSUMER_COMMANDS.iter().find_map(|want| {
                commands
                    .iter()
                    .find(|(name, _)| name == want)
                    .map(|(_, line)| line)
            });
            if let Some(line) = line
                && let Some(check) = check_in(&format!("{repo}: {line}"), &|d: &str| folder(d))
            {
                out.push(check);
            }
        }
        out
    }
}

impl ContractGate for ContractHub {
    fn judge(&self, job: JobId) -> Option<ContractJudgement> {
        let (tree, cache) = {
            let s = lock(&self.state);
            let tree = s.jobs.get(&job)?.clone();
            let cache = s.caches.get(&tree.workspace).cloned().unwrap_or_default();
            (tree, cache)
        };
        let started = std::time::Instant::now();
        let scan = scan_tree(&*self.source, &tree.root, &tree.repos, &cache);
        let report = Report::of(&tree.baseline, &scan.index);
        if report.is_empty() {
            let mut s = lock(&self.state);
            s.escalations.remove(&job);
            s.last.remove(&job);
            return None;
        }
        let (scope, mission_checks) = tree.mission.clone().unwrap_or_default();
        let in_scope = |repo: &str| scope.is_empty() || scope.iter().any(|r| r == repo);
        let impacted = report.repos();
        let inside: BTreeSet<String> = impacted.iter().filter(|r| in_scope(r)).cloned().collect();
        let checks = Self::consumer_checks(&tree, &mission_checks, &inside);

        let mut text = format!(
            "contracts: {} provided symbol(s) changed; {} place(s) in {} consume them.",
            report.impacts.len(),
            report.consumers().len(),
            if impacted.is_empty() {
                "no other repository".to_string()
            } else {
                impacted.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        );
        let mut dangling = Vec::new();
        let mut outside = Vec::new();
        let mut outside_repos = BTreeSet::new();
        for i in report.dangling() {
            let places = i
                .consumers
                .iter()
                .map(|s| s.place())
                .collect::<Vec<_>>()
                .join(", ");
            let line = format!(
                "BROKEN: {} ({}) is gone (removed or renamed) but still used by: {places}.",
                i.before.sym.label(),
                i.before.place()
            );
            text.push('\n');
            text.push_str(&line);
            dangling.push(format!("{} <- {places}", i.before.sym.label()));
            for s in i.consumers.iter().filter(|s| !in_scope(&s.repo)) {
                outside.push(s.place());
                outside_repos.insert(s.repo.clone());
            }
        }
        for i in report.reshaped() {
            if let Delta::Changed(now) = &i.delta {
                text.push_str(&format!(
                    "\nCHANGED: {} ({}) is now \"{now}\"; used by: {}. The checks of those repositories judge it.",
                    i.before.sym.label(),
                    i.before.place(),
                    i.consumers.iter().take(6).map(|s| s.place()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if !outside.is_empty() {
            text.push_str(&format!(
                "\nOUTSIDE THE MISSION: {} -- {} isn't part of this mission, so it can't be updated here: revert the change or tell the owner.",
                outside.join(", "),
                outside_repos.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !checks.is_empty() {
            text.push_str(&format!(
                "\nAlso checking: {}.",
                checks
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let text: String = text.chars().take(OUTPUT_MAX).collect();
        let passed = dangling.is_empty();
        let event = ContractEvent::Checked {
            job,
            changed: report.impacts.len(),
            consumers: report.consumers().len(),
            repos: impacted.iter().cloned().collect(),
            dangling,
            outside: outside.clone(),
            passed,
        };
        {
            let mut s = lock(&self.state);
            if s.last.get(&job) != Some(&event) {
                s.events.push(event.clone());
                s.last.insert(job, event);
            }
            if outside.is_empty() {
                s.escalations.remove(&job);
            } else {
                s.escalations.insert(
                    job,
                    Escalation {
                        job,
                        places: outside,
                        repos: outside_repos.into_iter().collect(),
                    },
                );
            }
        }
        Some(ContractJudgement {
            checks,
            run: CheckRun {
                name: "contracts".into(),
                command: "swarmux contracts check".into(),
                exit: Some(i32::from(!passed)),
                timed_out: false,
                spawn_error: None,
                duration_ms: started.elapsed().as_millis() as u64,
                output: text,
                flaky: false,
            },
        })
    }
}

impl ToolContext for ContractHub {
    fn tool_context(&self, request: &HookRequest) -> Option<String> {
        self.pull(request, PULL_TOKENS)
    }
}

/// Knowledge and contracts in one pull: the contracts get what the
/// knowledge left of the 300 tokens a pull may cost.
pub struct PullContext {
    pub knowledge: Arc<dyn ToolContext>,
    pub contracts: Arc<ContractHub>,
}

impl ToolContext for PullContext {
    fn tool_context(&self, request: &HookRequest) -> Option<String> {
        let first = self.knowledge.tool_context(request);
        let used = first.as_deref().map_or(0, estimate_tokens);
        let left = PULL_TOKENS.saturating_sub(used);
        let second = (left >= MIN_PULL_TOKENS)
            .then(|| self.contracts.pull(request, left))
            .flatten();
        match (first, second) {
            (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
            (a, b) => a.or(b),
        }
    }
}
