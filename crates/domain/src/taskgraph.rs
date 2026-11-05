//! Task graph runs: an approved plan's tasks run as separate jobs, in
//! parallel where the graph allows, each inside the files it claims, and
//! merged back in dependency order.
//!
//! This module holds the rules, with no I/O: which topology a plan gets,
//! which tasks may start, whether two claims overlap, whether a write is in
//! scope, which finished task merges next, and the run's state as kept.
//!
//! A task is *ready* when every task it depends on has merged and its
//! claim overlaps no claim held by a task still out. Overlapping claims are
//! never held at once: such tasks run one after the other.

use std::fmt;

use crate::gate::written_paths;
use crate::guard::glob::{absolute, is_windows, normalize, relative};
use crate::json::{Json, quote};
use crate::mission::Contract;
use crate::plan::{Plan, PlanTask, area_matches};
use crate::{JobId, Millis};

/// How a plan's tasks are run.
///
/// Parallel agents pay off on work that splits into independent parts and
/// cost more than they bring on a chain of steps that each need the one
/// before; so a plan where no two tasks can ever run at once stays one
/// session, and anything else fans out along its dependencies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Topology {
    /// One job builds the whole plan (a chain, or a single task).
    #[default]
    Single,
    /// One job per task, started as their dependencies merge.
    Fanout,
}

impl Topology {
    pub fn as_str(self) -> &'static str {
        match self {
            Topology::Single => "single",
            Topology::Fanout => "fanout",
        }
    }

    pub fn parse(s: &str) -> Option<Topology> {
        [Topology::Single, Topology::Fanout]
            .into_iter()
            .find(|t| t.as_str() == s)
    }

    /// The topology for `plan`: fan-out when at least two of its tasks
    /// could run at once.
    pub fn choose(plan: &Plan) -> Topology {
        if width(plan) >= 2 {
            Topology::Fanout
        } else {
            Topology::Single
        }
    }
}

/// Most tasks of one dependency layer whose claims don't overlap.
pub fn width(plan: &Plan) -> usize {
    plan.layers()
        .iter()
        .map(|layer| {
            let mut held: Vec<&PlanTask> = Vec::new();
            for &i in layer {
                let t = &plan.tasks[i];
                if !held.iter().any(|h| claims_overlap(&h.files, &t.files)) {
                    held.push(t);
                }
            }
            held.len()
        })
        .max()
        .unwrap_or(0)
}

/// The literal folder or file an area starts with, before any wildcard;
/// `""` when it starts with one.
fn literal_part(area: &str) -> String {
    let a = area.trim().replace('\\', "/");
    let a = a.strip_prefix("./").unwrap_or(&a);
    let a = normalize(a).unwrap_or_else(|| a.to_string());
    a.trim_matches('/')
        .split('/')
        .take_while(|s| !s.contains(['*', '?', '[']))
        .collect::<Vec<_>>()
        .join("/")
}

fn nests(a: &str, b: &str) -> bool {
    a.is_empty() || b.is_empty() || a == b || b.starts_with(&format!("{a}/"))
}

/// Whether two areas (files, folders or globs) may name the same file.
/// When that can't be told for sure, they overlap: the cost of a wrong
/// "no" is two agents editing one file.
pub fn areas_overlap(a: &str, b: &str) -> bool {
    let (la, lb) = (literal_part(a), literal_part(b));
    nests(&la, &lb) || nests(&lb, &la) || area_matches(a, &lb) || area_matches(b, &la)
}

/// Whether two claims share any file.
pub fn claims_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| areas_overlap(x, y)))
}

/// Whether `rel` (relative to the repository) is inside the claim.
pub fn in_claim(areas: &[String], rel: &str) -> bool {
    areas.iter().any(|a| area_matches(a, rel))
}

/// Targets that are not files.
const NOT_FILES: [&str; 4] = ["/dev/null", "nul", "$null", "/dev/stdout"];

