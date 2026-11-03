//! The independent verifier: the gate's checks run in a clean copy of the
//! work that swarmux makes and owns, never in the agent's worktree, followed
//! by the repository's hidden checks.
//!
//! The copy holds what the worktree holds, committed or not, but nothing
//! git ignores: no build output, caches or files planted where git doesn't
//! look. Before anything runs, the change since the job started is scanned
//! for tests that were edited or switched off. The hidden checks come from
//! a store outside every repository; their files are hash-checked, placed
//! into the copy only after the visible checks ran, and only how many ran
//! and passed is ever reported.
//!
//! The ports live here: [`CleanCheckout`] makes the copies, [`HoldoutStore`]
//! keeps the hidden checks.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swarmux_domain::gate::{CheckRun, GateCheck, GateSpec, PinChange};
use swarmux_domain::verifier::{HiddenTally, HoldoutCheck, protected_changes, scan_diff};

use crate::AppResult;
use crate::gate::{Checker, run_checks};

/// A clean copy of a repository's current work that lives while it is held.
pub trait CleanCopy: Send {
    fn path(&self) -> &Path;
    /// The holdout name of the repository the copy was made from: the
    /// name of its main checkout's folder, also for a worktree.
    fn repo(&self) -> &str;
    /// The change from `base` to what the copy holds, as a unified diff.
    /// Without a base: from the last commit, so only work not yet committed.
    fn diff_from(&self, base: Option<&str>) -> AppResult<String>;
}

/// Makes clean copies of work in progress.
pub trait CleanCheckout: Send + Sync {
    /// The commit the repository `dir` lies in is at; `None` outside one.
    fn head(&self, dir: &Path) -> Option<String>;
    /// The holdout name of the repository `dir` lies in (see
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
    /// The visible checks, run in the copy.
    pub runs: Vec<CheckRun>,
    /// Protected files the copy holds differently from the commit the job
    /// started from, as git compares them (line endings normalised).
    pub protected: Vec<PinChange>,
    pub hidden: Option<HiddenTally>,
    /// Tests edited or switched off since the job started.
    pub tampering: Vec<String>,
    /// What went wrong around the checks, for the user only (never names a
    /// hidden check).
    pub problem: Option<String>,
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

    /// Whether a clean run of `dir` has anything to do: it is in git, and
    /// there are visible checks or (when `hidden`) hidden ones.
    pub fn applies(&self, dir: &Path, spec: &GateSpec, hidden: bool) -> bool {
        let Some(repo) = self.checkouts.repo(dir) else {
            return false;
        };
        !spec.checks.is_empty()
            || (hidden
                && self
                    .holdout
                    .as_ref()
                    .is_some_and(|h| h.list(&repo).is_ok_and(|c| !c.is_empty())))
    }

    /// Folders agents may neither read nor write: the hidden checks and the
    /// clean copies (hidden files are placed there while they run).
    pub fn fenced(&self) -> Vec<PathBuf> {
        let mut out = vec![self.checkouts.root()];
        out.extend(self.holdout.as_ref().map(|h| h.root()));
        out
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
        match copy.diff_from(request.base) {
            Ok(diff) => {
                out.protected = protected_changes(&diff, &spec.protected_globs(), cfg!(windows));
                if spec.verifier.block_test_edits {
                    out.tampering = scan_diff(&diff).iter().map(|t| t.describe()).collect();
                }
            }
            Err(e) => out.problem = Some(format!("verifier: no diff to scan: {e}")),
        }
        if request.visible {
            out.runs = run_checks(&*self.checker, &spec.checks, copy.path());
        }
        if request.hidden {
            out.hidden = self.run_hidden(copy.as_ref(), &mut out.problem);
        }
        drop(copy);
        Ok(out)
    }

    /// Places every hidden check's files, then runs each. A check whose
    /// files no longer match their pins counts as failed.
    fn run_hidden(
        &self,
        copy: &dyn CleanCopy,
        problem: &mut Option<String>,
    ) -> Option<HiddenTally> {
        let store = self.holdout.as_ref()?;
        let checks = match store.list(copy.repo()) {
            Ok(c) => c,
            Err(e) => {
                // Fail closed: hidden checks that can't be read didn't pass.
                *problem = Some(format!("verifier: hidden checks unreadable: {e}"));
                return Some(HiddenTally { ran: 1, passed: 0 });
            }
        };
        if checks.is_empty() {
            return None;
        }
        let placed: Vec<bool> = checks
            .iter()
            .map(|c| store.place(copy.repo(), c, copy.path()).is_ok())
            .collect();
        let tampered = placed.iter().filter(|ok| !**ok).count();
        if tampered > 0 {
            *problem = Some(format!(
                "verifier: {tampered} hidden check(s) whose files no longer match their pins"
            ));
        }
        let mut tally = HiddenTally {
            ran: u32::try_from(checks.len()).unwrap_or(u32::MAX),
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
        Some(tally)
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
