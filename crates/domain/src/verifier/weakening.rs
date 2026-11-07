//! The test-weakening detector: what a job's diff does to the tests that
//! judge it.
//!
//! Agents that are graded by tests sometimes make the tests easier instead
//! of the code better: they delete a test, switch it off, loosen what it
//! asserts, bless a changed snapshot, widen a tolerance, or relax the CI
//! step that runs it. This reads a unified diff (hunks in, findings out; no
//! model, no I/O) and names each of those, with the line that shows it.
//!
//! The rules are heuristics over text, tuned to stay quiet on honest work:
//! a renamed file, a new test, a test moved to another file and an
//! assertion rewritten with the same strength are not findings. A path in
//! the `allow` list (globs, like protected paths) is never reported: that
//! is how a repository says "removing these tests is intended".

use std::collections::HashSet;

use super::{FileDiff, Tamper, cut, is_prose, is_test_path, parse_diff, scan_diff, trivial};
use crate::gate::{GateCheck, protected_glob};

/// Longest line quoted in a finding.
const SHOWN: usize = 100;

/// What kind of weakening a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WeakKind {
    /// A test file was deleted.
    TestFileDeleted,
    /// Test functions were removed from a file that stays.
    TestsRemoved,
    /// Lines of an existing test were removed or changed.
    TestEdited,
    /// A test was switched off, or narrowed to some tests (`only`).
    Skipped,
    /// A changed test makes fewer assertions than before.
    AssertionsDropped,
    /// An exact check became a check anything passes.
    LooseMatcher,
    /// A snapshot or golden file was edited or deleted.
    SnapshotEdited,
    /// A numeric tolerance was made larger.
    ToleranceRelaxed,
    /// A CI or test-runner setting that runs, or gates on, the tests.
    ConfigChanged,
}

impl WeakKind {
    /// A few words for the kind.
    pub fn label(self) -> &'static str {
        match self {
            WeakKind::TestFileDeleted => "test file deleted",
            WeakKind::TestsRemoved => "tests removed",
            WeakKind::TestEdited => "existing tests changed",
            WeakKind::Skipped => "a test was switched off",
            WeakKind::AssertionsDropped => "assertions dropped",
            WeakKind::LooseMatcher => "assertion loosened",
            WeakKind::SnapshotEdited => "snapshot or golden file changed",
            WeakKind::ToleranceRelaxed => "tolerance relaxed",
            WeakKind::ConfigChanged => "test or CI configuration changed",
        }
    }
}

/// One thing in a diff that makes the tests easier to pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weakening {
    pub kind: WeakKind,
    pub path: String,
    /// What shows it: a count, a line from the diff.
    pub detail: String,
}

impl Weakening {
    /// The evidence line the gate shows: `path: what (detail)`.
    pub fn describe(&self) -> String {
        if self.detail.is_empty() {
            format!("{}: {}", self.path, self.kind.label())
        } else {
            format!("{}: {} ({})", self.path, self.kind.label(), self.detail)
        }
    }

    /// Whether this finding touches what the checks run: a protected path,
    /// a path a check names, or (for tests themselves) any repository whose
    /// gate has checks. CI configuration alone does not.
    pub fn affects_checks(&self, protected: &[String], checks: &[GateCheck], fold: bool) -> bool {
        if protected_glob(protected, &self.path, fold).is_some() {
            return true;
        }
        if checks
            .iter()
            .flat_map(|c| c.args.iter())
            .any(|a| !a.is_empty() && self.path.contains(a.trim_matches('/')) && a.len() > 3)
        {
            return true;
        }
        self.kind != WeakKind::ConfigChanged && !checks.is_empty()
    }
}

fn weak(kind: WeakKind, path: &str, detail: impl Into<String>) -> Weakening {
    Weakening {
        kind,
        path: path.to_string(),
        detail: detail.into(),
    }
}

