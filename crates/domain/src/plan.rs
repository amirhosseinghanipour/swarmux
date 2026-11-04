//! Plan-first missions: the plan an agent proposes before it builds, as a
//! task graph, and what a person decides about it.
//!
//! A plan is a short summary and tasks. Each task names the files or areas
//! it expects to touch, what it depends on and how its result is checked.
//! The graph is validated here (unique ids, known dependencies, no cycles)
//! so a plan that reaches the user is always one a scheduler could follow.
//! Once the work is built, [`drift`] compares the approved plan with what
//! changed.

pub mod drift;

use std::collections::HashSet;
use std::fmt;

use crate::json::{Json, quote};
use crate::ledger::{hex, sha256};
use crate::mission::Contract;

pub use drift::{DEFAULT_DRIFT_THRESHOLD, DriftReport, area_matches, check_drift};

/// Most tasks a plan may have.
pub const MAX_TASKS: usize = 30;
/// Most files or areas named by one task.
const MAX_FILES: usize = 24;
/// Longest id, in characters.
const ID_MAX: usize = 24;
/// Longest title, summary and acceptance text, in characters.
const TITLE_MAX: usize = 140;
const SUMMARY_MAX: usize = 600;
/// Longest file or area, in characters.
const PATH_MAX: usize = 200;
/// Opens the plan in a build prompt; reviewers look for it.
pub const PLAN_MARKER: &str = "Approved plan:";

/// The structured answer a planning run is asked for, as a JSON schema.
pub const PLAN_SCHEMA: &str = r#"{"type":"object","properties":{"summary":{"type":"string"},"tasks":{"type":"array","minItems":1,"maxItems":30,"items":{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"files":{"type":"array","minItems":1,"items":{"type":"string"}},"depends_on":{"type":"array","items":{"type":"string"}},"acceptance":{"type":"string"}},"required":["id","title","files","depends_on","acceptance"],"additionalProperties":false}}},"required":["summary","tasks"],"additionalProperties":false}"#;

/// One step of a plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    /// Files, folders or globs the task is expected to change.
    pub files: Vec<String>,
    /// Ids of the tasks that must be done first.
    pub depends_on: Vec<String>,
    /// How its result is checked.
    pub acceptance: String,
}

/// A plan and its task graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub summary: String,
    pub tasks: Vec<PlanTask>,
}

/// Why a plan is not usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    NoTasks,
    TooMany(usize),
    BadId(String),
    DuplicateId(String),
    NoFiles(String),
    UnknownDependency {
        task: String,
        dep: String,
    },
    SelfDependency(String),
    /// Tasks that wait on each other (or on something that does).
    Cycle(Vec<String>),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NoTasks => f.write_str("the plan has no tasks"),
            PlanError::TooMany(n) => write!(f, "the plan has {n} tasks; at most {MAX_TASKS}"),
            PlanError::BadId(id) => write!(f, "task id {id:?} is empty or too long"),
            PlanError::DuplicateId(id) => write!(f, "task id {id} is used twice"),
            PlanError::NoFiles(id) => write!(f, "task {id} names no files or areas"),
            PlanError::UnknownDependency { task, dep } => {
                write!(f, "task {task} depends on {dep}, which is not in the plan")
            }
            PlanError::SelfDependency(id) => write!(f, "task {id} depends on itself"),
            PlanError::Cycle(ids) => write!(f, "tasks wait on each other: {}", ids.join(", ")),
        }
    }
}

impl Plan {
    /// Checks that the graph is usable: ids unique, every task naming at
    /// least one file or area, dependencies known and acyclic.
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.tasks.is_empty() {
            return Err(PlanError::NoTasks);
        }
        if self.tasks.len() > MAX_TASKS {
            return Err(PlanError::TooMany(self.tasks.len()));
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for t in &self.tasks {
            if t.id.trim().is_empty() || t.id.chars().count() > ID_MAX {
                return Err(PlanError::BadId(t.id.clone()));
            }
            if !seen.insert(t.id.as_str()) {
                return Err(PlanError::DuplicateId(t.id.clone()));
            }
            if t.files.iter().all(|f| f.trim().is_empty()) {
                return Err(PlanError::NoFiles(t.id.clone()));
            }
        }
        for t in &self.tasks {
            for dep in &t.depends_on {
                if dep == &t.id {
                    return Err(PlanError::SelfDependency(t.id.clone()));
                }
                if !seen.contains(dep.as_str()) {
                    return Err(PlanError::UnknownDependency {
                        task: t.id.clone(),
                        dep: dep.clone(),
                    });
                }
            }
        }
        match self.order() {
            Ok(_) => Ok(()),
            Err(stuck) => Err(PlanError::Cycle(stuck)),
        }
    }

