use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use swarmux_application::evidence::{HeadFacts, Vcs};
use swarmux_application::gate::Checker;
use swarmux_application::{AppResult, CleanCheckout, CleanCopy, GateSource, LoadedGate};
use swarmux_domain::evidence::{ChangeKind, DiffFile, DiffSummary};
use swarmux_domain::gate::{CheckRun, GateCheck, GateOrigin, GateSpec, Pin};

/// The commit every demo repository is at.
const HEAD: &str = "9f3c2a71b8d04e56a1c7e0d9b24f68a3c5e1d7b0";

/// The checks each demo repository configures, by folder name. The mission
/// of `api-gateway` brings its own (`cargo test`), so that repository lists
/// none.
const CHECKS: [(&str, &str, &[&str]); 4] = [
    ("data-pipeline", "pytest", &["etl/"]),
    ("mobile-app", "./gradlew", &[":app:testDebug"]),
    ("docs-site", "npm", &["run", "docs:build"]),
    ("web-checkout", "npm", &["test"]),
];

/// Every demo repository has the same gate: its tests are protected, and the
/// checks are its own (see [`CHECKS`]; missions bring theirs). The checks run
/// in a clean copy (see [`DemoCheckout`]), as for real work.
pub struct DemoGateSource;

impl GateSource for DemoGateSource {
    fn load(&self, dir: &Path) -> LoadedGate {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let checks = CHECKS
            .iter()
            .filter(|(repo, ..)| *repo == name)
            .map(|(_, program, args)| GateCheck::new(&check_name(program, args), program, args))
            .collect();
        LoadedGate {
            root: dir.to_path_buf(),
            spec: GateSpec {
                checks,
                protected: vec!["tests/**".into()],
                evidence: false,
                ..GateSpec::default()
            },
            origin: GateOrigin::File,
            problem: None,
        }
    }

    fn pins(&self, _root: &Path, _globs: &[String]) -> Vec<Pin> {
        Vec::new()
    }
}

/// A check's name: its command line.
fn check_name(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Makes the "clean copies" the demo's gate runs its checks in. The demo has
/// no repository to copy, so a copy is a name for the repository's own
/// folder, holds no files and carries no change to scan; the checks that
/// run in it are the script's. Nothing is read or written.
pub struct DemoCheckout;

impl CleanCheckout for DemoCheckout {
    fn head(&self, _dir: &Path) -> Option<String> {
        Some(HEAD.into())
    }

    fn repo(&self, dir: &Path) -> Option<String> {
        Some(format!("demo/{}", dir.file_name()?.to_str()?))
    }

    fn snapshot(&self, dir: &Path) -> AppResult<Box<dyn CleanCopy>> {
        Ok(Box::new(DemoCopy {
            path: dir.to_path_buf(),
            repo: self.repo(dir).unwrap_or_default(),
        }))
    }

    fn root(&self) -> PathBuf {
        PathBuf::from("/swarmux-demo/copies")
    }
}

struct DemoCopy {
    path: PathBuf,
    repo: String,
}

impl CleanCopy for DemoCopy {
    fn path(&self) -> &Path {
        &self.path
    }

    fn repo(&self) -> &str {
        &self.repo
    }

    fn diff_from(&self, _base: Option<&str>) -> AppResult<String> {
        Ok(String::new())
    }

    fn restore(&self, _base: &str, _paths: &[String]) -> AppResult<()> {
        Ok(())
    }

    fn head(&self) -> Option<&str> {
        Some(HEAD)
    }

    fn show(&self, _commit: &str, _path: &str) -> AppResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }

    fn write(&self, _path: &str, _bytes: &[u8]) -> AppResult<()> {
        Ok(())
    }

    fn remove(&self, _path: &str) -> AppResult<()> {
        Ok(())
    }

    fn files(&self) -> AppResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn reset(&self) -> AppResult<()> {
        Ok(())
    }
}

/// Runs no process: the checks of a folder fail a set number of times and
/// pass after that. The gate's own retry of a failed check counts as a run.
#[derive(Default)]
pub struct DemoChecker {
    failures: Mutex<HashMap<PathBuf, u32>>,
}

impl DemoChecker {
    /// The next `runs` runs of any check in `root` fail.
    pub fn fail(&self, root: &Path, runs: u32) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root.to_path_buf(), runs);
    }
}

impl Checker for DemoChecker {
    fn run(&self, check: &GateCheck, root: &Path) -> CheckRun {
        let failed = {
            let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            match failures.get_mut(root) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    true
                }
                _ => false,
            }
        };
        CheckRun {
            name: check.name.clone(),
            command: check.command_line(),
            exit: Some(if failed { 101 } else { 0 }),
            timed_out: false,
            spawn_error: None,
            duration_ms: if failed { 3_400 } else { 4_100 },
            output: if failed {
                "test invoice::rounds_half_up ... FAILED\n\
                 assertion `left == right` failed: left: 1999, right: 2000\n\
                 test result: FAILED. 41 passed; 1 failed"
                    .into()
            } else {
                "test result: ok. 42 passed; 0 failed".into()
            },
            flaky: false,
        }
    }
}

/// Every folder is a repository at the same commit.
pub struct DemoVcs;

impl Vcs for DemoVcs {
    fn head(&self, dir: &Path) -> AppResult<Option<HeadFacts>> {
        Ok(Some(HeadFacts {
            head: HEAD.into(),
            dirty: false,
            branch: Some("swarmux/demo/1".into()),
            base: Some("1b7e4d09c2a85f36e0b94d7a13c6f258ea09d4b7".into()),
            repo: dir.to_path_buf(),
        }))
    }

    fn diff(&self, _dir: &Path, _base: Option<&str>, _head: &str) -> AppResult<DiffSummary> {
        Ok(DiffSummary {
            files: 3,
            insertions: 41,
            deletions: 9,
            hash: "d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0d3m0".into(),
            // The scripted change, file by file: they add up to the totals.
            changes: vec![
                DiffFile::text("src/invoice/round.rs", 22, 5).with_kind(ChangeKind::Modified),
                DiffFile::text("src/invoice/total.rs", 10, 2).with_kind(ChangeKind::Modified),
                DiffFile::text("src/invoice/mod.rs", 9, 2).with_kind(ChangeKind::Modified),
            ],
        })
    }
}
