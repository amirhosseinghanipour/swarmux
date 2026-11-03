use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::*;
use crate::gate::{GateKeeper, GateRecord, GateSource, LoadedGate, WriteFence};
use crate::hookbus::{HookNotice, StopAnswer, StopContext, StopPolicy};
use crate::{AppError, ExecutionLog};
use swarmux_domain::gate::{GateOrigin, GateOutcome, Pin};
use swarmux_domain::{HookEvent, JobId};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

const SECRET: &str = "secret_answer_for_seven";

/// One git repository at `/w` with a `cargo test` check.
struct Source {
    spec: Mutex<GateSpec>,
    /// Pins by root: the worktree `/w` and the copy `/copy`.
    pins: Mutex<Vec<(PathBuf, Vec<Pin>)>>,
}

impl Source {
    fn new() -> Source {
        Source {
            spec: Mutex::new(GateSpec {
                checks: vec![GateCheck::new("cargo test", "cargo", &["test"])],
                ..GateSpec::default()
            }),
            pins: Mutex::new(Vec::new()),
        }
    }

    fn set_pins(&self, root: &str, pins: &[(&str, &str)]) {
        let pins: Vec<Pin> = pins
            .iter()
            .map(|(p, h)| Pin {
                path: (*p).into(),
                sha256: (*h).into(),
            })
            .collect();
        let mut all = lock(&self.pins);
        all.retain(|(r, _)| r != Path::new(root));
        all.push((PathBuf::from(root), pins));
    }
}

impl GateSource for Source {
    fn load(&self, _dir: &Path) -> LoadedGate {
        LoadedGate {
            root: PathBuf::from("/w"),
            spec: lock(&self.spec).clone(),
            origin: GateOrigin::File,
            problem: None,
        }
    }

    fn pins(&self, root: &Path, _globs: &[String]) -> Vec<Pin> {
        lock(&self.pins)
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }
}

/// Checks pass unless their name is listed as failing; every run is
/// remembered with the folder it ran in.
#[derive(Default)]
struct Checks {
    failing: Mutex<HashSet<String>>,
    ran: Mutex<Vec<(String, PathBuf)>>,
}

impl Checks {
    fn fail(&self, name: &str) {
        lock(&self.failing).insert(name.into());
    }

    fn pass(&self, name: &str) {
        lock(&self.failing).remove(name);
    }

    fn roots(&self) -> Vec<PathBuf> {
        lock(&self.ran).iter().map(|(_, r)| r.clone()).collect()
    }
}

impl Checker for Checks {
    fn run(&self, check: &GateCheck, root: &Path) -> CheckRun {
        lock(&self.ran).push((check.name.clone(), root.to_path_buf()));
        let pass = !lock(&self.failing).contains(&check.name);
        CheckRun {
            name: check.name.clone(),
            command: check.command_line(),
            exit: Some(if pass { 0 } else { 1 }),
            timed_out: false,
            spawn_error: None,
            duration_ms: 10,
            output: if pass {
                "ok".into()
            } else {
                format!("assertion failed: {SECRET}")
            },
            flaky: false,
        }
    }
}

/// Copies at `/copy`; `/elsewhere` is not in git.
#[derive(Default)]
struct Copies {
    diff: Mutex<String>,
    made: AtomicUsize,
    dropped: Arc<AtomicUsize>,
    broken: Mutex<bool>,
}

struct Copy {
    path: PathBuf,
    dropped: Arc<AtomicUsize>,
    diff: String,
    bases: Arc<Mutex<Vec<Option<String>>>>,
}

impl CleanCopy for Copy {
    fn path(&self) -> &Path {
        &self.path
    }

    fn repo(&self) -> &str {
        "w"
    }

    fn diff_from(&self, base: Option<&str>) -> AppResult<String> {
        lock(&self.bases).push(base.map(str::to_string));
        Ok(self.diff.clone())
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct SharedCopies(Arc<Copies>, Arc<Mutex<Vec<Option<String>>>>);

impl CleanCheckout for SharedCopies {
    fn head(&self, dir: &Path) -> Option<String> {
        (dir == Path::new("/w")).then(|| "base-sha".to_string())
    }

    fn repo(&self, dir: &Path) -> Option<String> {
        (dir == Path::new("/w")).then(|| "w".to_string())
    }

    fn snapshot(&self, _dir: &Path) -> AppResult<Box<dyn CleanCopy>> {
        if *lock(&self.0.broken) {
            return Err(AppError::Workspace("git broke".into()));
        }
        self.0.made.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Copy {
            path: PathBuf::from("/copy"),
            dropped: self.0.dropped.clone(),
            diff: lock(&self.0.diff).clone(),
            bases: self.1.clone(),
        }))
    }

