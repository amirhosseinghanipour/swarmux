//! `GitRepoRoots` against real `git`. It lives here and not in the library's
//! unit tests because it starts git processes: a child forked while another
//! test of the library's binary holds a lock file keeps that lock until it
//! execs, and `daemon::lock`'s test would then see the lock still held after
//! it let go. Integration tests run in a binary of their own.

use std::path::Path;

use swarmux_application::ports::RepoRoots;
use swarmux_infrastructure::GitRepoRoots;

#[test]
fn git_roots_follow_a_worktree_to_its_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("app");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |cwd: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
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
    };
    if !git(&repo, &["init", "-q"]) {
        eprintln!("git not available; skipping");
        return;
    }
    std::fs::write(repo.join("a.txt"), "x").unwrap();
    assert!(git(&repo, &["add", "."]));
    assert!(git(&repo, &["commit", "-q", "-m", "one"]));
    let tree = dir.path().join("wt");
    assert!(git(
        &repo,
        &["worktree", "add", "-q", tree.to_str().unwrap(), "-b", "w"]
    ));
    let roots = GitRepoRoots::new("git".into());
    let norm = |s: Option<String>| s.map(|s| s.replace('\\', "/").to_lowercase());
    let want_repo = norm(Some(
        std::fs::canonicalize(&repo)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ))
    .map(|s| s.trim_start_matches("//?/").to_string());
    let in_tree = tree.to_string_lossy().into_owned();
    let main =
        norm(roots.repo_root(None, &in_tree)).map(|s| s.trim_start_matches("//?/").to_string());
    assert_eq!(
        main, want_repo,
        "a worktree belongs to the repository it was made from"
    );
    let top = norm(roots.tree_root(&in_tree)).unwrap();
    assert!(top.ends_with("/wt"), "{top}");
    assert_eq!(roots.repo_root(None, "/definitely/not/a/folder"), None);
}