    /// The tasks' indexes with their depth, dependencies first. The ids
    /// stuck in a cycle when there is one. Expects known dependencies.
    fn order(&self) -> Result<Vec<(usize, usize)>, Vec<String>> {
        let n = self.tasks.len();
        let index = |id: &str| self.tasks.iter().position(|t| t.id == id);
        let deps: Vec<Vec<usize>> = self
            .tasks
            .iter()
            .map(|t| t.depends_on.iter().filter_map(|d| index(d)).collect())
            .collect();
        let mut depth: Vec<Option<usize>> = vec![None; n];
        let mut out = Vec::with_capacity(n);
        loop {
            let mut progressed = false;
            for i in 0..n {
                if depth[i].is_some() {
                    continue;
                }
                let ready: Option<usize> = deps[i]
                    .iter()
                    .try_fold(0usize, |top, &d| depth[d].map(|x| top.max(x + 1)));
                if let Some(d) = ready {
                    depth[i] = Some(d);
                    out.push((i, d));
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        if out.len() == n {
            return Ok(out);
        }
        Err((0..n)
            .filter(|i| depth[*i].is_none())
            .map(|i| self.tasks[i].id.clone())
            .collect())
    }

    /// Tasks grouped by depth: each group depends only on the groups
    /// before it. Empty for a plan with a cycle.
    pub fn layers(&self) -> Vec<Vec<usize>> {
        let Ok(order) = self.order() else {
            return Vec::new();
        };
        let top = order.iter().map(|(_, d)| *d).max().map_or(0, |d| d + 1);
        let mut layers = vec![Vec::new(); top];
        for (i, d) in order {
            layers[d].push(i);
        }
        layers
    }

    /// The tasks as rows of a compact tree: by layer, each with how deep
    /// it sits and what it waits for.
    pub fn tree(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for (depth, layer) in self.layers().into_iter().enumerate() {
            for i in layer {
                let t = &self.tasks[i];
                rows.push(TreeRow {
                    depth,
                    id: t.id.clone(),
                    title: t.title.clone(),
                    after: t.depends_on.clone(),
                    files: t.files.clone(),
                });
            }
        }
        rows
    }

    /// Every file or area the plan names, without repeats.
    pub fn areas(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for f in self.tasks.iter().flat_map(|t| t.files.iter()) {
            if !out.contains(&f.as_str()) {
                out.push(f);
            }
        }
        out
    }

    /// The plan as JSON, in the shape of [`PLAN_SCHEMA`].
    pub fn to_json(&self) -> String {
        let list = |items: &[String]| {
            format!(
                "[{}]",
                items.iter().map(|s| quote(s)).collect::<Vec<_>>().join(",")
            )
        };
        let tasks: Vec<String> = self
            .tasks
            .iter()
            .map(|t| {
                format!(
                    "{{\"id\":{},\"title\":{},\"files\":{},\"depends_on\":{},\"acceptance\":{}}}",
                    quote(&t.id),
                    quote(&t.title),
                    list(&t.files),
                    list(&t.depends_on),
                    quote(&t.acceptance),
                )
            })
            .collect();
        format!(
            "{{\"summary\":{},\"tasks\":[{}]}}",
            quote(&self.summary),
            tasks.join(",")
        )
    }

    /// A short digest of the plan, hex: what an approval vouches for.
    pub fn digest(&self) -> String {
        hex(&sha256(self.to_json().as_bytes()))
    }
}

/// One line of the plan's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Layers it sits below the tasks that wait for nothing.
    pub depth: usize,
    pub id: String,
    pub title: String,
    /// What it waits for.
    pub after: Vec<String>,
    pub files: Vec<String>,
}

fn cut(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s,
    }
}

fn strings(v: Option<&Json>, max_len: usize, max_n: usize) -> Vec<String> {
    v.and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Json::as_str)
        .map(|s| cut(s, max_len))
        .filter(|s| !s.is_empty())
        .take(max_n)
        .collect()
}

