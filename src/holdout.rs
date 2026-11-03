//! `swarmux holdout`: hidden checks the verifier runs in clean copies and
//! agents never see. They live in `~/.swarmux/holdout/<repo>/`, outside
//! every repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use swarmux_application::{Checker, CleanCheckout, HoldoutStore, Verifier};
use swarmux_domain::gate::{DEFAULT_CHECK_TIMEOUT_SECS, GateCheck};
use swarmux_domain::verifier::{holdout_path, valid_name};
use swarmux_infrastructure::{FsHoldoutStore, GitCleanCheckout, Paths, ProcessChecker};

/// The verifier the app and `swarmux gate run` use.
pub fn verifier(paths: &Paths, checker: Arc<dyn Checker>) -> Verifier {
    Verifier::new(
        Arc::new(GitCleanCheckout::new("git".into(), paths.verify.clone())),
        checker,
    )
    .with_holdout(Arc::new(FsHoldoutStore::new(paths.holdout.clone())))
}

/// The plain checker, for callers that build their own verifier.
pub fn checker() -> Arc<dyn Checker> {
    Arc::new(ProcessChecker::new())
}

fn store() -> anyhow::Result<FsHoldoutStore> {
    Ok(FsHoldoutStore::new(Paths::default_home()?.holdout))
}

/// A repository given by folder (inside it, or a worktree of it) or by
/// its holdout name.
fn repo_name(repo: &str, paths: &Paths) -> anyhow::Result<String> {
    let dir = Path::new(repo);
    if dir.is_dir() {
        let checkouts = GitCleanCheckout::new("git".into(), paths.verify.clone());
        return checkouts
            .repo(dir)
            .with_context(|| format!("{} is not in a git repository", dir.display()));
    }
    valid_name(repo).map_err(|e| anyhow::anyhow!("repository name {e}"))?;
    // Names are kept lowercased, as they are found from a folder.
    Ok(repo.to_ascii_lowercase())
}

/// `tests/hidden.rs=C:\keep\hidden.rs` (where it goes = where it is now),
/// or a path that is both.
fn file_arg(arg: &str) -> anyhow::Result<(String, PathBuf)> {
    let (dest, source) = match arg.split_once('=') {
        Some((dest, source)) => (dest, PathBuf::from(source)),
        None => (arg, PathBuf::from(arg)),
    };
    let dest = holdout_path(dest).map_err(|e| anyhow::anyhow!(e))?;
    Ok((dest, source))
}

pub fn add(
    repo: &str,
    check: &str,
    files: &[String],
    timeout: Option<u64>,
    run: &[String],
) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let repo = repo_name(repo, &paths)?;
    let (program, args) = run
        .split_first()
        .context("give the command after --, e.g. -- cargo test --test hidden")?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut gate_check = GateCheck::new(check, program, &args);
    gate_check.timeout_secs = match timeout {
        Some(0) => anyhow::bail!("--timeout must be above 0"),
        Some(t) => t,
        None => DEFAULT_CHECK_TIMEOUT_SECS,
    };
    let mut contents = Vec::new();
    for f in files {
        let (dest, source) = file_arg(f)?;
        let bytes =
            std::fs::read(&source).with_context(|| format!("cannot read {}", source.display()))?;
        contents.push((dest, bytes));
    }
    let held = FsHoldoutStore::new(paths.holdout.clone()).add(&repo, &gate_check, &contents)?;
    println!(
        "hidden check {} added to {repo}: {} ({} file{}, pinned)",
        held.name(),
        held.check.command_line(),
        held.files.len(),
        if held.files.len() == 1 { "" } else { "s" }
    );
    for f in &held.files {
        println!("  {}  {}", &f.sha256[..12.min(f.sha256.len())], f.path);
    }
    println!(
        "It runs in clean copies after the visible checks; agents only ever learn how many hidden checks failed."
    );
    if files.is_empty() {
        return Ok(());
    }
    println!(
        "Keep the originals out of the repository (or delete them now): a file in the worktree is a file the agent can read."
    );
    Ok(())
}

pub fn list(repo: Option<&str>) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let store = store()?;
    let repos: Vec<String> = match repo {
        Some(r) => vec![repo_name(r, &paths)?],
        None => match std::fs::read_dir(&paths.holdout) {
            Ok(entries) => {
                let mut names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                names
            }
            Err(_) => Vec::new(),
        },
    };
    let mut any = false;
    for repo in repos {
        let checks = store.list(&repo)?;
        if checks.is_empty() {
            continue;
        }
        any = true;
        println!("{repo}");
        for h in checks {
            println!(
                "  {:<20} {}  (timeout {} s, {} file{})",
                h.name(),
                h.check.command_line(),
                h.check.timeout_secs,
                h.files.len(),
                if h.files.len() == 1 { "" } else { "s" }
            );
        }
    }
    if !any {
        println!(
            "no hidden checks in {}: add one with swarmux holdout add <repo> <check> -- <command>",
            paths.holdout.display()
        );
    }
    Ok(())
}

pub fn rm(repo: &str, check: &str) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let repo = repo_name(repo, &paths)?;
    if store()?.remove(&repo, check)? {
        println!("hidden check {check} removed from {repo}");
        Ok(())
    } else {
        anyhow::bail!("{repo} has no hidden check {check}")
    }
}