/// The first path a tool call would write outside the claim, if any: a
/// path outside the task's worktree `root`, or inside it but in no claimed
/// area. Calls that write nothing visible are in scope.
pub fn out_of_scope(
    areas: &[String],
    root: &str,
    tool: &str,
    subject: &str,
    cwd: &str,
) -> Option<String> {
    let base = if cwd.trim().is_empty() { root } else { cwd };
    for path in written_paths(tool, subject) {
        let p = path.trim();
        if p.is_empty() || NOT_FILES.iter().any(|n| p.eq_ignore_ascii_case(n)) {
            continue;
        }
        let Some(abs) = absolute(p, base) else {
            return Some(p.to_string());
        };
        let rel = match relative(&abs, root) {
            Some(rel) if !rel.is_empty() => rel,
            _ => return Some(abs),
        };
        let fold = is_windows(root) || is_windows(&abs);
        let inside = if fold {
            let lower = rel.to_ascii_lowercase();
            areas
                .iter()
                .any(|a| area_matches(&a.to_ascii_lowercase(), &lower))
        } else {
            in_claim(areas, &rel)
        };
        if !inside {
            return Some(rel);
        }
    }
    None
}

/// What the agent is told when a write leaves its scope.
pub fn scope_reason(task: &str, areas: &[String], path: &str) -> String {
    format!(
        "{path} is outside task {task}'s scope (scope claim). This task may change only: {}. \
         Other tasks own the rest; if the plan is wrong, say so in your final answer instead.",
        areas.join(", ")
    )
}

/// Where one task of a run stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskState {
    /// Its dependencies have not all merged, or its claim is taken.
    #[default]
    Waiting,
    /// Its job is queued or running.
    Running,
    /// Its job ended; the gate's verdict is read.
    Gate,
    /// Passed, waiting its turn in the merge queue.
    Merging,
    /// In the integration branch.
    Merged,
    /// Its merge conflicted; its agent was sent back once to rebase.
    Conflict,
    /// Needs the user: a second conflict, a failed job or gate, or files
    /// outside its scope.
    Failed,
}