/// Reads a plan from its JSON value and validates it.
pub fn plan_from_json(v: &Json) -> Result<Plan, String> {
    let tasks = v
        .get("tasks")
        .and_then(Json::as_array)
        .ok_or("the plan has no tasks list")?;
    let plan = Plan {
        summary: cut(
            v.get("summary").and_then(Json::as_str).unwrap_or_default(),
            SUMMARY_MAX,
        ),
        tasks: tasks
            .iter()
            .map(|t| PlanTask {
                id: cut(
                    t.get("id").and_then(Json::as_str).unwrap_or_default(),
                    ID_MAX + 1,
                ),
                title: cut(
                    t.get("title").and_then(Json::as_str).unwrap_or_default(),
                    TITLE_MAX,
                ),
                files: strings(t.get("files"), PATH_MAX, MAX_FILES),
                depends_on: strings(t.get("depends_on"), ID_MAX + 1, MAX_TASKS),
                acceptance: cut(
                    t.get("acceptance")
                        .and_then(Json::as_str)
                        .unwrap_or_default(),
                    TITLE_MAX,
                ),
            })
            .collect(),
    };
    plan.validate().map_err(|e| e.to_string())?;
    Ok(plan)
}

/// Reads a plan from text: a JSON object, bare or inside prose or a fence.
pub fn parse_plan(text: &str) -> Result<Plan, String> {
    let text = text.trim();
    if let Ok(v) = Json::parse(text) {
        return plan_from_json(&v);
    }
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else {
        return Err("the answer has no JSON".into());
    };
    let v = Json::parse(&text[start..=end]).map_err(|e| format!("the answer's JSON: {e}"))?;
    plan_from_json(&v)
}

/// What the planning run is asked. `previous` and `note` carry a
/// rejected plan and why, for a revision.
pub fn plan_prompt(contract: &Contract, previous: Option<&Plan>, note: &str) -> String {
    let c = contract;
    let mut p = String::from(
        "You are planning a task before anyone builds it. Explore the repository read-only \
         and do not change anything. Answer only through the structured output: a plan \
         and its task graph.\n\n",
    );
    p.push_str(&format!("Goal:\n{}\n", c.goal.trim()));
    if !c.acceptance.checks.is_empty() {
        p.push_str("\nIt is done when these pass:\n");
        for check in &c.acceptance.checks {
            p.push_str(&format!("- {check}\n"));
        }
    }
    if !c.constraints.paths_rw.is_empty() {
        p.push_str(&format!(
            "\nWork in: {}\n",
            c.constraints.paths_rw.join(", ")
        ));
    }
    if !c.constraints.protected.is_empty() {
        p.push_str(&format!(
            "\nNever change: {}\n",
            c.constraints.protected.join(", ")
        ));
    }
    for q in c
        .clarifications
        .iter()
        .filter(|q| !q.answer.trim().is_empty())
    {
        p.push_str(&format!(
            "\nThe user said: {} {}\n",
            q.question.trim(),
            q.answer.trim()
        ));
    }
    p.push_str(
        "\nThe plan:\n\
         - summary: two sentences on the approach.\n\
         - tasks: small steps, at most 12, ids t1, t2, ...\n\
         - files: the files, folders (end them with /**) or globs each task expects to \
         change, tests included. Be specific: they are compared with the real change later.\n\
         - depends_on: ids of tasks that must be done first; leave it empty for tasks that \
         can run in parallel. No cycles.\n\
         - acceptance: one line on how that task's result is checked.\n",
    );
    if let Some(old) = previous {
        p.push_str(&format!(
            "\nAn earlier plan was rejected:\n{}\n",
            old.to_json()
        ));
        if !note.trim().is_empty() {
            p.push_str(&format!("The user's note: {}\n", note.trim()));
        }
        p.push_str("Make a new plan that answers the note.\n");
    }
    p
}

