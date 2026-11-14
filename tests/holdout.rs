//! `swarmux holdout` as a user runs it: the real binary and a home folder of
//! its own.

use std::path::Path;
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn swarmux(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    Command::new(EXE)
        .args(args)
        .current_dir(cwd)
        .env(var, home)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn hidden_checks_are_added_listed_and_removed() {
    let home = tempfile::tempdir().unwrap();
    // An existing install: its data is where it always was.
    std::fs::create_dir_all(home.path().join(".swarmux")).unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(work.path().join("hidden.rs"), "#[test] fn h() {}\n").unwrap();

    let added = swarmux(
        home.path(),
        work.path(),
        &[
            "holdout",
            "add",
            "Shop",
            "bigger",
            "--file",
            "tests/hidden.rs=hidden.rs",
            "--timeout",
            "120",
            "--",
            "cargo",
            "test",
            "--test",
            "hidden",
        ],
    );
    assert!(added.status.success(), "{}", text(&added));
    assert!(text(&added).contains("pinned"), "{}", text(&added));
    let kept = home
        .path()
        .join(".swarmux/holdout/shop/bigger/files/tests/hidden.rs");
    assert!(kept.is_file(), "kept outside every repository");

    let listed = swarmux(home.path(), work.path(), &["holdout", "list"]);
    let listed = text(&listed);
    assert!(listed.contains("shop"), "{listed}");
    assert!(listed.contains("cargo test --test hidden"), "{listed}");
    assert!(listed.contains("timeout 120 s"), "{listed}");

    let bad = swarmux(
        home.path(),
        work.path(),
        &[
            "holdout",
            "add",
            "Shop",
            "x",
            "--file",
            "../out.rs=hidden.rs",
            "--",
            "true",
        ],
    );
    assert!(!bad.status.success(), "a file outside the repository");

    let removed = swarmux(
        home.path(),
        work.path(),
        &["holdout", "rm", "Shop", "bigger"],
    );
    assert!(removed.status.success(), "{}", text(&removed));
    let again = swarmux(
        home.path(),
        work.path(),
        &["holdout", "rm", "Shop", "bigger"],
    );
    assert!(!again.status.success());
    let empty = swarmux(home.path(), work.path(), &["holdout", "list"]);
    assert!(
        text(&empty).contains("no hidden checks"),
        "{}",
        text(&empty)
    );
}
