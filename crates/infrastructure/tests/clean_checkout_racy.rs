//! A same-size edit made right after a commit must be in the clean copy.
//!
//! Git trusts a file whose size and time match its index entry, except when
//! the entry is as new as the index file itself ("racily clean"). The clean
//! copy starts from a copy of the real index; when that copy was stamped with
//! the time of copying instead of the original's time, such entries looked
//! old and an edit that kept the file's size and landed in the same clock
//! tick as the commit (`a - b` to `a + b`) was missing from the snapshot: the
//! verifier then judged work the agent never did. Measured on gimle before
//! the fix: 2 misses in 150 and about 1 in 75 over repeated runs; after it,
//! 0 in 600. This loop makes the window likely enough to catch a return.

use std::path::Path;
use std::process::Command;

use swarmux_application::CleanCheckout;
use swarmux_infrastructure::GitCleanCheckout;

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

#[test]
fn a_same_size_edit_right_after_a_commit_is_in_the_snapshot() {
    let base = tempfile::tempdir().unwrap();
    let checkout = GitCleanCheckout::new("git".into(), base.path().join("verify"));
    for i in 0..300 {
        let root = base.path().join(format!("r{i}"));
        std::fs::create_dir_all(&root).unwrap();
        if !git(&root, &["init", "-q", "-b", "main"]) {
            eprintln!("git not available; skipping");
            return;
        }
        std::fs::write(root.join("a.txt"), "a - b\n").unwrap();
        assert!(git(&root, &["add", "."]));
        assert!(git(&root, &["commit", "-q", "-m", "seed"]));
        std::fs::write(root.join("a.txt"), "a + b\n").unwrap();
        let copy = checkout.snapshot(&root).unwrap();
        let got = std::fs::read_to_string(copy.path().join("a.txt")).unwrap();
        assert_eq!(
            got, "a + b\n",
            "round {i}: the edit is missing from the copy"
        );
    }
}
