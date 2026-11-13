//! The independent verifier: the gate's checks run in a clean copy of the
//! work that swarmux makes and owns, never in the agent's worktree, followed
//! by the repository's hidden checks.
//!
//! The copy holds what the worktree holds, committed or not, but nothing
//! git ignores: no build output, caches or files planted where git doesn't
//! look. Before anything runs, the change since the job started is scanned
//! for tests that were edited or switched off and for the harness (test
//! runner configuration, build scripts, plugins) that was created or
//! changed. The hidden checks come from a store outside every repository;
//! their files are hash-checked, placed into the copy only after the visible
//! checks ran, and only how many ran and passed is ever reported.
//!
//! The base suite and the hidden checks do not run on what the agent left:
//! the harness and the tests it changed are put back as the commit the job
//! started from had them first (what the agent created there is removed),
//! so a planted `conftest.py` or a rewritten runner does not decide them.
//! When that is not possible they do not run, and the verdict says so.
//!
//! The ports live here: [`CleanCheckout`] makes the copies, [`HoldoutStore`]
//! keeps the hidden checks.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use swarmux_domain::gate::{
    CheckRun, DependencyChanges, GateCheck, GateFindings, GateSpec, HiddenNotRun, PinChange, RanIn,
    RevertProbe, WorktreeChecks, protected_glob,
};
use swarmux_domain::verifier::substrate::{self, SubstrateScan};
use swarmux_domain::verifier::weakening::{Weakening, detect};
use swarmux_domain::verifier::{
    BaseSuite, FileDiff, HiddenTally, HoldoutCheck, is_test_path, parse_diff, protected_changes,
};

use crate::AppResult;
use crate::gate::{Checker, run_checks};

mod selftest;
pub use selftest::{Probe, SelfTest};

/// A clean copy of a repository's current work that lives while it is held.
pub trait CleanCopy: Send {
    fn path(&self) -> &Path;
    /// The holdout key of the repository the copy was made from: its main
    /// checkout's folder name and what the repository is (its remote and
    /// root commit), also for a worktree.
    fn repo(&self) -> &str;
    /// The change from `base` to what the copy holds, as a unified diff.
    /// Without a base: from the last commit, so only work not yet committed.
    fn diff_from(&self, base: Option<&str>) -> AppResult<String>;
    /// Puts `paths` back as the commit `base` had them, over what the copy
    /// holds: a file `base` had is written as it was, a file it did not
    /// have is removed. Only the copy changes.
    fn restore(&self, base: &str, paths: &[String]) -> AppResult<()>;
    /// The commit the worktree was at when the copy was made; what
    /// "unchanged" means when no base is known.
    fn head(&self) -> Option<&str>;
    /// `path` as the commit `commit` has it; `None` when it has no such file.
    fn show(&self, commit: &str, path: &str) -> AppResult<Option<Vec<u8>>>;
    /// `path` as the copy holds it now; `None` when it holds no such file.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Writes `path` into the copy (a folder it needs is made). The path
    /// is relative to the copy and stays inside it.
    fn write(&self, path: &str, bytes: &[u8]) -> AppResult<()>;
    /// Removes `path` from the copy, if it holds it.
    fn remove(&self, path: &str) -> AppResult<()>;
    /// Every file the copy holds, relative to it, `/`-separated.
    fn files(&self) -> AppResult<Vec<String>>;
    /// Puts the copy back as it was made: every file as the work had it,
    /// nothing else. What running the checks wrote into it (a build script
    /// that rewrites the source, files a test left) is gone.
    fn reset(&self) -> AppResult<()>;
}

/// Makes clean copies of work in progress.
pub trait CleanCheckout: Send + Sync {
    /// The commit the repository `dir` lies in is at; `None` outside one.
    fn head(&self, dir: &Path) -> Option<String>;
    /// The holdout key of the repository `dir` lies in (see
    /// [`CleanCopy::repo`]); `None` outside one.
    fn repo(&self, dir: &Path) -> Option<String>;
    /// Everything the repository `dir` lies in holds now, committed or
    /// not, without what git ignores, in a folder of swarmux's own. The
    /// repository's files and index are not touched.
    fn snapshot(&self, dir: &Path) -> AppResult<Box<dyn CleanCopy>>;
    /// Where the copies are made. Agents have no business there.
    fn root(&self) -> PathBuf;
}

