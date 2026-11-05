//! The merge queue against real git: task worktrees branched from the
//! integration branch, clean merges, a conflict that is undone at once,
//! a rebase that fixes it, and the user's own branch never touched.

use std::fs;
use std::path::Path;
use std::process::Command;

use swarmux_application::{AddWorktree, MergeOutcome, MergeQueue, Worktrees};
use swarmux_infrastructure::{GitMergeQueue, GitWorktrees};

fn git_out(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.editor=true"])
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .expect("git")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_out(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn tasks_merge_in_order_and_a_conflict_is_undone_then_rebased() {
    if !have_git() {
        eprintln!("git not available; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "user.email", "t@t"]);
    fs::write(repo.join("shared.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let user_head = git(&repo, &["rev-parse", "main"]);

    let queue = GitMergeQueue::under(dir.path().join("wt"));
    let branch = "swarmux/split/integration";
    let integ = queue.open(&repo, branch).unwrap();
    assert!(integ.join("shared.txt").is_file());
    assert_eq!(queue.open(&repo, branch).unwrap(), integ, "opened once");
    assert_eq!(git(&repo, &["rev-parse", branch]), user_head);

    let wts = GitWorktrees::default();
    let add = |n: u32| {
        let path = dir.path().join("wt").join(format!("task-{n}"));
        let b = format!("swarmux/split-t{n}/{n}");
        wts.add(&AddWorktree {
            repo: repo.clone(),
            path: path.clone(),
            branch: b.clone(),
            copy: Vec::new(),
            base: Some(branch.to_string()),
        })
        .unwrap();
        (path, b)
    };
    let (p1, b1) = add(1);
    let (p2, b2) = add(2);
    assert_eq!(
        git(&p1, &["rev-parse", "HEAD"]),
        user_head,
        "from the integration branch"
    );

    fs::write(p1.join("a.txt"), "a\n").unwrap();
    fs::write(p1.join("shared.txt"), "one\nfrom t1\n").unwrap();
    wts.snapshot(&p1, "t1").unwrap();
    fs::write(p2.join("b.txt"), "b\n").unwrap();
    fs::write(p2.join("shared.txt"), "one\nfrom t2\n").unwrap();
    wts.snapshot(&p2, "t2").unwrap();

    let mut changed = queue.changed(&integ, &b1).unwrap();
    changed.sort();
    assert_eq!(changed, vec!["a.txt", "shared.txt"]);
    assert!(!queue.contains(&integ, &b1).unwrap());
    let MergeOutcome::Merged { commit } = queue.merge(&integ, &b1, "merge t1").unwrap() else {
        panic!("t1 merges clean");
    };
    assert_eq!(git(&repo, &["rev-parse", branch]), commit);
    // Once in, it is seen to be in: a restart never merges it again.
    assert!(queue.contains(&integ, &b1).unwrap());
    assert!(!queue.contains(&integ, &b2).unwrap());

    // t2 touched the same line: the merge conflicts and is undone.
    let before = git(&integ, &["rev-parse", "HEAD"]);
    match queue.merge(&integ, &b2, "merge t2").unwrap() {
        MergeOutcome::Conflict { files } => assert_eq!(files, vec!["shared.txt"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(git(&integ, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(&integ, &["status", "--porcelain"]), "");
    // Only t2's own files count against its claim.
    let mut changed = queue.changed(&integ, &b2).unwrap();
    changed.sort();
    assert_eq!(changed, vec!["b.txt", "shared.txt"]);

    // What the re-invoked agent does: rebase onto the integration branch
    // and resolve.
    assert!(!git_out(&p2, &["rebase", branch]).status.success());
    fs::write(p2.join("shared.txt"), "one\nfrom t1\nfrom t2\n").unwrap();
    git(&p2, &["add", "shared.txt"]);
    git(&p2, &["rebase", "--continue"]);
    assert!(matches!(
        queue.merge(&integ, &b2, "merge t2").unwrap(),
        MergeOutcome::Merged { .. }
    ));
    assert_eq!(
        fs::read_to_string(integ.join("shared.txt"))
            .unwrap()
            .replace("\r\n", "\n"),
        "one\nfrom t1\nfrom t2\n"
    );

    // The last task's branch is brought up to the integration branch.
    queue.fast_forward(&p2, branch).unwrap();
    assert_eq!(
        git(&p2, &["rev-parse", "HEAD"]),
        git(&repo, &["rev-parse", branch])
    );
    assert!(p2.join("a.txt").is_file());
    // The user's branch never moved.
    assert_eq!(git(&repo, &["rev-parse", "main"]), user_head);
}