/// Everything in `diff` that weakens the tests, except paths `allow`
/// matches. Skip markers and edited test lines come from the older tamper
/// scan; the rest is checked here.
pub fn detect(diff: &str, allow: &[String]) -> Vec<Weakening> {
    let mut out: Vec<Weakening> = scan_diff(diff)
        .into_iter()
        .map(|t| match t {
            Tamper::TestEdited { path, lines } => weak(
                WeakKind::TestEdited,
                &path,
                format!("{lines} line{} removed or changed", plural(lines)),
            ),
            Tamper::Skipped { path, line } => weak(WeakKind::Skipped, &path, format!("`{line}`")),
        })
        .collect();
    out.extend(detect_more(diff));
    // A deleted file's lines are all "changed": say it once.
    let gone: HashSet<String> = out
        .iter()
        .filter(|w| matches!(w.kind, WeakKind::TestFileDeleted | WeakKind::TestsRemoved))
        .map(|w| w.path.clone())
        .collect();
    out.retain(|w| w.kind != WeakKind::TestEdited || !gone.contains(&w.path));
    out.retain(|w| protected_glob(allow, &w.path, false).is_none());
    out.sort_by(|a, b| (&a.path, a.kind).cmp(&(&b.path, b.kind)));
    out.dedup();
    out
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The kinds the older scan does not know: deleted tests, dropped and
/// loosened assertions, snapshots, tolerances and CI configuration.
pub fn detect_more(diff: &str) -> Vec<Weakening> {
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
        if is_snapshot(&f.path) {
            if !f.created {
                let what = if f.deleted { "deleted" } else { "edited" };
                out.push(weak(WeakKind::SnapshotEdited, &f.path, what));
            }
            continue;
        }
        if is_prose(&f.path) {
            continue;
        }
        if is_ci_config(&f.path) {
            out.extend(config_change(f, &added_anywhere));
            continue;
        }
        if !is_test_path(&f.path) {
            continue;
        }
        out.extend(test_file(f, &added_anywhere));
    }
    out
}

fn test_file(f: &FileDiff, added_anywhere: &HashSet<&str>) -> Vec<Weakening> {
    let mut out = Vec::new();
    if f.deleted {
        // A file whose lines all live on somewhere else was moved.
        let lost = f
            .removed
            .iter()
            .filter(|l| !trivial(l) && !added_anywhere.contains(l.trim()))
            .count();
        if lost > 0 {
            out.push(weak(
                WeakKind::TestFileDeleted,
                &f.path,
                format!("{lost} line{} gone", plural(lost as u32)),
            ));
        }
        return out;
    }
    if f.created {
        return out;
    }
    let removed_tests = count(&f.removed, is_test_decl);
    let added_tests = count(&f.added, is_test_decl);
    if removed_tests > added_tests {
        out.push(weak(
            WeakKind::TestsRemoved,
            &f.path,
            format!("{} removed, {} added", removed_tests, added_tests),
        ));
    } else {
        let removed = count(&f.removed, is_assertion);
        let added = count(&f.added, is_assertion);
        if removed > added {
            out.push(weak(
                WeakKind::AssertionsDropped,
                &f.path,
                format!("{removed} assertions before, {added} after"),
            ));
        }
    }
    let loose_added = f.added.iter().filter(|l| is_loose(l)).count();
    let loose_removed = f.removed.iter().filter(|l| is_loose(l)).count();
    if loose_added > loose_removed
        && let Some(line) = f.added.iter().find(|l| is_loose(l))
    {
        out.push(weak(
            WeakKind::LooseMatcher,
            &f.path,
            format!("`{}`", cut(line.trim(), SHOWN)),
        ));
    }
    if let Some(what) = relaxed_tolerance(&f.removed, &f.added) {
        out.push(weak(WeakKind::ToleranceRelaxed, &f.path, what));
    }
    out
}

fn count(lines: &[String], pred: fn(&str) -> bool) -> usize {
    lines.iter().filter(|l| pred(l)).count()
}

/// A line that declares one test.
fn is_test_decl(line: &str) -> bool {
    let t = line.trim_start();
    const STARTS: [&str; 14] = [
        "#[test]",
        "#[tokio::test",
        "#[rstest",
        "#[test_case",
        "def test_",
        "async def test_",
        "func Test",
        "@Test",
        "it(",
        "it.each",
        "test(",
        "test.each",
        "testWidgets(",
        "[Fact",
    ];
    STARTS.iter().any(|s| t.starts_with(s)) || t.starts_with("[Theory")
}

/// A line that asserts something.
fn is_assertion(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") || t.starts_with('#') && !t.starts_with("#[") {
        return false;
    }
    const WORDS: [&str; 20] = [
        "assert!",
        "assert_eq!",
        "assert_ne!",
        "debug_assert",
        "assert ",
        "assert(",
        "assertEquals",
        "assertEqual",
        "assertThat",
        "assertTrue",
        "assertFalse",
        "assertRaises",
        "expect(",
        "expect_",
        "EXPECT_",
        "ASSERT_",
        "t.Error",
        "t.Fatal",
        "require.",
        "self.assert",
    ];
    WORDS.iter().any(|w| t.contains(w)) || t.contains("matches!(") && t.contains("assert")
}

/// A check that passes whatever the code does.
fn is_loose(line: &str) -> bool {
    const LOOSE: [&str; 12] = [
        "assert!(true",
        "assert True",
        "assert(true",
        "assertTrue(true",
        "assert_eq!(true, true",
        ".toBeTruthy(",
        ".toBeDefined(",
        ".not.toBeNull(",
        "expect.anything(",
        "expect.any(",
        "assertIsNotNone(",
        "toBeGreaterThanOrEqual(0)",
    ];
    LOOSE.iter().any(|m| line.contains(m))
}