/// Where hidden checks are kept, per repository, outside every repository.
pub trait HoldoutStore: Send + Sync {
    /// The repository's hidden checks, by name.
    fn list(&self, repo: &str) -> AppResult<Vec<HoldoutCheck>>;
    /// Keeps `check` with `files` (paths relative to the repository root
    /// and their content), pinning each file's hash. Replaces a check of
    /// the same name.
    fn add(
        &self,
        repo: &str,
        check: &GateCheck,
        files: &[(String, Vec<u8>)],
    ) -> AppResult<HoldoutCheck>;
    /// Whether there was such a check.
    fn remove(&self, repo: &str, name: &str) -> AppResult<bool>;
    /// Copies the check's files into `dest`, each compared with its pin
    /// first; an error when one no longer matches.
    fn place(&self, repo: &str, check: &HoldoutCheck, dest: &Path) -> AppResult<()>;
    /// The folder the store keeps. Agents may not see it.
    fn root(&self) -> PathBuf;
}

/// What a clean run found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verification {
    /// The visible checks that ran (those at the gate's level).
    pub runs: Vec<CheckRun>,
    /// Protected files the copy holds differently from the commit the job
    /// started from, as git compares them (line endings normalised).
    pub protected: Vec<PinChange>,
    pub hidden: Option<HiddenTally>,
    /// Hidden checks that exist and did not run, and why.
    pub hidden_not_run: Option<HiddenNotRun>,
    /// Evidence lines for tests weakened since the job started.
    pub tampering: Vec<String>,
    /// Whether any of them touches what the checks run.
    pub checks_weakened: bool,
    /// Evidence lines for harness files created, changed or removed.
    pub substrate: Vec<String>,
    /// New or changed dependencies, one line per manifest or lockfile.
    pub dependencies: Vec<String>,
    /// The checks against the original tests, when the job changed any.
    pub base_suite: Option<BaseSuite>,
    /// The revert probe, when the repository asks for it.
    pub revert_probe: Option<RevertProbe>,
    /// Where the visible checks ran; `None` when none ran.
    pub ran_in: Option<RanIn>,
    /// The checks planned at the gate's level, and those left out.
    pub planned: Vec<String>,
    pub left_out: Vec<(String, String)>,
    /// The checks whose failure blocks nothing.
    pub optional: Vec<String>,
    /// Why the change could not be scanned for tampering.
    pub unscanned: Option<String>,
    /// What went wrong around the checks, for the user only (never names a
    /// hidden check).
    pub problem: Option<String>,
}

impl Verification {
    /// What the gate judges, from what the verifier found and what only
    /// the caller knows (pins, claims and the gate's own configuration are
    /// filled in by the caller).
    pub fn into_findings(self, spec: &GateSpec, unattended: bool) -> GateFindings {
        GateFindings {
            runs: self.runs,
            pins: self.protected,
            hidden: self.hidden,
            tampering: self.tampering,
            checks_weakened: self.checks_weakened,
            base_suite: self.base_suite,
            substrate: self.substrate,
            dependencies: self.dependencies,
            block_dependencies: spec.verifier.dependencies == DependencyChanges::Block,
            hidden_not_run: self.hidden_not_run,
            ran_in: self.ran_in,
            planned: self.planned,
            left_out: self.left_out,
            optional: self.optional,
            revert_probe: self.revert_probe,
            unscanned: self.unscanned,
            unattended,
            worktree_allowed: spec.verifier.worktree_checks == WorktreeChecks::Allow,
            flake_policy: (spec.verifier.flaky_after, spec.verifier.flaky_window),
            policy_sha256: spec.policy_sha256(),
            warnings: spec.warnings.clone(),
            ..GateFindings::default()
        }
    }
}

/// The checks a gate runs at its level, those it leaves out (with why) and
/// the names of the ones whose failure blocks nothing.
pub fn plan_checks(spec: &GateSpec) -> (Vec<GateCheck>, Vec<(String, String)>, Vec<String>) {
    let (run, left) = spec.planned();
    let left_out = left
        .iter()
        .map(|c| {
            (
                c.name.clone(),
                format!(
                    "level {} is above the gate's {}",
                    c.level.as_str(),
                    spec.level.as_str()
                ),
            )
        })
        .collect();
    let optional = spec
        .checks
        .iter()
        .filter(|c| !c.required)
        .map(|c| c.name.clone())
        .collect();
    (run.into_iter().cloned().collect(), left_out, optional)
}

/// What to verify.
pub struct VerifyRequest<'a> {
    /// The repository's root.
    pub dir: &'a Path,
    pub spec: &'a GateSpec,
    /// The commit the job started from.
    pub base: Option<&'a str>,
    /// Run the visible checks.
    pub visible: bool,
    /// Run the hidden checks.
    pub hidden: bool,
}