    fn root(&self) -> PathBuf {
        PathBuf::from("C:\\Users\\me\\.swarmux\\verify")
    }
}

/// Hidden checks of repository `w`; a check can be marked tampered.
#[derive(Default)]
struct Holdout {
    checks: Mutex<Vec<HoldoutCheck>>,
    tampered: Mutex<HashSet<String>>,
    placed: Mutex<Vec<(String, PathBuf)>>,
}

impl Holdout {
    fn hide(&self, name: &str) {
        lock(&self.checks).push(HoldoutCheck {
            check: GateCheck::new(name, "cargo", &["test", "--test", name]),
            files: vec![Pin {
                path: format!("tests/{name}.rs"),
                sha256: "h".into(),
            }],
        });
    }
}

impl HoldoutStore for Holdout {
    fn list(&self, repo: &str) -> AppResult<Vec<HoldoutCheck>> {
        Ok(if repo == "w" {
            lock(&self.checks).clone()
        } else {
            Vec::new()
        })
    }

    fn add(&self, _: &str, check: &GateCheck, _: &[(String, Vec<u8>)]) -> AppResult<HoldoutCheck> {
        let h = HoldoutCheck {
            check: check.clone(),
            files: Vec::new(),
        };
        lock(&self.checks).push(h.clone());
        Ok(h)
    }

    fn remove(&self, _: &str, name: &str) -> AppResult<bool> {
        let mut c = lock(&self.checks);
        let before = c.len();
        c.retain(|h| h.name() != name);
        Ok(c.len() < before)
    }

    fn place(&self, _: &str, check: &HoldoutCheck, dest: &Path) -> AppResult<()> {
        if lock(&self.tampered).contains(check.name()) {
            return Err(AppError::Workspace("pin mismatch".into()));
        }
        lock(&self.placed).push((check.name().to_string(), dest.to_path_buf()));
        Ok(())
    }

    fn root(&self) -> PathBuf {
        PathBuf::from("C:\\Users\\me\\.swarmux\\holdout")
    }
}

struct World {
    keeper: GateKeeper,
    records: Receiver<GateRecord>,
    source: Arc<Source>,
    checks: Arc<Checks>,
    copies: Arc<Copies>,
    bases: Arc<Mutex<Vec<Option<String>>>>,
    holdout: Arc<Holdout>,
    job: JobId,
}

fn world() -> World {
    let source = Arc::new(Source::new());
    let checks = Arc::new(Checks::default());
    let copies = Arc::new(Copies::default());
    let bases = Arc::new(Mutex::new(Vec::new()));
    let holdout = Arc::new(Holdout::default());
    let verifier = Verifier::new(
        Arc::new(SharedCopies(copies.clone(), bases.clone())),
        checks.clone(),
    )
    .with_holdout(holdout.clone());
    let (keeper, records) = GateKeeper::new(source.clone(), checks.clone());
    World {
        keeper: keeper.with_verifier(verifier),
        records,
        source,
        checks,
        copies,
        bases,
        holdout,
        job: JobId(7),
    }
}

impl World {
    fn start(&self) {
        self.keeper.observe(&HookNotice {
            event: HookEvent::SessionStart,
            job: Some(self.job),
            session: "c".into(),
            cwd: "/w".into(),
            tool: None,
            summary: String::new(),
            detail: String::new(),
            input_sha256: String::new(),
            input_len: 0,
            raw: String::new(),
        });
    }

    fn stop(&self) -> (StopAnswer, Option<GateRecord>) {
        self.keeper.executed(self.job, "cargo test", true);
        let answer = self.keeper.on_stop(&StopContext {
            event: HookEvent::Stop,
            job: Some(self.job),
            session: "c".into(),
            cwd: "/w".into(),
            last_message: "Done.".into(),
        });
        (answer, self.records.try_recv().ok())
    }
}

