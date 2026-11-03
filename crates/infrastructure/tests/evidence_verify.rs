//! `verify` against a real repository: the checks run again in a clean
//! copy of the commit the bundle names, not in the working folder.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use swarmux_application::{Rerunner, Vcs};
use swarmux_domain::evidence::{BUNDLE_SCHEMA, Bundle, CheckResult};
use swarmux_infrastructure::{GitCheckouts, GitVcs, ProcessChecker};

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The check: the file `ok.txt` exists at the commit being checked.
fn check(passed: bool) -> CheckResult {
    CheckResult {
        name: "ok.txt".into(),
        program: "git".into(),
        args: ["cat-file", "-e", "HEAD:ok.txt"].map(String::from).to_vec(),
        timeout_secs: 60,
        exit: Some(if passed { 0 } else { 128 }),
        passed,
        flaky: false,
        duration_ms: 10,
        first_failure: String::new(),
    }
}

fn bundle(repo: &Path, head: &str, passed: bool) -> Bundle {
    let mut b = Bundle {
        schema: BUNDLE_SCHEMA,
        repo: repo.display().to_string(),
        head_sha: head.into(),
        checks: vec![check(passed)],
        ..Bundle::default()
    };
    b.contract.slug = "demo".into();
    b
}

fn rerunner(base: &Path) -> Rerunner {
    Rerunner::new(
        Arc::new(ProcessChecker::new()),
        Arc::new(GitCheckouts::default().under(base.to_path_buf())),
    )
}

#[test]
fn the_checks_run_at_the_bundles_head_in_a_clean_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    if !git(p, &["init", "-q", "-b", "main"]) {
        eprintln!("git not available; skipping");
        return;
    }
    std::fs::write(p.join("ok.txt"), "fine\n").unwrap();
    assert!(git(p, &["add", "ok.txt"]));
    assert!(git(p, &["commit", "-q", "-m", "with the file"]));
    let good = GitVcs::default().head(p).unwrap().unwrap().head;
    assert!(git(p, &["rm", "-q", "ok.txt"]));
    assert!(git(p, &["commit", "-q", "-m", "without the file"]));
    let bad = GitVcs::default().head(p).unwrap().unwrap().head;
    // The working folder is now at the commit without the file: only a
    // checkout of the named commit can pass.
    assert!(!p.join("ok.txt").exists());
    let scratch = tempfile::tempdir().unwrap();
    let rerun = rerunner(scratch.path());

    let items = rerun.run(&bundle(p, &good, true));
    assert!(items.iter().all(|i| i.ok), "match expected: {items:?}");
    assert!(items.iter().any(|i| i.subject == "check ok.txt"));

    // A bundle claiming a pass at the commit where the file is gone does not hold.
    let items = rerun.run(&bundle(p, &bad, true));
    let miss: Vec<_> = items.iter().filter(|i| !i.ok).collect();
    assert_eq!(miss.len(), 1, "{items:?}");
    assert_eq!(miss[0].expected, "pass");
    assert!(miss[0].found.starts_with("fail"), "{:?}", miss[0]);

    // A failure that is claimed as a failure matches, exit code and all.
    let items = rerun.run(&bundle(p, &bad, false));
    assert!(items.iter().all(|i| i.ok), "{items:?}");

    // A commit that is not in the repository cannot be checked out.
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let items = rerun.run(&bundle(p, missing, true));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].subject, "clean checkout");
    assert!(!items[0].ok);

    // Nothing is left behind.
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
    let listed = Command::new("git")
        .arg("-C")
        .arg(p)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&listed.stdout).lines().count(), 1);
}