/// Runs checks in clean copies; shared with the hook transport's threads.
#[derive(Clone)]
pub struct Verifier {
    checkouts: Arc<dyn CleanCheckout>,
    checker: Arc<dyn Checker>,
    holdout: Option<Arc<dyn HoldoutStore>>,
}

/// How the copy is made ready for the hidden checks: from what the agent
/// left, with the harness and the tests as the base had them.
struct HiddenSetup<'a> {
    /// Makes the copy ready again.
    prepare: &'a dyn Fn(&dyn CleanCopy) -> Result<(), String>,
    /// The copy is ready already, with this result, when nothing ran in it
    /// since it was made so.
    ready: Option<Result<(), String>>,
}

/// What the harness scan and the files to put back came to.
#[derive(Default)]
struct Harness {
    scan: SubstrateScan,
    /// Existing tests and protected files the job changed (the base suite).
    tests: Vec<String>,
    /// Harness files that were reported: put back, or removed when new.
    substrate: Vec<String>,
    /// Test files the job created: removed before hidden checks run.
    created_tests: Vec<String>,
}

fn text_of(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
}

impl Verifier {
    pub fn new(checkouts: Arc<dyn CleanCheckout>, checker: Arc<dyn Checker>) -> Verifier {
        Verifier {
            checkouts,
            checker,
            holdout: None,
        }
    }

    /// Runs the hidden checks `store` keeps after the visible ones.
    pub fn with_holdout(mut self, store: Arc<dyn HoldoutStore>) -> Verifier {
        self.holdout = Some(store);
        self
    }

    /// The commit `dir` is at, when it is in a git repository.
    pub fn head(&self, dir: &Path) -> Option<String> {
        self.checkouts.head(dir)
    }

    /// How many hidden checks the repository `dir` lies in has; an
    /// unreadable store counts as one (which then fails closed). `None`
    /// when there are none or `dir` is not in git.
    pub fn hidden_planned(&self, dir: &Path) -> Option<u32> {
        let repo = self.checkouts.repo(dir)?;
        let store = self.holdout.as_ref()?;
        match store.list(&repo) {
            Ok(c) if c.is_empty() => None,
            Ok(c) => Some(u32::try_from(c.len()).unwrap_or(u32::MAX)),
            Err(_) => Some(1),
        }
    }

    /// Whether a clean run of `dir` has anything to do: it is in git, and
    /// there are visible checks or (when `hidden`) hidden ones.
    pub fn applies(&self, dir: &Path, spec: &GateSpec, hidden: bool) -> bool {
        if self.checkouts.repo(dir).is_none() {
            return false;
        }
        !spec.checks.is_empty() || (hidden && self.hidden_planned(dir).is_some())
    }

    /// Folders agents may neither read nor write: the hidden checks and the
    /// clean copies (hidden files are placed there while they run).
    pub fn fenced(&self) -> Vec<PathBuf> {
        let mut out = vec![self.checkouts.root()];
        out.extend(self.holdout.as_ref().map(|h| h.root()));
        out
    }

    /// What the change did to the harness and the dependencies, and which
    /// files that means putting back.
    fn harness(
        &self,
        copy: &dyn CleanCopy,
        spec: &GateSpec,
        base: Option<&str>,
        diff: &str,
        files: &[FileDiff],
    ) -> Harness {
        let fold = cfg!(windows);
        let mut allow = spec.allow_substrate.clone();
        allow.extend(spec.allow_test_changes.iter().cloned());
        let old = |p: &str| base.and_then(|b| text_of(copy.show(b, p).ok().flatten()));
        let new = |p: &str| text_of(copy.read(p));
        let scan = substrate::scan(files, &spec.substrate_globs(), &allow, &old, &new, fold);
        let mut substrate_paths: BTreeSet<String> =
            scan.changes.iter().map(|c| c.path.clone()).collect();
        // A file moved onto a harness name is created there; its old place
        // is put back too.
        for f in files {
            if let Some(from) = &f.renamed_from
                && protected_glob(&spec.substrate_globs(), from, fold).is_some()
            {
                substrate_paths.insert(from.clone());
            }
        }
        let globs = spec.protected_globs();
        let created_tests = files
            .iter()
            .filter(|f| f.created || f.renamed_from.is_some())
            .map(|f| f.path.clone())
            .filter(|p| {
                !p.starts_with(".swarmux/")
                    && (is_test_path(p) || protected_glob(&globs, p, fold).is_some())
                    && protected_glob(&spec.allow_test_changes, p, fold).is_none()
            })
            .collect();
        Harness {
            scan,
            tests: base_suite_paths(diff, spec),
            substrate: substrate_paths.into_iter().collect(),
            created_tests,
        }
    }