#[test]
fn the_checks_run_in_the_clean_copy_not_the_worktree() {
    let w = world();
    w.start();
    let (answer, record) = w.stop();
    assert_eq!(answer, StopAnswer::Allow);
    assert_eq!(w.checks.roots(), [PathBuf::from("/copy")]);
    assert_eq!(w.copies.made.load(Ordering::SeqCst), 1);
    assert_eq!(
        w.copies.dropped.load(Ordering::SeqCst),
        1,
        "the copy is gone"
    );
    let record = record.expect("a verdict");
    assert_eq!(record.outcome, GateOutcome::Pass);
    assert_eq!(record.findings.hidden, None, "no hidden checks yet");
    // The tamper scan starts where the session started.
    assert_eq!(*lock(&w.bases), [Some("base-sha".to_string())]);
}

#[test]
fn a_failing_hidden_check_blocks_and_is_counted_never_named() {
    let w = world();
    w.holdout.hide(SECRET);
    w.holdout.hide("other_hidden");
    w.checks.fail(SECRET);
    w.start();
    let (answer, record) = w.stop();
    let StopAnswer::Block { reason } = answer else {
        panic!("a failing hidden check must block");
    };
    assert!(reason.contains("hidden checks: 1 of 2 failed"), "{reason}");
    assert!(!reason.contains(SECRET), "never named: {reason}");
    assert!(!reason.contains("--test"), "never its command: {reason}");
    let record = record.unwrap();
    assert_eq!(
        record.findings.hidden,
        Some(HiddenTally { ran: 2, passed: 1 })
    );
    assert!(record.findings.runs.iter().all(|r| r.name != SECRET));
    // Hidden files go into the copy after the visible checks ran there.
    let ran: Vec<String> = lock(&w.checks.ran).iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(ran[0], "cargo test");
    let placed = lock(&w.holdout.placed).clone();
    assert!(placed.iter().all(|(_, d)| d == Path::new("/copy")));

    w.checks.pass(SECRET);
    let (answer, record) = w.stop();
    assert_eq!(answer, StopAnswer::Allow);
    let record = record.unwrap();
    assert_eq!(
        record.findings.hidden,
        Some(HiddenTally { ran: 2, passed: 2 })
    );
    assert!(record.digest.contains("2 hidden"), "{}", record.digest);
}

#[test]
fn hidden_files_that_changed_since_they_were_added_fail_their_check() {
    let w = world();
    w.holdout.hide(SECRET);
    lock(&w.holdout.tampered).insert(SECRET.into());
    w.start();
    let (answer, record) = w.stop();
    assert!(matches!(answer, StopAnswer::Block { .. }));
    let record = record.unwrap();
    assert_eq!(
        record.findings.hidden,
        Some(HiddenTally { ran: 1, passed: 0 })
    );
    let problem = record.problem.unwrap();
    assert!(problem.contains("no longer match their pins"), "{problem}");
    assert!(!problem.contains(SECRET));
    assert!(
        lock(&w.checks.ran).iter().all(|(n, _)| n != SECRET),
        "not run"
    );
}

#[test]
fn tests_edited_or_switched_off_block_the_stop() {
    let w = world();
    *lock(&w.copies.diff) = "diff --git a/tests/a.rs b/tests/a.rs\n--- a/tests/a.rs\n+++ b/tests/a.rs\n@@ -1 +1,2 @@\n+#[ignore]\n #[test]\n".into();
    w.start();
    let (answer, record) = w.stop();
    let StopAnswer::Block { reason } = answer else {
        panic!("tampering must block");
    };
    assert!(reason.contains("switched off"), "{reason}");
    assert_eq!(record.unwrap().findings.tampering.len(), 1);

    // A repository can allow test edits.
    lock(&w.source.spec).verifier.block_test_edits = false;
    let (answer, _) = w.stop();
    assert_eq!(answer, StopAnswer::Allow);
}