/// Tolerance names and the number after them, compared before and after.
fn relaxed_tolerance(removed: &[String], added: &[String]) -> Option<String> {
    const KEYS: [&str; 10] = [
        "abs=",
        "rel=",
        "abs_tol",
        "rel_tol",
        "rtol",
        "atol",
        "delta",
        "epsilon",
        "tolerance",
        "approx",
    ];
    let before: Vec<(&str, f64)> = removed.iter().flat_map(|l| numbers(l, &KEYS)).collect();
    for line in added {
        for (key, now) in numbers(line, &KEYS) {
            let was = before.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            if let Some(was) = was.filter(|w| now > *w) {
                return Some(format!("{key} {was} -> {now}"));
            }
        }
    }
    None
}

/// `(key, number)` for each of `keys` followed by a number on `line`.
fn numbers<'a>(line: &str, keys: &[&'a str]) -> Vec<(&'a str, f64)> {
    let lower = line.to_ascii_lowercase();
    let mut out = Vec::new();
    for key in keys {
        let Some(at) = lower.find(key) else { continue };
        let rest = lower[at + key.len()..]
            .trim_start_matches(|c: char| " =:(,<>_".contains(c) || c == '\t')
            .to_string();
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'e' | '-' | '+' | '_')))
            .unwrap_or(rest.len());
        let num = rest[..end].replace('_', "");
        if let Ok(v) = num.parse::<f64>() {
            out.push((*key, v));
        }
    }
    out
}

/// Snapshot and golden files: expected output kept in a file.
fn is_snapshot(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    low.contains("__snapshots__/")
        || low.contains("/snapshots/")
        || low.starts_with("snapshots/")
        || low.contains("/golden/")
        || low.starts_with("golden/")
        || [
            ".snap",
            ".golden",
            ".approved.txt",
            ".approved.json",
            ".ambr",
        ]
        .iter()
        .any(|e| low.ends_with(e))
        || low
            .rsplit('/')
            .next()
            .is_some_and(|f| f.starts_with("golden.") || f.contains(".golden."))
}

/// Files that configure how tests run or what CI demands of them.
fn is_ci_config(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let file = low.rsplit('/').next().unwrap_or(&low);
    low.starts_with(".github/workflows/")
        || low.starts_with(".circleci/")
        || low.starts_with(".buildkite/")
        || matches!(
            file,
            ".gitlab-ci.yml"
                | "jenkinsfile"
                | "azure-pipelines.yml"
                | "tox.ini"
                | "pytest.ini"
                | ".coveragerc"
                | "codecov.yml"
                | ".codecov.yml"
                | "noxfile.py"
                | ".mocharc.json"
                | ".mocharc.js"
                | "package.json"
                | "pyproject.toml"
                | "setup.cfg"
                | "cargo.toml"
                | "config.toml"
        )
        || file.starts_with("jest.config.")
        || file.starts_with("vitest.config.")
        || file.starts_with("karma.conf")
}

/// Whether a changed line of a configuration file is about running tests.
fn test_line(path: &str, line: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let file = low.rsplit('/').next().unwrap_or(&low);
    let l = line.to_ascii_lowercase();
    match file {
        "package.json" => {
            l.contains("\"test")
                || l.contains("coveragethreshold")
                || l.contains("\"jest\"") && l.contains('{')
        }
        "cargo.toml" => {
            l.trim_start().starts_with("test =")
                || l.trim_start().starts_with("doctest =")
                || l.trim_start().starts_with("harness =")
        }
        "config.toml" => false,
        "pyproject.toml" | "setup.cfg" | "tox.ini" | "pytest.ini" => [
            "fail_under",
            "fail-under",
            "addopts",
            "testpaths",
            "filterwarnings",
            "--cov",
        ]
        .iter()
        .any(|k| l.contains(k)),
        _ => [
            "test",
            "pytest",
            "coverage",
            "cov-fail",
            "fail_under",
            "threshold",
            "mocha",
            "jest",
            "vitest",
        ]
        .iter()
        .any(|k| l.contains(k)),
    }
}

/// Phrases that make a CI step stop mattering.
fn defanged(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "continue-on-error: true",
        "|| true",
        "--ignore",
        "--deselect",
        "-k \"not",
        "--no-verify",
        "--passwithnotests",
        "if: false",
    ]
    .iter()
    .any(|k| l.contains(k))
}

fn config_change(f: &FileDiff, added_anywhere: &HashSet<&str>) -> Option<Weakening> {
    let gone = f
        .removed
        .iter()
        .find(|l| test_line(&f.path, l) && !added_anywhere.contains(l.trim()));
    let off = f.added.iter().find(|l| defanged(l));
    let line = gone.or(off)?;
    Some(weak(
        WeakKind::ConfigChanged,
        &f.path,
        format!("`{}`", cut(line.trim(), SHOWN)),
    ))
}

#[cfg(test)]
mod tests;