    /// Makes a clean copy of `request.dir`'s work and checks it. An error
    /// when no copy could be made.
    pub fn verify(&self, request: &VerifyRequest<'_>) -> Result<Verification, String> {
        let copy = self
            .checkouts
            .snapshot(request.dir)
            .map_err(|e| e.to_string())?;
        let spec = request.spec;
        let mut out = Verification::default();
        let (planned, left_out, optional) = plan_checks(spec);
        out.planned = planned.iter().map(|c| c.name.clone()).collect();
        out.left_out = left_out;
        out.optional = optional;
        // What "as it was" means: the commit the job started from, else the
        // last commit of the worktree.
        let base_commit: Option<String> = request
            .base
            .map(str::to_string)
            .or_else(|| copy.head().map(str::to_string));
        let mut harness = Harness::default();
        let mut diff_text = None;
        match copy.diff_from(request.base) {
            Ok(diff) => {
                let globs = spec.protected_globs();
                out.protected = protected_changes(&diff, &globs, cfg!(windows));
                if spec.verifier.block_test_edits {
                    let found = detect(&diff, &spec.allow_test_changes);
                    out.checks_weakened = found
                        .iter()
                        .any(|w| w.affects_checks(&globs, &spec.checks, cfg!(windows)));
                    out.tampering = found.iter().map(Weakening::describe).collect();
                }
                let files = parse_diff(&diff);
                harness = self.harness(&*copy, spec, base_commit.as_deref(), &diff, &files);
                out.substrate = harness
                    .scan
                    .changes
                    .iter()
                    .map(substrate::SubstrateChange::describe)
                    .collect();
                out.dependencies = harness.scan.dependencies.clone();
                diff_text = Some(diff);
            }
            Err(e) => {
                out.problem = Some(format!("verifier: no diff to scan: {e}"));
                out.unscanned = Some(format!("the change could not be read ({e})"));
            }
        }
        if request.visible {
            out.runs = run_checks(&*self.checker, &planned, copy.path());
            if !out.runs.is_empty() {
                out.ran_in = Some(RanIn::CleanCopy);
            }
        }
        let passing = out
            .runs
            .iter()
            .all(|r| r.passed() || out.optional.contains(&r.name));
        // From here on the copy is made again from what the agent left
        // (whatever running the checks wrote into it is gone) with the
        // harness and the tests as the commit the job started from had
        // them. Before the hidden files are placed: a check that runs
        // everything must not print a hidden test's name into what the
        // agent is told.
        let to_restore: Vec<String> = harness
            .tests
            .iter()
            .chain(&harness.substrate)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let prepare = |copy: &dyn CleanCopy| -> Result<(), String> {
            copy.reset()
                .map_err(|e| format!("the copy could not be made again: {e}"))?;
            match (&base_commit, to_restore.is_empty()) {
                (_, true) => Ok(()),
                (Some(base), false) => copy.restore(base, &to_restore).map_err(|e| e.to_string()),
                (None, false) => Err("no commit to put the files back from".to_string()),
            }
        };
        let restored = prepare(&*copy);
        // Only when the visible checks passed: a failure is already a block.
        if request.visible
            && passing
            && spec.verifier.base_suite
            && request.base.is_some()
            && !planned.is_empty()
            && !harness.tests.is_empty()
            && diff_text.is_some()
        {
            match &restored {
                Ok(()) => {
                    out.base_suite = Some(run_base_suite(
                        &*self.checker,
                        &planned,
                        copy.path(),
                        &harness.tests,
                    ));
                }
                Err(e) => out.problem = Some(format!("verifier: base suite not run: {e}")),
            }
        }
        if request.hidden {
            // The base suite ran on this copy: the hidden checks need it
            // again from what the agent left, with nothing it did in the
            // meantime (made when it is known there are hidden checks).
            let ready = (out.base_suite.is_none()).then_some(restored);
            self.run_hidden(
                copy.as_ref(),
                base_commit.as_deref(),
                HiddenSetup {
                    prepare: &prepare,
                    ready,
                },
                &harness.created_tests,
                &mut out,
            );
        }
        if request.visible && passing && spec.verifier.revert_probe {
            out.revert_probe =
                self.revert_probe(request, &harness, base_commit.as_deref(), &planned);
        }
        drop(copy);
        Ok(out)
    }