#[test]
fn protected_files_are_compared_by_git_from_the_first_commit() {
    let w = world();
    lock(&w.source.spec).protected = vec!["tests/oracles/**".into()];
    lock(&w.source.spec).verifier.block_test_edits = false;
    w.source
        .set_pins("/w", &[("tests/oracles/expected.txt", "a")]);
    w.start();
    // The worktree's file is back as it was, but a commit changed it; and
    // a protected file was moved away. Files swarmux writes don't count.
    *lock(&w.copies.diff) = "diff --git a/tests/oracles/expected.txt b/tests/oracles/expected.txt\n--- a/tests/oracles/expected.txt\n+++ b/tests/oracles/expected.txt\n@@ -1 +1 @@\n-42\n+41\n\
diff --git a/tests/oracles/old.txt b/elsewhere/old.txt\nsimilarity index 100%\nrename from tests/oracles/old.txt\nrename to elsewhere/old.txt\n\
diff --git a/.swarmux/missions/m.toml b/.swarmux/missions/m.toml\nnew file mode 100644\n--- /dev/null\n+++ b/.swarmux/missions/m.toml\n@@ -0,0 +1 @@\n+goal = \"g\"\n".into();
    let (answer, record) = w.stop();
    assert!(matches!(answer, StopAnswer::Block { .. }));
    assert_eq!(
        record.unwrap().findings.pins,
        [
            PinChange::Changed("tests/oracles/expected.txt".into()),
            PinChange::Removed("tests/oracles/old.txt".into()),
        ]
    );
}

#[test]
fn without_git_or_with_the_verifier_off_the_checks_run_in_place() {
    let w = world();
    w.holdout.hide(SECRET);
    lock(&w.source.spec).verifier.clean = false;
    w.start();
    let (_, record) = w.stop();
    assert_eq!(w.checks.roots(), [PathBuf::from("/w")]);
    assert_eq!(record.unwrap().findings.hidden, None);
    assert_eq!(w.copies.made.load(Ordering::SeqCst), 0);
}

#[test]
fn when_no_copy_can_be_made_the_checks_still_run_and_the_user_is_told() {
    let w = world();
    *lock(&w.copies.broken) = true;
    w.start();
    let (answer, record) = w.stop();
    assert_eq!(answer, StopAnswer::Allow);
    assert_eq!(w.checks.roots(), [PathBuf::from("/w")]);
    let problem = record.unwrap().problem.unwrap();
    assert!(problem.contains("no clean copy"), "{problem}");
}

#[test]
fn a_manual_verification_runs_the_hidden_checks_too() {
    let w = world();
    lock(&w.source.spec).checks.clear();
    w.holdout.hide(SECRET);
    use crate::gate::JobGates;
    w.keeper.verify(w.job, Path::new("/w")).unwrap();
    let record = w
        .records
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(record.outcome, GateOutcome::Pass);
    assert_eq!(
        record.findings.hidden,
        Some(HiddenTally { ran: 1, passed: 1 })
    );
}

#[test]
fn agents_may_not_look_at_the_hidden_checks_or_the_copies() {
    let w = world();
    let refuses = |tool: &str, subject: &str, cwd: &str| w.keeper.refuses(None, tool, subject, cwd);
    for (tool, subject) in [
        ("Read", "C:\\Users\\me\\.swarmux\\holdout\\w\\x\\check.toml"),
        ("Grep", "c:/users/me/.swarmux/holdout"),
        ("Bash", "cat ~/.swarmux/holdout/w/x/files/tests/x.rs"),
        (
            "PowerShell",
            "Get-Content $env:USERPROFILE\\.swarmux\\holdout\\w",
        ),
        ("Glob", "C:\\Users\\me\\.swarmux\\verify\\w\\3\\**"),
    ] {
        let why = refuses(tool, subject, "/w").unwrap_or_else(|| panic!("{tool} {subject}"));
        assert!(why.contains("verifier"), "{why}");
    }
    assert!(refuses("Bash", "ls", "C:\\Users\\me\\.swarmux\\verify\\w\\3").is_some());
    assert_eq!(refuses("Read", "/w/src/lib.rs", "/w"), None);
    assert_eq!(refuses("Bash", "cargo test", "/w"), None);
    assert!(refuses("Bash", "swarmux holdout list w", "/w").is_some());
    assert!(refuses("PowerShell", "& swarmux.exe HOLDOUT list", "/w").is_some());
    assert_eq!(refuses("Read", "/w/docs/swarmux-holdout.md", "/w"), None);
}

#[test]
fn fenced_folders_match_by_their_last_two_parts() {
    let fenced = [PathBuf::from("/home/u/.swarmux/holdout")];
    assert!(fenced_folder(&fenced, "cat $HOME/.swarmux/holdout/x", "/w").is_some());
    assert!(fenced_folder(&fenced, "cat /home/u/.swarmux/holdouts", "/w").is_some());
    assert!(fenced_folder(&fenced, "cat /home/u/.swarmux/config.toml", "/w").is_none());
    assert!(fenced_folder(&[PathBuf::new()], "anything", "/w").is_none());
}
