//! The independent verifier's rules: what an agent is told before it
//! starts, how hidden checks are named and counted, which changes to tests
//! look like gaming them, and when a change is risky enough to be read by
//! the reviewer.
//!
//! The verifier itself runs the gate's checks in a clean copy of the work,
//! made by swarmux outside the agent's worktree, then the repository's
//! hidden checks, which the agent never sees. Only how many hidden checks
//! ran and passed leaves the verifier; their names, commands, files and
//! output stay with the user.

use std::collections::HashSet;

use crate::alarm::{Reversibility, RiskInputs, RiskTier, assess};
use crate::gate::{GateCheck, Pin, PinChange, protected_glob};

/// Folders below `.swarmux` that swarmux itself writes while agents run.
const OURS: [&str; 2] = [".swarmux/missions/", ".swarmux/evidence/"];

/// The protected files a diff changes, adds or removes: the gate's pins as
/// git sees them, so line-ending conversions are no change and committed
/// edits count as much as uncommitted ones. Files swarmux writes itself
/// are left out.
pub fn protected_changes(diff: &str, globs: &[String], fold: bool) -> Vec<PinChange> {
    let guarded = |p: &str| {
        !OURS.iter().any(|o| p.starts_with(o)) && protected_glob(globs, p, fold).is_some()
    };
    let mut out = Vec::new();
    for f in parse_diff(diff) {
        // A protected file moved away is a protected file removed.
        if let Some(old) = f.renamed_from.as_deref().filter(|o| guarded(o)) {
            out.push(PinChange::Removed(old.to_string()));
        }
        if !guarded(&f.path) {
            continue;
        }
        out.push(if f.created || f.renamed_from.is_some() {
            PinChange::Added(f.path)
        } else if f.deleted {
            PinChange::Removed(f.path)
        } else {
            PinChange::Changed(f.path)
        });
    }
    out.sort();
    out.dedup();
    out
}

/// What every verified job is told up front.
pub const ANTI_EXPLOIT_PREAMBLE: &str = "How your work is judged: swarmux runs the checks itself, in a clean copy of what you leave behind, plus checks you cannot see. So:\n\
- Never edit, delete, rename or weaken existing tests, fixtures or expected outputs to make them pass.\n\
- Never switch a test off: no skip or ignore markers, no `.only`, no commenting tests out.\n\
- Never special-case the inputs a test uses or detect that code runs under test; solve the problem in general.\n\
- If a check looks wrong, stop and say so instead of working around it.";

/// Longest name of a repository's or a check's holdout folder.
const NAME_MAX: usize = 64;
/// Longest line quoted from a diff.
const LINE_MAX: usize = 120;

/// How the hidden checks went: counts only, never which or why.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HiddenTally {
    pub ran: u32,
    pub passed: u32,
}

impl HiddenTally {
    pub fn failed(&self) -> u32 {
        self.ran.saturating_sub(self.passed)
    }

    pub fn clean(&self) -> bool {
        self.failed() == 0
    }

    /// `2/3`.
    pub fn text(&self) -> String {
        format!("{}/{}", self.passed, self.ran)
    }
}

/// One hidden check: the command, and the files it brings into the clean
/// copy with their content hashes as they were when it was added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldoutCheck {
    pub check: GateCheck,
    /// Paths relative to the repository root, `/`-separated.
    pub files: Vec<Pin>,
}

impl HoldoutCheck {
    pub fn name(&self) -> &str {
        &self.check.name
    }
}

/// Whether `name` can name a holdout folder: letters, digits, `-`, `_`
/// and `.`, not starting with a dot.
pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > NAME_MAX {
        return Err(format!("{name:?} must be 1 to {NAME_MAX} characters"));
    }
    if name.starts_with('.') {
        return Err(format!("{name:?} may not start with a dot"));
    }
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    {
        return Err(format!("{name:?} may not contain {c:?}"));
    }
    Ok(())
}

/// The holdout name of the repository whose top folder is `top`: its last
/// path segment, lowercased, with anything but letters, digits, `-`, `_`
/// and `.` turned into `-`.
pub fn repo_key(top: &str) -> Option<String> {
    let last: String = top
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()?
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let last = last.trim_start_matches('.');
    let last: String = last.chars().take(NAME_MAX).collect();
    valid_name(&last).ok().map(|_| last)
}

/// Where a hidden check's file goes, checked: relative, `/`-separated,
/// without `..` and outside `.git` and `.swarmux`.
pub fn holdout_path(path: &str) -> Result<String, String> {
    let p = path.trim().replace('\\', "/");
    let p = p.trim_start_matches("./");
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.as_bytes().get(1) == Some(&b':')
        || p.split('/').any(|s| s.is_empty() || s == "." || s == "..");
    if bad {
        return Err(format!(
            "{path:?} must be a path inside the repository, like tests/hidden.rs"
        ));
    }
    let first = p.split('/').next().unwrap_or_default().to_ascii_lowercase();
    if first == ".git" || first == ".swarmux" {
        return Err(format!("{path:?} may not go into {first}"));
    }
    Ok(p.to_string())
}

