//! The knowledge the hook transport's threads read: what each running job
//! is, the items, and the guard rules compiled. They answer from it at
//! once; what they delivered or caught waits in a queue for the engine,
//! which writes it to the store and the ledger.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::gate::GateCheck;
use swarmux_domain::knowledge::deliver::{self, Rendered};
use swarmux_domain::knowledge::rule::strictest;
use swarmux_domain::knowledge::{
    Action, CompiledGuard, Enforcer, Item, ItemId, JobScope, PULL_TOKENS, Status, ToolCall,
};
use swarmux_domain::{Decision, JobId, Verdict};

use crate::hookbus::{SessionStartContext, SessionStartInfo};
use crate::ports::HookRequest;

/// Request ids remembered so a held call decided twice is logged once.
const LOGGED_KEPT: usize = 256;

/// What a running job is, for knowledge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobInfo {
    pub scope: JobScope,
    /// The folder the job runs in, `/`-separated: tool paths are taken
    /// relative to it.
    pub root: String,
    /// A workspace repository's name and its folder below the root.
    pub folders: Vec<(String, String)>,
}

/// Something the transport delivered or caught, for the engine to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Usage {
    /// A block reached an agent: `startup`, `clear`, `compact` or `pull`.
    Injected {
        job: JobId,
        trigger: String,
        items: Vec<ItemId>,
        tokens: usize,
    },
    /// A guard rule matched a call (in shadow: would have).
    Guard {
        job: Option<JobId>,
        rule: ItemId,
        action: Action,
        shadow: bool,
        tool: String,
        subject: String,
    },
    /// A gate or lint check of a rule ran.
    Checked {
        job: JobId,
        rule: ItemId,
        passed: bool,
        shadow: bool,
    },
}

/// A check a rule adds to the done-gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleCheck {
    pub rule: ItemId,
    pub check: GateCheck,
    /// Run after the visible checks; only its count is ever told.
    pub hidden: bool,
    /// Its result is logged, never acted on.
    pub shadow: bool,
}

/// Rules the Guard consults before its own policy.
pub trait RuleGuard: Send + Sync {
    /// A deny or an ask from the strictest rule `request` breaks, if any.
    fn check(&self, request: &HookRequest) -> Option<Decision>;
}

/// Context added to a tool call's result: the items it pulls, warnings.
pub trait ToolContext: Send + Sync {
    fn tool_context(&self, request: &HookRequest) -> Option<String>;
}

/// The default [`ToolContext`]: adds nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoToolContext;

impl ToolContext for NoToolContext {
    fn tool_context(&self, _: &HookRequest) -> Option<String> {
        None
    }
}

/// Checks rules add to the done-gate, and where their results go.
pub trait RuleChecks: Send + Sync {
    /// The checks for `job`; hidden ones only for a mission's job.
    fn checks(&self, job: JobId, mission: bool) -> Vec<RuleCheck>;
    fn report(&self, job: JobId, rule: &ItemId, passed: bool, shadow: bool);
}

#[derive(Default)]
struct State {
    items: Vec<Item>,
    guards: Vec<(usize, CompiledGuard)>,
    jobs: HashMap<JobId, JobInfo>,
    delivered: HashMap<JobId, BTreeSet<ItemId>>,
    logged: VecDeque<u64>,
    usage: Vec<Usage>,
}

/// The knowledge shared with the transport's threads.
pub struct KnowledgeHub {
    state: Mutex<State>,
    /// What else sessions start with (the handoff, the board's digest),
    /// after the knowledge.
    inner: Mutex<Option<Arc<dyn SessionStartContext>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether paths are compared without case here.
fn fold() -> bool {
    cfg!(windows)
}

/// Tools whose file path pulls knowledge.
fn reads_or_writes(tool: &str) -> bool {
    matches!(
        tool,
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit"
    )
}

impl KnowledgeHub {
    pub fn new() -> Arc<KnowledgeHub> {
        Arc::new(KnowledgeHub {
            state: Mutex::default(),
            inner: Mutex::new(None),
        })
    }

    /// Sessions start with `inner`'s text after the knowledge.
    pub fn set_inner(&self, inner: Arc<dyn SessionStartContext>) {
        *lock(&self.inner) = Some(inner);
    }

    /// The items as they are now.
    pub fn publish(&self, items: Vec<Item>) {
        let guards = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.acts())
            .filter_map(|(n, i)| match &i.enforcer {
                Some(Enforcer::Guard(g)) => Some((n, g.compile())),
                _ => None,
            })
            .collect();
        let mut s = lock(&self.state);
        s.items = items;
        s.guards = guards;
    }

    pub fn register(&self, job: JobId, info: JobInfo) {
        lock(&self.state).jobs.insert(job, info);
    }

