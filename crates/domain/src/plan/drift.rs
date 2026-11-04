//! Drift: how far what was built strayed from the approved plan.
//!
//! Two things are compared, with no model involved. Files changed outside
//! every area the plan named are scope the user never approved. Tasks none
//! of whose areas saw a change are work that was planned and not done.
//! Each is a share (of the changed files, of the tasks); the larger one is
//! the drift score, and a score at or above the threshold is drift.

use crate::guard::glob::{glob_match, normalize};
use crate::json::{Json, quote};

use super::Plan;

/// Three in ten of the changed files outside the plan, or three in ten of the plan
/// tasks untouched, is drift.
pub const DEFAULT_DRIFT_THRESHOLD: f64 = 0.3;
/// Files that follow from any change and say nothing about scope.
const INCIDENTAL_NAMES: [&str; 7] = [
    "Cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "go.sum",
    "Gemfile.lock",
];
/// Folders swarmux writes into itself.
const INCIDENTAL_DIRS: [&str; 1] = [".swarmux/"];
/// Most names kept in a report.
const KEPT: usize = 40;
const EPS: f64 = 1e-9;

/// What comparing a plan with a change found.
#[derive(Clone, Debug, PartialEq)]
pub struct DriftReport {
    /// Files changed, not counting incidental ones (lockfiles).
    pub changed: u32,
    /// Changed files outside every planned area (the first few).
    pub outside: Vec<String>,
    /// How many of the changed files are outside every planned area.
    pub outside_count: u32,
    /// How many tasks the plan has.
    pub tasks: u32,
    /// Ids of the tasks no change matched.
    pub untouched: Vec<String>,
    pub outside_share: f64,
    pub untouched_share: f64,
    pub threshold: f64,
}

impl DriftReport {
    /// The larger of the two shares, `0.0..=1.0`.
    pub fn score(&self) -> f64 {
        self.outside_share.max(self.untouched_share)
    }

    /// The change strayed from the plan by at least the threshold.
    pub fn drifted(&self) -> bool {
        self.score() + EPS >= self.threshold
    }

    /// One line for a list: `3 of 7 changed files outside the plan, 1 of 4 tasks with no change`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.outside_count > 0 {
            parts.push(format!(
                "{} of {} changed files outside the plan",
                self.outside_count, self.changed
            ));
        }
        if !self.untouched.is_empty() {
            parts.push(format!(
                "{} of {} tasks with no change",
                self.untouched.len(),
                self.tasks
            ));
        }
        if parts.is_empty() {
            "the change matches the plan".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// The report as JSON, for keeping.
    pub fn to_json(&self) -> String {
        let list = |items: &[String]| {
            format!(
                "[{}]",
                items.iter().map(|s| quote(s)).collect::<Vec<_>>().join(",")
            )
        };
        format!(
            "{{\"changed\":{},\"outside\":{},\"outside_count\":{},\"tasks\":{},\"untouched\":{},\"outside_share\":{},\"untouched_share\":{},\"threshold\":{}}}",
            self.changed,
            list(&self.outside),
            self.outside_count,
            self.tasks,
            list(&self.untouched),
            num(self.outside_share),
            num(self.untouched_share),
            num(self.threshold),
        )
    }

    pub fn from_json(v: &Json) -> Option<DriftReport> {
        let count = |k: &str| {
            v.get(k)
                .and_then(Json::as_u64)
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        };
        let list = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        };
        let share = |k: &str| v.get(k).and_then(Json::as_f64);
        Some(DriftReport {
            changed: count("changed")?,
            outside: list("outside"),
            outside_count: count("outside_count")?,
            tasks: count("tasks")?,
            untouched: list("untouched"),
            outside_share: share("outside_share")?,
            untouched_share: share("untouched_share")?,
            threshold: share("threshold")?,
        })
    }
}

fn num(x: f64) -> String {
    if x.is_finite() {
        format!("{x}")
    } else {
        "0".into()
    }
}

fn clean(path: &str) -> String {
    let p = path.trim();
    normalize(p).unwrap_or_else(|| p.replace('\\', "/"))
}

/// Whether `path` lies in `area`: a glob it matches, a folder it is in or
/// the file itself. Both are relative to the repository, with `/` or `\`.
pub fn area_matches(area: &str, path: &str) -> bool {
    let (area, path) = (clean(area), clean(path));
    if area.is_empty() {
        return true;
    }
    if area.contains(['*', '?']) {
        return glob_match(&area, &path, false);
    }
    path == area || path.starts_with(&format!("{area}/"))
}

fn incidental(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    INCIDENTAL_NAMES.contains(&name) || INCIDENTAL_DIRS.iter().any(|d| path.starts_with(d))
}

fn share(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// Compares `plan` with the files `changed`. A change with no files is
/// all planned work left undone.
pub fn check_drift(plan: &Plan, changed: &[String], threshold: f64) -> DriftReport {
    let mut files: Vec<String> = changed
        .iter()
        .map(|p| clean(p))
        .filter(|p| !p.is_empty() && !incidental(p))
        .collect();
    files.sort();
    files.dedup();
    let planned = |p: &str| {
        plan.tasks
            .iter()
            .any(|t| t.files.iter().any(|a| area_matches(a, p)))
    };
    let outside: Vec<String> = files.iter().filter(|p| !planned(p)).cloned().collect();
    let untouched: Vec<String> = plan
        .tasks
        .iter()
        .filter(|t| {
            !files
                .iter()
                .any(|f| t.files.iter().any(|a| area_matches(a, f)))
        })
        .map(|t| t.id.clone())
        .collect();
    DriftReport {
        changed: u32::try_from(files.len()).unwrap_or(u32::MAX),
        outside_share: share(outside.len(), files.len()),
        outside_count: u32::try_from(outside.len()).unwrap_or(u32::MAX),
        outside: outside.into_iter().take(KEPT).collect(),
        tasks: u32::try_from(plan.tasks.len()).unwrap_or(u32::MAX),
        untouched_share: share(untouched.len(), plan.tasks.len()),
        untouched,
        threshold: if threshold.is_finite() && threshold > 0.0 {
            threshold.min(1.0)
        } else {
            DEFAULT_DRIFT_THRESHOLD
        },
    }
}
