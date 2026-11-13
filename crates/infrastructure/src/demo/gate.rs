use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use swarmux_application::evidence::{HeadFacts, Vcs};
use swarmux_application::gate::Checker;
use swarmux_application::{AppResult, GateSource, LoadedGate};
use swarmux_domain::evidence::{ChangeKind, DiffFile, DiffSummary};
use swarmux_domain::gate::{CheckRun, GateCheck, GateOrigin, GateSpec, Pin};

/// Every demo repository has the same gate: its tests are protected and
/// there are no checks of its own (missions bring theirs).
pub struct DemoGateSource;

impl GateSource for DemoGateSource {
    fn load(&self, dir: &Path) -> LoadedGate {
        LoadedGate {
            root: dir.to_path_buf(),
            spec: GateSpec {
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
            head: "9f3c2a71b8d04e56a1c7e0d9b24f68a3c5e1d7b0".into(),
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
