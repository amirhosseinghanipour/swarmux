//! The harness substrate: the files that decide how tests are found, built
//! and run.
//!
//! A test is only as honest as what runs it. A planted `conftest.py`, a
//! `.cargo/config.toml` that names a different test runner, a `build.rs`,
//! a jest config that ignores the failing folder or a `package.json` whose
//! test script is `true` make every check pass without the code being any
//! better. Weakening a test shows in a diff of the test; changing its
//! harness does not, so this list names the harness files and every
//! created, changed, moved or deleted one is a finding.
//!
//! The list is data: the built-in defaults below, extended by
//! `[verifier] substrate` in `gates.toml`. A repository that changes one on
//! purpose says so with `allow_substrate` (or `allow_test_changes`).
//!
//! Manifests (`Cargo.toml`, `package.json`, `pyproject.toml`, `pubspec.yaml`,
//! `go.mod`) are both harness and the place dependencies are declared. A
//! change that only adds or bumps dependencies is a dependency finding, not
//! harness tampering; a change to anything else in them (a `[patch]`
//! section, a test script, a pytest entry point, a build script) is.

use std::collections::{BTreeMap, BTreeSet};

use super::{FileDiff, manifest};
use crate::gate::{GateCheck, protected_glob};

/// The built-in harness files, by ecosystem: the glob (like protected
/// paths: a bare name matches at any depth) and what it does to the tests.
pub const DEFAULT_SUBSTRATE: &[(&str, &str)] = &[
    // Python
    ("conftest.py", "pytest runs it before every test"),
    ("pytest.ini", "pytest's configuration"),
    ("pyproject.toml", "Python project and test configuration"),
    ("setup.cfg", "Python project and test configuration"),
    ("setup.py", "runs when the project is installed"),
    ("tox.ini", "tox and pytest configuration"),
    ("noxfile.py", "decides how the sessions run"),
    (".coveragerc", "coverage configuration"),
    ("sitecustomize.py", "Python runs it at start-up"),
    ("usercustomize.py", "Python runs it at start-up"),
    ("*.pth", "Python runs its import lines at start-up"),
    // JavaScript and TypeScript
    ("jest.config.*", "jest's configuration"),
    ("jest.setup.*", "jest runs it before the tests"),
    ("vitest.config.*", "vitest's configuration"),
    ("vitest.setup.*", "vitest runs it before the tests"),
    ("karma.conf.*", "karma's configuration"),
    (".mocharc*", "mocha's configuration"),
    ("playwright.config.*", "playwright's configuration"),
    ("cypress.config.*", "cypress' configuration"),
    (
        "package.json",
        "its scripts and test configuration run the tests",
    ),
    ("babel.config.*", "decides how the tests are compiled"),
    (".babelrc*", "decides how the tests are compiled"),
    ("tsconfig*.json", "decides how the tests are compiled"),
    // Rust
    ("build.rs", "cargo runs it before building"),
    (
        "**/.cargo/config.toml",
        "cargo's settings, including the test runner",
    ),
    (
        "**/.cargo/config",
        "cargo's settings, including the test runner",
    ),
    (
        "Cargo.toml",
        "dependencies, targets and cargo's test settings",
    ),
    ("rust-toolchain", "decides which compiler runs"),
    ("rust-toolchain.toml", "decides which compiler runs"),
    // Go
    ("go.mod", "dependencies and replacements"),
    ("go.work", "decides which modules are used"),
    // Dart and Flutter
    ("dart_test.yaml", "dart's test configuration"),
    ("pubspec.yaml", "dependencies and overrides"),
    // General
    ("Makefile", "make runs what it says"),
    ("makefile", "make runs what it says"),
    ("GNUmakefile", "make runs what it says"),
    ("justfile", "just runs what it says"),
    ("Justfile", "just runs what it says"),
    ("Taskfile.yml", "task runs what it says"),
    ("Taskfile.yaml", "task runs what it says"),
    (".github/workflows/**", "the CI pipeline"),
    (".gitlab-ci.yml", "the CI pipeline"),
    (".circleci/**", "the CI pipeline"),
    ("azure-pipelines.yml", "the CI pipeline"),
    ("Jenkinsfile", "the CI pipeline"),
    (".githooks/**", "git runs these hooks"),
    (".husky/**", "git runs these hooks"),
    (".pre-commit-config.yaml", "runs on every commit"),
    (".gitattributes", "decides how git writes files"),
    (".npmrc", "decides where packages come from"),
    (".yarnrc*", "decides where packages come from"),
    ("pip.conf", "decides where packages come from"),
    (".pypirc", "decides where packages come from"),
];

/// Lockfiles and requirement lists: no harness, but where dependencies
/// appear.
pub const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "go.sum",
    "pubspec.lock",
    "requirements*.txt",
];

/// How a harness file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubstrateKind {
    Created,
    Modified,
    Deleted,
    /// Moved away from where it was.
    MovedAway,
}

impl SubstrateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubstrateKind::Created => "created",
            SubstrateKind::Modified => "changed",
            SubstrateKind::Deleted => "deleted",
            SubstrateKind::MovedAway => "moved away",
        }
    }
}