/// A change to the tests that looks like gaming them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tamper {
    /// Lines of an existing test were removed or changed.
    TestEdited { path: String, lines: u32 },
    /// A test was switched off.
    Skipped { path: String, line: String },
}

impl Tamper {
    pub fn path(&self) -> &str {
        match self {
            Tamper::TestEdited { path, .. } | Tamper::Skipped { path, .. } => path,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Tamper::TestEdited { path, lines } => format!(
                "{path}: {lines} line{} of existing tests removed or changed",
                if *lines == 1 { "" } else { "s" }
            ),
            Tamper::Skipped { path, line } => {
                format!("{path}: a test was switched off (`{line}`)")
            }
        }
    }
}

/// Markers that switch a test off, or run some tests and not the rest.
const SKIP_MARKERS: [&str; 27] = [
    "#[ignore",
    "@pytest.mark.skip",
    "@pytest.mark.xfail",
    "pytest.skip(",
    "@unittest.skip",
    "unittest.skip(",
    ".skipTest(",
    "t.Skip(",
    "t.Skipf(",
    "t.SkipNow(",
    "it.skip(",
    "describe.skip(",
    "test.skip(",
    "it.only(",
    "describe.only(",
    "test.only(",
    "xit(",
    "xdescribe(",
    "xtest(",
    "@Ignore",
    "@Disabled",
    "[Ignore",
    "(Skip =",
    "(Skip=",
    "#[cfg(any())]",
    "if: false",
    "--skip",
];

/// Folders whose files are tests.
const TEST_DIRS: [&str; 6] = ["tests", "test", "__tests__", "spec", "specs", "testdata"];

/// Whether `path` (relative, `/`-separated) holds tests.
pub fn is_test_path(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let mut segs: Vec<&str> = low.split('/').collect();
    let Some(file) = segs.pop() else {
        return false;
    };
    if segs.iter().any(|s| TEST_DIRS.contains(s)) {
        return true;
    }
    let stem = file.split('.').next().unwrap_or(file);
    stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("_spec")
        || file.contains(".test.")
        || file.contains(".spec.")
        || (stem.ends_with("test") && file.ends_with(".java"))
        || (stem.ends_with("tests") && file.ends_with(".cs"))
        || stem == "conftest"
}

/// Files whose added lines are not code: a skip marker there is prose.
fn is_prose(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    [".md", ".txt", ".rst", ".adoc"]
        .iter()
        .any(|ext| low.ends_with(ext))
}

/// A removed line that says nothing about what a test checks.
fn trivial(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.starts_with("//")
        || (t.starts_with('#') && !t.starts_with("#["))
        || t.starts_with("use ")
        || t.starts_with("import ")
        || t.starts_with("from ")
        || t.starts_with("require(")
        || matches!(t, "{" | "}" | "};" | ")" | ");" | "]" | "],")
}

/// One file's part of a diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub created: bool,
    pub deleted: bool,
    /// The path it had before a rename.
    pub renamed_from: Option<String>,
}

fn diff_path(raw: &str) -> String {
    let p = raw.trim().trim_matches('"');
    let p = p
        .strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p);
    p.to_string()
}

/// The files of a unified git diff with their added and removed lines.
pub fn parse_diff(diff: &str) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            in_hunk = false;
            // `a/x b/x`: the new path until `+++` says otherwise.
            let path = rest
                .rsplit_once(" b/")
                .map_or_else(|| diff_path(rest), |(_, b)| b.trim_matches('"').to_string());
            out.push(FileDiff {
                path,
                ..FileDiff::default()
            });
            continue;
        }
        let Some(file) = out.last_mut() else {
            continue;
        };
        if !in_hunk {
            if line.starts_with("@@") {
                in_hunk = true;
            } else if let Some(p) = line.strip_prefix("+++ ") {
                if p.trim() == "/dev/null" {
                    file.deleted = true;
                } else {
                    file.path = diff_path(p);
                }
            } else if let Some(p) = line.strip_prefix("--- ") {
                if p.trim() == "/dev/null" {
                    file.created = true;
                }
            } else if line.starts_with("new file mode") {
                file.created = true;
            } else if let Some(old) = line.strip_prefix("rename from ") {
                file.renamed_from = Some(old.trim().trim_matches('"').to_string());
            } else if line.starts_with("deleted file mode") {
                file.deleted = true;
            }
            continue;
        }
        if let Some(added) = line.strip_prefix('+') {
            file.added.push(added.to_string());
        } else if let Some(removed) = line.strip_prefix('-') {
            file.removed.push(removed.to_string());
        }
    }
    out
}

fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// What in `diff` looks like gaming the tests: lines of existing tests
/// removed or changed (lines merely moved don't count), and tests switched
/// off anywhere in code. swarmux's own folder is left out.
pub fn scan_diff(diff: &str) -> Vec<Tamper> {
    let files = parse_diff(diff);
    let added_anywhere: HashSet<&str> = files
        .iter()
        .flat_map(|f| f.added.iter().map(|l| l.trim()))
        .collect();
    let mut out = Vec::new();
    for f in &files {
        if f.path.starts_with(".swarmux/") {
            continue;
        }
        if !is_prose(&f.path) {
            let removed_markers: HashSet<&str> = f.removed.iter().map(|l| l.trim()).collect();
            let skip = f.added.iter().find(|l| {
                let t = l.trim();
                !removed_markers.contains(t) && SKIP_MARKERS.iter().any(|m| t.contains(m))
            });
            if let Some(line) = skip {
                out.push(Tamper::Skipped {
                    path: f.path.clone(),
                    line: cut(line.trim(), LINE_MAX),
                });
            }
        }
        if is_test_path(&f.path) {
            let lines = f
                .removed
                .iter()
                .filter(|l| !trivial(l) && !added_anywhere.contains(l.trim()))
                .count();
            if lines > 0 {
                out.push(Tamper::TestEdited {
                    path: f.path.clone(),
                    lines: u32::try_from(lines).unwrap_or(u32::MAX),
                });
            }
        } else if !is_prose(&f.path) {
            // Tests that live beside the code: their assertions.
            let lines = f
                .removed
                .iter()
                .filter(|l| l.contains("assert") && !added_anywhere.contains(l.trim()))
                .count();
            if lines > 0 {
                out.push(Tamper::TestEdited {
                    path: f.path.clone(),
                    lines: u32::try_from(lines).unwrap_or(u32::MAX),
                });
            }
        }
    }
    out
}

/// Paths where a change reaches past the code itself: builds, dependencies,
/// pipelines, agent settings, schemas and secrets.
const RISKY: [&str; 22] = [
    ".github/",
    ".gitlab-ci",
    ".claude/",
    ".swarmux/",
    "cargo.toml",
    "build.rs",
    "package.json",
    "pyproject.toml",
    "requirements",
    "go.mod",
    "gemfile",
    "pom.xml",
    "build.gradle",
    "dockerfile",
    "makefile",
    "migrations/",
    "auth",
    "security",
    "crypto",
    "secret",
    "password",
    ".env",
];

/// How much a finished job's change needs a second reader.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewRisk {
    /// `0.0..=1.0`.
    pub score: f64,
    pub tier: RiskTier,
    /// What raised it, most telling first.
    pub reasons: Vec<String>,
}

impl ReviewRisk {
    /// The reviewer reads elevated and destructive changes, not routine ones.
    pub fn warrants_review(&self) -> bool {
        self.tier >= RiskTier::Elevated
    }

    /// `routine 0.19: 12 lines in 1 file`.
    pub fn summary(&self) -> String {
        let why = if self.reasons.is_empty() {
            String::new()
        } else {
            format!(": {}", self.reasons.join(", "))
        };
        format!("{} {:.2}{why}", self.tier.as_str(), self.score)
    }
}

/// Scores a change the way the inbox scores a tool call: a change in the
/// worktree is reversible (R1); one that touches builds, dependencies,
/// pipelines, secrets or the tests' referees reaches further (R2); its
/// size and deletions widen its reach.
pub fn review_risk(diff: &str) -> ReviewRisk {
    let files = parse_diff(diff);
    if files.is_empty() {
        return ReviewRisk {
            score: 0.0,
            tier: RiskTier::Routine,
            reasons: vec!["no changes".into()],
        };
    }
    let lines: usize = files.iter().map(|f| f.added.len() + f.removed.len()).sum();
    let deleted = files.iter().filter(|f| f.deleted).count();
    let risky: Vec<&str> = files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| {
            let low = p.to_ascii_lowercase();
            RISKY.iter().any(|r| low.contains(r))
        })
        .collect();
    let tamper = scan_diff(diff);
    let mut reasons = Vec::new();
    for t in &tamper {
        reasons.push(t.describe());
    }
    if let Some(first) = risky.first() {
        reasons.push(if risky.len() == 1 {
            format!("touches {first}")
        } else {
            format!(
                "touches {first} and {} more sensitive files",
                risky.len() - 1
            )
        });
    }
    if deleted > 0 {
        reasons.push(format!(
            "deletes {deleted} file{}",
            if deleted == 1 { "" } else { "s" }
        ));
    }
    reasons.push(format!(
        "{lines} line{} in {} file{}",
        if lines == 1 { "" } else { "s" },
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    ));
    let reach = if tamper.is_empty() && risky.is_empty() {
        Reversibility::R1
    } else {
        Reversibility::R2
    };
    let mut blast = (lines as f64 / 400.0).min(1.0);
    if files.len() > 15 {
        blast = blast.max(0.6);
    }
    if deleted > 0 {
        blast = blast.max(0.8);
    }
    let risk = assess(
        &RiskInputs {
            blast,
            ..RiskInputs::bare(reach)
        },
        false,
    );
    ReviewRisk {
        score: risk.score,
        tier: risk.tier,
        reasons,
    }
}

#[cfg(test)]
mod tests;