/// The approved plan, as the part of a build prompt that carries it.
pub fn plan_brief(plan: &Plan) -> String {
    let mut p = format!("{PLAN_MARKER} the user approved this plan. Follow it.\n");
    if !plan.summary.is_empty() {
        p.push_str(&format!("{}\n", plan.summary));
    }
    for t in &plan.tasks {
        p.push_str(&format!("- {} {}", t.id, t.title));
        if !t.depends_on.is_empty() {
            p.push_str(&format!(" (after {})", t.depends_on.join(", ")));
        }
        p.push_str(&format!(" [files: {}]", t.files.join(", ")));
        if !t.acceptance.is_empty() {
            p.push_str(&format!(" done when: {}", t.acceptance));
        }
        p.push('\n');
    }
    p.push_str(
        "Keep to the planned files. If the plan turns out wrong, say so and why instead of \
         working outside it: swarmux compares the change with the plan when you finish.\n",
    );
    p
}

/// Where a mission's plan stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanStatus {
    /// None asked for yet.
    #[default]
    Idle,
    /// A planning run is out.
    Planning,
    /// A plan waits for the user's decision.
    Proposed,
    Approved,
    /// The planning run gave no usable plan; the note says why.
    Failed,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Idle => "idle",
            PlanStatus::Planning => "planning",
            PlanStatus::Proposed => "proposed",
            PlanStatus::Approved => "approved",
            PlanStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<PlanStatus> {
        [
            PlanStatus::Idle,
            PlanStatus::Planning,
            PlanStatus::Proposed,
            PlanStatus::Approved,
            PlanStatus::Failed,
        ]
        .into_iter()
        .find(|p| p.as_str() == s)
    }
}

/// A mission's plan: the setting, the plan itself and what became of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanRecord {
    /// Set for this mission; the configured default applies when `None`.
    pub plan_first: Option<bool>,
    pub status: PlanStatus,
    pub plan: Option<Plan>,
    /// The user's note on the last rejection, or why planning failed.
    pub note: String,
    /// Plans asked for so far.
    pub rounds: u32,
    pub cost_usd: f64,
    /// Plan against change, once the build was checked.
    pub drift: Option<DriftReport>,
}

impl PlanRecord {
    /// The record as JSON, for keeping.
    pub fn encode(&self) -> String {
        let plan = self
            .plan
            .as_ref()
            .map_or_else(|| "null".to_string(), Plan::to_json);
        let drift = self
            .drift
            .as_ref()
            .map_or_else(|| "null".to_string(), DriftReport::to_json);
        let first = match self.plan_first {
            Some(true) => "true",
            Some(false) => "false",
            None => "null",
        };
        format!(
            "{{\"plan_first\":{first},\"status\":{},\"plan\":{plan},\"note\":{},\"rounds\":{},\"cost_usd\":{},\"drift\":{drift}}}",
            quote(self.status.as_str()),
            quote(&self.note),
            self.rounds,
            if self.cost_usd.is_finite() {
                self.cost_usd
            } else {
                0.0
            },
        )
    }

    pub fn decode(text: &str) -> Result<PlanRecord, String> {
        let v = Json::parse(text)?;
        let status = v
            .get("status")
            .and_then(Json::as_str)
            .and_then(PlanStatus::parse)
            .ok_or("unknown plan status")?;
        let plan = match v.get("plan") {
            Some(p @ Json::Obj(_)) => Some(plan_from_json(p)?),
            _ => None,
        };
        let drift = match v.get("drift") {
            Some(d @ Json::Obj(_)) => DriftReport::from_json(d),
            _ => None,
        };
        Ok(PlanRecord {
            plan_first: v.get("plan_first").and_then(Json::as_bool),
            status,
            plan,
            note: v
                .get("note")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            rounds: v
                .get("rounds")
                .and_then(Json::as_u64)
                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            cost_usd: v.get("cost_usd").and_then(Json::as_f64).unwrap_or(0.0),
            drift,
        })
    }

    /// The approved plan, when there is one.
    pub fn approved(&self) -> Option<&Plan> {
        self.plan
            .as_ref()
            .filter(|_| self.status == PlanStatus::Approved)
    }
}

#[cfg(test)]
mod tests;