/// One harness file a job changed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubstrateChange {
    pub path: String,
    pub kind: SubstrateKind,
    /// What the file does to the tests, and for a manifest what changed in it.
    pub why: String,
}

impl SubstrateChange {
    /// The evidence line: `conftest.py: harness file created (pytest runs it before every test)`.
    pub fn describe(&self) -> String {
        format!(
            "{}: harness file {} ({})",
            self.path,
            self.kind.as_str(),
            self.why
        )
    }
}

/// What a diff does to the harness and to the dependencies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubstrateScan {
    pub changes: Vec<SubstrateChange>,
    /// New or changed dependencies, one line per file with the package names.
    pub dependencies: Vec<String>,
}

/// The programs of `checks` that are paths inside the repository
/// (`./scripts/test.sh`): what a check runs is harness too. Relative to the
/// repository root, `/`-separated.
pub fn check_program_paths(checks: &[GateCheck]) -> Vec<String> {
    let mut out = Vec::new();
    for c in checks {
        let p = c.program.replace('\\', "/");
        let local = p.contains('/') && !p.starts_with('/') && p.as_bytes().get(1) != Some(&b':');
        if !local {
            continue;
        }
        let p = p.trim_start_matches("./");
        let full = match &c.dir {
            Some(d) => format!("{d}/{p}"),
            None => p.to_string(),
        };
        if !full.split('/').any(|s| s == "..") && !out.contains(&full) {
            out.push(full);
        }
    }
    out
}

/// Every harness glob that applies: the defaults, the repository's own
/// `extra` and the check programs that are repository paths.
pub fn substrate_globs(extra: &[String], checks: &[GateCheck]) -> Vec<String> {
    let mut all: Vec<String> = DEFAULT_SUBSTRATE
        .iter()
        .map(|(g, _)| (*g).to_string())
        .collect();
    for g in extra.iter().cloned().chain(check_program_paths(checks)) {
        if !all.contains(&g) {
            all.push(g);
        }
    }
    all
}

/// What `path` does to the tests, from the built-in list; `None` for a
/// path the repository added to the list itself.
fn why_of(path: &str, fold: bool) -> String {
    DEFAULT_SUBSTRATE
        .iter()
        .find(|(g, _)| protected_glob(&[(*g).to_string()], path, fold).is_some())
        .map_or_else(
            || "listed as test harness in gates.toml or run by a check".to_string(),
            |(_, why)| (*why).to_string(),
        )
}

/// What a diff does to the harness: each file of `globs` created, changed,
/// moved or deleted, except those `allow` matches. `old` and `new` give a
/// file's content before and after, for refining manifests: one that only
/// gained dependencies is no tampering.
///
/// Dependencies of every manifest and lockfile are listed, allowed or not.
pub fn scan(
    files: &[FileDiff],
    globs: &[String],
    allow: &[String],
    old: &dyn Fn(&str) -> Option<String>,
    new: &dyn Fn(&str) -> Option<String>,
    fold: bool,
) -> SubstrateScan {
    let mut out = SubstrateScan::default();
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut note = |file: &str, names: Vec<String>| {
        deps.entry(file.to_string()).or_default().extend(names);
    };
    for f in files {
        if f.path.starts_with(".swarmux/") {
            continue;
        }
        let allowed = |p: &str| protected_glob(allow, p, fold).is_some();
        let mut flag = |path: &str, kind: SubstrateKind, why: String| {
            if protected_glob(globs, path, fold).is_some() && !allowed(path) {
                out.changes.push(SubstrateChange {
                    path: path.to_string(),
                    kind,
                    why,
                });
            }
        };
        if let Some(from) = f.renamed_from.as_deref() {
            flag(from, SubstrateKind::MovedAway, why_of(from, fold));
        }
        if protected_glob(globs, &f.path, fold).is_none() {
            if let Some(names) = manifest::lock_dependencies(f) {
                note(&f.path, names);
            }
            continue;
        }
        let kind = if f.created || f.renamed_from.is_some() {
            SubstrateKind::Created
        } else if f.deleted {
            SubstrateKind::Deleted
        } else {
            SubstrateKind::Modified
        };
        let why = why_of(&f.path, fold);
        if kind == SubstrateKind::Modified
            && let Some(m) =
                manifest::analyze(&f.path, old(&f.path).as_deref(), new(&f.path).as_deref())
        {
            note(&f.path, m.dependencies);
            if let Some(first) = m.harness.first() {
                let more = m.harness.len() - 1;
                let detail = if more == 0 {
                    first.clone()
                } else {
                    format!("{first} and {more} more")
                };
                flag(&f.path, kind, detail);
            }
            continue;
        }
        flag(&f.path, kind, why);
    }
    out.changes.sort();
    out.changes.dedup();
    out.dependencies = deps
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .map(|(file, names)| {
            format!(
                "{file}: {}",
                names.into_iter().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    out
}

#[cfg(test)]
mod tests;