impl TaskState {
    pub const ALL: [TaskState; 7] = [
        TaskState::Waiting,
        TaskState::Running,
        TaskState::Gate,
        TaskState::Merging,
        TaskState::Merged,
        TaskState::Conflict,
        TaskState::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Waiting => "waiting",
            TaskState::Running => "running",
            TaskState::Gate => "gate",
            TaskState::Merging => "merging",
            TaskState::Merged => "merged",
            TaskState::Conflict => "conflict",
            TaskState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<TaskState> {
        TaskState::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// A job is out for it, or its work waits to merge: it holds its claim.
    pub fn holds_claim(self) -> bool {
        matches!(
            self,
            TaskState::Running | TaskState::Gate | TaskState::Merging | TaskState::Conflict
        )
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One task of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskRun {
    pub id: String,
    pub state: TaskState,
    /// Its newest job.
    pub job: Option<JobId>,
    /// The branch its work is on, once known.
    pub branch: String,
    /// Its agent was sent back once to resolve a conflict.
    pub reinvoked: bool,
    /// It failed at the merge (a second conflict, or files outside its
    /// scope): the user is asked.
    pub merge_blocked: bool,
    /// Times its job was picked up again after swarmux restarted.
    pub resumes: u32,
    /// Why it needs the user, or the last thing that happened.
    pub note: String,
    pub started_at: Option<Millis>,
    pub ended_at: Option<Millis>,
}

/// A plan being built as a task graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphRun {
    pub topology: Topology,
    /// The branch the tasks merge into, owned by swarmux.
    pub integration: String,
    /// Where that branch is checked out.
    pub workdir: String,
    pub tasks: Vec<TaskRun>,
    /// Task ids in the order they merged.
    pub merged: Vec<String>,
    /// Writes refused for leaving a task's scope.
    pub denials: u32,
    /// What the tasks' finished jobs cost, in dollars.
    pub spent_usd: f64,
    /// The job the finished mission is verified and delivered as.
    pub final_job: Option<JobId>,
    pub started_at: Millis,
    /// When the integrated result's checks were asked for.
    pub verify_from: Option<Millis>,
}

/// `swarmux/<mission>/integration`.
pub fn integration_branch(mission: &str) -> String {
    format!("swarmux/{}/integration", crate::worktree::slug(mission))
}

impl GraphRun {
    pub fn new(plan: &Plan, integration: &str, now: Millis) -> GraphRun {
        GraphRun {
            topology: Topology::Fanout,
            integration: integration.to_string(),
            tasks: plan
                .tasks
                .iter()
                .map(|t| TaskRun {
                    id: t.id.clone(),
                    ..TaskRun::default()
                })
                .collect(),
            started_at: now,
            ..GraphRun::default()
        }
    }

    pub fn task(&self, id: &str) -> Option<&TaskRun> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn task_mut(&mut self, id: &str) -> Option<&mut TaskRun> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// The task whose newest job is `job`.
    pub fn task_of(&self, job: JobId) -> Option<&TaskRun> {
        self.tasks.iter().find(|t| t.job == Some(job))
    }

    fn state_of(&self, id: &str) -> TaskState {
        self.task(id).map_or(TaskState::Waiting, |t| t.state)
    }

    /// Tasks that may start now, in the plan's dependency order: waiting,
    /// every dependency merged, and a claim that overlaps no claim held
    /// (nor one picked before it).
    pub fn ready(&self, plan: &Plan) -> Vec<String> {
        let mut held: Vec<&[String]> = plan
            .tasks
            .iter()
            .filter(|t| self.state_of(&t.id).holds_claim())
            .map(|t| t.files.as_slice())
            .collect();
        let mut out = Vec::new();
        for i in plan.layers().into_iter().flatten() {
            let t = &plan.tasks[i];
            if self.state_of(&t.id) != TaskState::Waiting {
                continue;
            }
            if !t
                .depends_on
                .iter()
                .all(|d| self.state_of(d) == TaskState::Merged)
            {
                continue;
            }
            if held.iter().any(|h| claims_overlap(h, &t.files)) {
                continue;
            }
            held.push(&t.files);
            out.push(t.id.clone());
        }
        out
    }

    /// The merge queue: passed tasks in producer → consumer order.
    pub fn queue(&self, plan: &Plan) -> Vec<String> {
        plan.layers()
            .into_iter()
            .flatten()
            .map(|i| &plan.tasks[i].id)
            .filter(|id| self.state_of(id) == TaskState::Merging)
            .cloned()
            .collect()
    }

    /// The task to merge next: the first in the queue whose producers all
    /// merged.
    pub fn next_merge(&self, plan: &Plan) -> Option<String> {
        self.queue(plan).into_iter().find(|id| {
            plan.tasks.iter().find(|t| &t.id == id).is_some_and(|t| {
                t.depends_on
                    .iter()
                    .all(|d| self.state_of(d) == TaskState::Merged)
            })
        })
    }

    /// Every task merged.
    pub fn all_merged(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|t| t.state == TaskState::Merged)
    }

    /// Some task needs the user.
    pub fn blocked(&self) -> bool {
        self.tasks.iter().any(|t| t.state == TaskState::Failed)
    }

    /// No task is out or waiting to merge.
    pub fn idle(&self) -> bool {
        !self.tasks.iter().any(|t| t.state.holds_claim())
    }

    /// Nothing more can happen without the user: a task needs them and
    /// no other is out or can start.
    pub fn stuck(&self, plan: &Plan) -> bool {
        self.blocked() && self.idle() && self.ready(plan).is_empty()
    }

    /// Counts per state, for a summary line.
    pub fn counts(&self) -> Vec<(TaskState, usize)> {
        TaskState::ALL
            .into_iter()
            .map(|s| (s, self.tasks.iter().filter(|t| t.state == s).count()))
            .filter(|(_, n)| *n > 0)
            .collect()
    }

    /// The run as JSON, for keeping.
    pub fn to_json(&self) -> String {
        let opt_job = |j: Option<JobId>| j.map_or_else(|| "null".to_string(), |j| j.0.to_string());
        let opt_ms = |m: Option<Millis>| m.map_or_else(|| "null".to_string(), |m| m.to_string());
        let tasks: Vec<String> = self
            .tasks
            .iter()
            .map(|t| {
                format!(
                    "{{\"id\":{},\"state\":{},\"job\":{},\"branch\":{},\"reinvoked\":{},\"merge_blocked\":{},\"resumes\":{},\"note\":{},\"started_at\":{},\"ended_at\":{}}}",
                    quote(&t.id),
                    quote(t.state.as_str()),
                    opt_job(t.job),
                    quote(&t.branch),
                    t.reinvoked,
                    t.merge_blocked,
                    t.resumes,
                    quote(&t.note),
                    opt_ms(t.started_at),
                    opt_ms(t.ended_at),
                )
            })
            .collect();
        let merged: Vec<String> = self.merged.iter().map(|s| quote(s)).collect();
        format!(
            "{{\"topology\":{},\"integration\":{},\"workdir\":{},\"tasks\":[{}],\"merged\":[{}],\"denials\":{},\"spent_usd\":{},\"final_job\":{},\"started_at\":{},\"verify_from\":{}}}",
            quote(self.topology.as_str()),
            quote(&self.integration),
            quote(&self.workdir),
            tasks.join(","),
            merged.join(","),
            self.denials,
            if self.spent_usd.is_finite() {
                self.spent_usd
            } else {
                0.0
            },
            opt_job(self.final_job),
            self.started_at,
            opt_ms(self.verify_from),
        )
    }

    pub fn from_json(v: &Json) -> Option<GraphRun> {
        let text = |v: &Json, k: &str| {
            v.get(k)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let int = |v: &Json, k: &str| {
            v.get(k)
                .and_then(Json::as_f64)
                .filter(|x| x.is_finite())
                .map(|x| x as i64)
        };
        let tasks = v
            .get("tasks")
            .and_then(Json::as_array)?
            .iter()
            .map(|t| {
                Some(TaskRun {
                    id: text(t, "id"),
                    state: TaskState::parse(t.get("state")?.as_str()?)?,
                    job: int(t, "job").map(JobId),
                    branch: text(t, "branch"),
                    reinvoked: t.get("reinvoked").and_then(Json::as_bool).unwrap_or(false),
                    merge_blocked: t
                        .get("merge_blocked")
                        .and_then(Json::as_bool)
                        .unwrap_or(false),
                    resumes: t
                        .get("resumes")
                        .and_then(Json::as_u64)
                        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
                    note: text(t, "note"),
                    started_at: int(t, "started_at"),
                    ended_at: int(t, "ended_at"),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(GraphRun {
            topology: v
                .get("topology")
                .and_then(Json::as_str)
                .and_then(Topology::parse)
                .unwrap_or(Topology::Fanout),
            integration: text(v, "integration"),
            workdir: text(v, "workdir"),
            tasks,
            merged: v
                .get("merged")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect(),
            denials: v
                .get("denials")
                .and_then(Json::as_u64)
                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            spent_usd: v.get("spent_usd").and_then(Json::as_f64).unwrap_or(0.0),
            final_job: int(v, "final_job").map(JobId),
            started_at: int(v, "started_at").unwrap_or(0),
            verify_from: int(v, "verify_from"),
        })
    }
}

/// What one task's job is asked: the mission, the whole plan for context,
/// and the one task it does, inside its claim.
pub fn task_brief(contract: &Contract, plan: &Plan, task: &PlanTask) -> String {
    let mut p = format!(
        "You are one of several agents building an approved plan, each in its own \
         worktree. Do only your task.\n\nMission goal:\n{}\n",
        contract.goal.trim()
    );
    if !plan.summary.is_empty() {
        p.push_str(&format!("\nThe plan: {}\n", plan.summary));
    }
    let others: Vec<String> = plan
        .tasks
        .iter()
        .filter(|t| t.id != task.id)
        .map(|t| format!("- {} {} [{}]", t.id, t.title, t.files.join(", ")))
        .collect();
    if !others.is_empty() {
        p.push_str("\nOther tasks, done by other agents (don't do them):\n");
        p.push_str(&others.join("\n"));
        p.push('\n');
    }
    p.push_str(&format!("\nYour task: {} {}\n", task.id, task.title));
    if !task.acceptance.is_empty() {
        p.push_str(&format!("Done when: {}\n", task.acceptance));
    }
    p.push_str(&format!(
        "Your scope: you may change only {}. Writes anywhere else are refused. \
         If the task can't be done inside it, stop and say why.\n",
        task.files.join(", ")
    ));
    if !contract.constraints.protected.is_empty() {
        p.push_str(&format!(
            "Never change: {}\n",
            contract.constraints.protected.join(", ")
        ));
    }
    p.push_str("Commit nothing yourself: swarmux commits and merges your work when you finish.\n");
    p
}

/// What a task's agent is told when its work conflicts with what merged
/// before it.
pub fn conflict_brief(task: &PlanTask, integration: &str, files: &[String]) -> String {
    let mut p = format!(
        "Your work on task {} could not merge: it conflicts with work other tasks merged \
         into {integration}.",
        task.id
    );
    if !files.is_empty() {
        p.push_str(&format!(" Conflicting files: {}.", files.join(", ")));
    }
    p.push_str(&format!(
        " In this worktree, commit what you have, run `git rebase {integration}`, resolve the \
         conflicts keeping both sides' intent, and finish the rebase. Stay inside your scope ({}). \
         When the rebase is done and the task still works, stop.\n",
        task.files.join(", ")
    ));
    p
}

#[cfg(test)]
mod tests;