    /// `job` is also of `class` (a mission's job).
    pub fn add_class(&self, job: JobId, class: &str) {
        if let Some(info) = lock(&self.state).jobs.get_mut(&job)
            && !info.scope.classes.iter().any(|c| c == class)
        {
            info.scope.classes.push(class.to_string());
        }
    }

    pub fn job(&self, job: JobId) -> Option<JobInfo> {
        lock(&self.state).jobs.get(&job).cloned()
    }

    /// What was delivered and caught since the last call.
    pub fn drain(&self) -> Vec<Usage> {
        std::mem::take(&mut lock(&self.state).usage)
    }

    /// The pushed block for `job`, without recording it.
    pub fn pushed_for(&self, job: JobId) -> Option<Rendered> {
        let s = lock(&self.state);
        let info = s.jobs.get(&job)?;
        deliver::render_push(&deliver::pushed(&s.items, &info.scope))
    }

    /// The call's path relative to the job's root, `/`-separated.
    fn relative(info: &JobInfo, request: &HookRequest) -> Option<String> {
        Self::relative_to(&info.root, request)
    }

    /// Like [`relative`](Self::relative) for a job that runs in `root`.
    pub(crate) fn relative_to(root: &str, request: &HookRequest) -> Option<String> {
        if request.subject.is_empty() || !reads_or_writes(&request.tool_name) {
            return None;
        }
        let path = unalias(&request.subject.replace('\\', "/"));
        let root = unalias(root.trim_end_matches('/'));
        let root = root.as_str();
        let looks_absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
        if !looks_absolute {
            return Some(path.trim_start_matches("./").to_string());
        }
        let same = |a: &str, b: &str| {
            if fold() {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        };
        let prefix = format!("{root}/");
        (path.len() > prefix.len() && same(&path[..prefix.len()], &prefix))
            .then(|| path[prefix.len()..].to_string())
    }

    /// The text rules look at: the command, or what is being written.
    fn text_of(request: &HookRequest) -> &str {
        if reads_or_writes(&request.tool_name) {
            &request.content
        } else {
            &request.subject
        }
    }

    /// Every guard rule `request` breaks: `(item, action, shadow)`.
    fn hits(s: &State, request: &HookRequest) -> Vec<(usize, Action, bool)> {
        let Some(info) = request.job_id.and_then(|j| s.jobs.get(&j)) else {
            return Vec::new();
        };
        let rel = Self::relative(info, request);
        let call = ToolCall {
            tool: &request.tool_name,
            path: rel.as_deref(),
            text: Self::text_of(request),
        };
        s.guards
            .iter()
            .filter(|(n, _)| {
                let i = &s.items[*n];
                i.home.same(&info.scope.home) && i.scope.applies_to(&info.scope)
            })
            .filter_map(|(n, g)| {
                g.hit(&call, fold())
                    .map(|a| (*n, a, s.items[*n].status == Status::Shadow))
            })
            .collect()
    }
}

impl RuleGuard for KnowledgeHub {
    fn check(&self, request: &HookRequest) -> Option<Decision> {
        let mut s = lock(&self.state);
        let hits = Self::hits(&s, request);
        if hits.is_empty() {
            return None;
        }
        // A held call is decided again when the app takes it in.
        let first = !s.logged.contains(&request.request_id);
        if first {
            if s.logged.len() == LOGGED_KEPT {
                s.logged.pop_front();
            }
            s.logged.push_back(request.request_id);
            for (n, action, shadow) in &hits {
                let rule = s.items[*n].id.clone();
                s.usage.push(Usage::Guard {
                    job: request.job_id,
                    rule,
                    action: *action,
                    shadow: *shadow,
                    tool: request.tool_name.clone(),
                    subject: request.subject.clone(),
                });
            }
        }
        let acting: Vec<(ItemId, Action)> = hits
            .iter()
            .filter(|(_, a, shadow)| !shadow && *a != Action::Warn)
            .map(|(n, a, _)| (s.items[*n].id.clone(), *a))
            .collect();
        let (id, action) = strictest(&acting)?.clone();
        let item = s.items.iter().find(|i| i.id == id)?;
        let message = match &item.enforcer {
            Some(Enforcer::Guard(g)) => g.message.clone(),
            _ => String::new(),
        };
        let broken = s
            .guards
            .iter()
            .find(|(n, _)| s.items[*n].id == id)
            .and_then(|(_, g)| g.broken().map(str::to_string));
        let reason = match broken {
            Some(why) => format!(
                "{} (it could not be evaluated, so the call is refused: {why})",
                deliver::reason(item, &message)
            ),
            None => deliver::reason(item, &message),
        };
        let verdict = match action {
            Action::Deny => Verdict::Deny,
            Action::Ask | Action::Warn => Verdict::Ask,
        };
        Some(Decision::new(verdict, Some(reason)))
    }
}

impl ToolContext for KnowledgeHub {
    fn tool_context(&self, request: &HookRequest) -> Option<String> {
        let job = request.job_id?;
        let mut s = lock(&self.state);
        let warnings: Vec<String> = Self::hits(&s, request)
            .into_iter()
            .filter(|(_, a, shadow)| !shadow && *a == Action::Warn)
            .map(|(n, _, _)| deliver::warning(&s.items[n]))
            .collect();
        let info = s.jobs.get(&job)?.clone();
        let pulled = Self::relative(&info, request).and_then(|rel| {
            let none = BTreeSet::new();
            let delivered = s.delivered.get(&job).unwrap_or(&none);
            let chosen = deliver::pulled(&s.items, &info.scope, &rel, fold(), delivered);
            deliver::render_pull(&chosen, PULL_TOKENS)
        });
        if let Some(block) = &pulled {
            s.delivered
                .entry(job)
                .or_default()
                .extend(block.items.iter().cloned());
            s.usage.push(Usage::Injected {
                job,
                trigger: "pull".into(),
                items: block.items.clone(),
                tokens: block.tokens,
            });
        }
        let mut parts = warnings;
        parts.extend(pulled.map(|b| b.text));
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }
}

impl SessionStartContext for KnowledgeHub {
    fn context(&self, start: &SessionStartInfo) -> Option<String> {
        let inner = lock(&self.inner).clone().and_then(|i| i.context(start));
        // A resumed session has the block in its transcript already.
        let ours = match (start.job, start.source.as_str()) {
            (_, "resume") | (None, _) => None,
            (Some(job), source) => {
                let block = self.pushed_for(job);
                let mut s = lock(&self.state);
                // After a compaction, what was pulled is gone: pull it again.
                if source == "compact" {
                    s.delivered.remove(&job);
                }
                if let Some(b) = &block {
                    s.usage.push(Usage::Injected {
                        job,
                        trigger: if source.is_empty() { "startup" } else { source }.into(),
                        items: b.items.clone(),
                        tokens: b.tokens,
                    });
                }
                block.map(|b| b.text)
            }
        };
        // The knowledge first: it is the same every time, so it caches.
        match (ours, inner) {
            (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
            (a, b) => a.or(b),
        }
    }
}

impl RuleChecks for KnowledgeHub {
    fn checks(&self, job: JobId, mission: bool) -> Vec<RuleCheck> {
        let s = lock(&self.state);
        let Some(info) = s.jobs.get(&job) else {
            return Vec::new();
        };
        let folder = |repo: &str| {
            info.folders
                .iter()
                .find(|(name, _)| name == repo)
                .map(|(_, dir)| dir.clone())
        };
        let mut out = Vec::new();
        for i in s.items.iter().filter(|i| i.acts()) {
            if !i.home.same(&info.scope.home) || !i.scope.applies_to(&info.scope) {
                continue;
            }
            let (rule, hidden) = match &i.enforcer {
                Some(Enforcer::Gate(c)) if mission => (c, true),
                Some(Enforcer::Lint(c)) => (c, false),
                _ => continue,
            };
            if let Some(check) = rule.check(&i.id, hidden, &folder) {
                out.push(RuleCheck {
                    rule: i.id.clone(),
                    check,
                    hidden,
                    shadow: i.status == Status::Shadow,
                });
            }
        }
        out.sort_by(|a, b| a.rule.cmp(&b.rule));
        out
    }

    fn report(&self, job: JobId, rule: &ItemId, passed: bool, shadow: bool) {
        lock(&self.state).usage.push(Usage::Checked {
            job,
            rule: rule.clone(),
            passed,
            shadow,
        });
    }
}

/// `path` with macOS's `/private` prefix dropped from `/var`, `/tmp` and
/// `/etc`, which are links to `/private/...` there: a tool may report either
/// form for the same file.
fn unalias(path: &str) -> String {
    for dir in ["var", "tmp", "etc"] {
        let long = format!("/private/{dir}");
        if path == long || path.starts_with(&format!("{long}/")) {
            return path["/private".len()..].to_string();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod unalias_tests {
    use super::unalias;

    #[test]
    fn macos_private_links_are_the_same_folder() {
        assert_eq!(
            unalias("/private/var/folders/x/a.sql"),
            "/var/folders/x/a.sql"
        );
        assert_eq!(unalias("/private/tmp"), "/tmp");
        assert_eq!(unalias("/private/varnish/a"), "/private/varnish/a");
        assert_eq!(unalias("C:/work/a"), "C:/work/a");
    }
}