    /// Places every hidden check's files into the copy with the harness as
    /// the base had it, then runs each. A check whose files no longer match
    /// their pins counts as failed. When the harness cannot be put back
    /// they do not run, and the outcome says why.
    fn run_hidden(
        &self,
        copy: &dyn CleanCopy,
        base: Option<&str>,
        setup: HiddenSetup<'_>,
        created_tests: &[String],
        out: &mut Verification,
    ) {
        let Some(store) = self.holdout.as_ref() else {
            return;
        };
        let checks = match store.list(copy.repo()) {
            Ok(c) => c,
            Err(e) => {
                // Fail closed: hidden checks that can't be read didn't pass.
                out.problem = Some(format!("verifier: hidden checks unreadable: {e}"));
                out.hidden = Some(HiddenTally { ran: 1, passed: 0 });
                return;
            }
        };
        if checks.is_empty() {
            return;
        }
        let planned = u32::try_from(checks.len()).unwrap_or(u32::MAX);
        let not_run = |reason: String, out: &mut Verification| {
            out.hidden_not_run = Some(HiddenNotRun { planned, reason });
        };
        let restored = setup.ready.unwrap_or_else(|| (setup.prepare)(copy));
        if let Err(why) = restored {
            not_run(
                format!("the test harness could not be put back: {why}"),
                out,
            );
            return;
        }
        if !created_tests.is_empty() {
            let Some(base) = base else {
                not_run("no commit to put the tests back from".into(), out);
                return;
            };
            // Tests the agent added are not part of what the hidden
            // checks run beside; they are removed from this copy only.
            if let Err(e) = copy.restore(base, created_tests) {
                not_run(
                    format!("the agent's new tests could not be removed: {e}"),
                    out,
                );
                return;
            }
        }
        let placed: Vec<bool> = checks
            .iter()
            .map(|c| store.place(copy.repo(), c, copy.path()).is_ok())
            .collect();
        let tampered = placed.iter().filter(|ok| !**ok).count();
        if tampered > 0 {
            out.problem = Some(format!(
                "verifier: {tampered} hidden check(s) whose files no longer match their pins"
            ));
        }
        let mut tally = HiddenTally {
            ran: planned,
            passed: 0,
        };
        for (check, ok) in checks.iter().zip(placed) {
            if !ok {
                continue;
            }
            let passed = run_checks(
                &*self.checker,
                std::slice::from_ref(&check.check),
                copy.path(),
            )
            .first()
            .is_some_and(CheckRun::passed);
            if passed {
                tally.passed += 1;
            }
        }
        out.hidden = Some(tally);
    }

    /// The change with its non-test part reverted, run against the tests
    /// the job added or changed: at least one of them has to fail.
    fn revert_probe(
        &self,
        request: &VerifyRequest<'_>,
        harness: &Harness,
        base: Option<&str>,
        checks: &[GateCheck],
    ) -> Option<RevertProbe> {
        let spec = request.spec;
        let fold = cfg!(windows);
        let Some(base) = base else {
            return Some(RevertProbe {
                proved: false,
                detail: "no commit to revert to".into(),
            });
        };
        let copy = match self.checkouts.snapshot(request.dir) {
            Ok(c) => c,
            Err(e) => {
                return Some(RevertProbe {
                    proved: false,
                    detail: format!("no second copy could be made ({e})"),
                });
            }
        };
        let diff = match copy.diff_from(Some(base)) {
            Ok(d) => d,
            Err(e) => {
                return Some(RevertProbe {
                    proved: false,
                    detail: format!("no diff to revert ({e})"),
                });
            }
        };
        let files = parse_diff(&diff);
        let globs = spec.protected_globs();
        let tests = |p: &str| is_test_path(p) || protected_glob(&globs, p, fold).is_some();
        let touched_tests = files.iter().any(|f| !f.deleted && tests(&f.path));
        let reverted: Vec<String> = files
            .iter()
            .filter(|f| !f.path.starts_with(".swarmux/") && !tests(&f.path))
            .filter(|f| {
                protected_glob(&spec.substrate_globs(), &f.path, fold).is_none()
                    || harness.substrate.contains(&f.path)
            })
            .flat_map(|f| f.renamed_from.iter().cloned().chain([f.path.clone()]))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if reverted.is_empty() {
            return Some(RevertProbe {
                proved: true,
                detail: "only tests changed: there is no change to revert".into(),
            });
        }
        if !touched_tests {
            return Some(RevertProbe {
                proved: false,
                detail: format!(
                    "{} file{} changed and no test was added or changed: nothing shows the change is needed",
                    reverted.len(),
                    if reverted.len() == 1 { "" } else { "s" }
                ),
            });
        }
        if let Err(e) = copy.restore(base, &reverted) {
            return Some(RevertProbe {
                proved: false,
                detail: format!("the change could not be reverted ({e})"),
            });
        }
        let runs = run_checks(&*self.checker, checks, copy.path());
        let failed: Vec<&str> = runs
            .iter()
            .filter(|r| !r.passed())
            .map(|r| r.name.as_str())
            .collect();
        let n = reverted.len();
        let files_text = format!("{n} file{} reverted", if n == 1 { "" } else { "s" });
        Some(if failed.is_empty() {
            RevertProbe {
                proved: false,
                detail: format!("{files_text} and every check still passes"),
            }
        } else {
            RevertProbe {
                proved: true,
                detail: format!(
                    "{files_text}; {} fail without the change",
                    failed.join(", ")
                ),
            }
        })
    }
}

/// The files of `diff` that existed at the base commit and are tests or
/// protected: the ones whose original versions must still pass.
pub fn base_suite_paths(diff: &str, spec: &GateSpec) -> Vec<String> {
    let globs = spec.protected_globs();
    let fold = cfg!(windows);
    let mut out = Vec::new();
    for f in parse_diff(diff) {
        if f.created && f.renamed_from.is_none() {
            continue;
        }
        let path = f.renamed_from.clone().unwrap_or_else(|| f.path.clone());
        let ours = path.starts_with(".swarmux/");
        let wanted = protected_glob(&globs, &path, fold).is_some() || is_test_path(&path);
        let allowed = protected_glob(&spec.allow_test_changes, &path, fold).is_some();
        if !ours && wanted && !allowed {
            out.push(path);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Runs the visible checks again in the copy, which already holds the
/// original versions of `restored`.
fn run_base_suite(
    checker: &dyn Checker,
    checks: &[GateCheck],
    root: &Path,
    restored: &[String],
) -> BaseSuite {
    let failed = run_checks(checker, checks, root)
        .iter()
        .filter(|r| !r.passed())
        .map(|r| {
            let line = r.failure_excerpt().into_iter().next();
            (r.name.clone(), line.unwrap_or_else(|| r.status_text()))
        })
        .collect();
    BaseSuite {
        restored: restored.to_vec(),
        failed,
    }
}

/// Whether a tool call names one of `fenced` (or works inside one); the
/// folder it names. Paths are compared without case and slash direction,
/// and the folder's last two parts are enough (`.swarmux/holdout`), so
/// `~` or `%USERPROFILE%` spellings don't slip by.
pub fn fenced_folder<'a>(fenced: &'a [PathBuf], subject: &str, cwd: &str) -> Option<&'a Path> {
    let norm = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    let subject = norm(subject);
    let cwd = norm(cwd);
    fenced.iter().map(PathBuf::as_path).find(|root| {
        let full = norm(&root.to_string_lossy());
        let full = full.trim_end_matches('/');
        if full.is_empty() {
            return false;
        }
        let parts: Vec<&str> = full.rsplit('/').take(2).collect();
        let tail = parts.into_iter().rev().collect::<Vec<_>>().join("/");
        subject.contains(full)
            || (tail.contains('/') && subject.contains(&tail))
            || cwd == full
            || cwd.starts_with(&format!("{full}/"))
    })
}

/// Whether a shell line runs `swarmux knowledge` or `swarmux rules`, which
/// show the commands of rules' hidden checks.
pub fn runs_knowledge_command(tool: &str, subject: &str) -> bool {
    if !matches!(tool, "Bash" | "PowerShell") {
        return false;
    }
    let s = subject.to_ascii_lowercase();
    s.contains("swarmux") && (s.contains("knowledge") || s.contains(" rules"))
}

/// Whether a shell line runs `swarmux holdout`, which lists hidden checks.
pub fn runs_holdout_command(tool: &str, subject: &str) -> bool {
    if !matches!(tool, "Bash" | "PowerShell") {
        return false;
    }
    let s = subject.to_ascii_lowercase();
    s.contains("swarmux") && s.contains("holdout")
}

#[cfg(test)]
mod tests;
