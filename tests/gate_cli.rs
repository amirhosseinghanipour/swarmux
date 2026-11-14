//! `swarmux gate` as a user runs it: the real binary, a git repository and
//! a home folder of its own.

use std::fs;
use std::path::{Path, PathBuf};
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

/// The owner trusts the repository: its checks may run.
fn grant(home: &Path, root: &Path) {
    let out = swarmux(home, root, &["trust", "grant", root.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out));
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

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

/// A small repository with a test and a gate whose one check is `git
/// --version` (it passes anywhere git is).
fn repo(parent: &Path, gates: &str) -> Option<PathBuf> {
    let root = parent.join("shop");
    fs::create_dir_all(root.join("tests")).ok()?;
    fs::create_dir_all(root.join(".swarmux")).ok()?;
    fs::write(root.join(".swarmux/gates.toml"), gates).ok()?;
    fs::write(root.join("app.py"), "def add(a, b):\n    return a - b\n").ok()?;
    fs::write(
        root.join("tests/test_app.py"),
        "from app import add\n\n\ndef test_add():\n    assert add(2, 2) == 4\n",
    )
    .ok()?;
    if !git(&root, &["init", "-q", "-b", "main"]) {
        eprintln!("git not available; skipping");
        return None;
    }
    assert!(git(&root, &["add", "."]));
    assert!(git(&root, &["commit", "-q", "-m", "shop"]));
    Some(root)
}

const GATES: &str = "[[check]]\nname = \"git\"\nrun = [\"git\", \"--version\"]\n";

#[test]
fn a_malformed_gates_toml_is_linted_and_the_gate_cannot_pass_on_it() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let Some(root) = repo(work.path(), "[gate\nmax_blocks = 3\n") else {
        return;
    };
    let lint = swarmux(home.path(), &root, &["gate", "lint"]);
    assert!(!lint.status.success(), "{}", text(&lint));
    let said = text(&lint);
    assert!(said.contains("✗ .swarmux/gates.toml"), "{said}");
    assert!(said.contains("every verdict is escalate"), "{said}");

    let shown = text(&swarmux(home.path(), &root, &["gate", "show"]));
    assert!(shown.contains("problem"), "{shown}");
    assert!(shown.contains("its verdict is escalate"), "{shown}");

    let run = swarmux(home.path(), &root, &["gate", "run"]);
    assert!(!run.status.success(), "{}", text(&run));
    let said = text(&run);
    assert!(said.contains("! gates.toml could not be used"), "{said}");
    assert!(said.contains("gate cannot pass this: escalate"), "{said}");
}

#[test]
fn values_that_cannot_be_used_are_warnings_and_strict_lint_fails_on_them() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let gates = format!(
        "{GATES}\n[verifier]\nworktree_checks = \"whenever\"\nenv = [\"ANTHROPIC_API_KEY\"]\n"
    );
    let Some(root) = repo(work.path(), &gates) else {
        return;
    };
    let lint = swarmux(home.path(), &root, &["gate", "lint"]);
    assert!(
        lint.status.success(),
        "a warning is not a failure: {}",
        text(&lint)
    );
    let said = text(&lint);
    assert!(
        said.contains("! verifier worktree_checks is one of escalate, allow"),
        "{said}"
    );
    assert!(
        said.contains("! verifier env: ANTHROPIC_API_KEY is never passed to a check"),
        "{said}"
    );
    let strict = swarmux(home.path(), &root, &["gate", "lint", "--strict"]);
    assert!(!strict.status.success(), "{}", text(&strict));
    // The fallbacks are the strict values.
    let shown = text(&swarmux(home.path(), &root, &["gate", "show"]));
    assert!(shown.contains("a mission escalates"), "{shown}");
}

#[test]
fn a_run_in_a_clean_copy_passes_and_says_where_the_checks_ran() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let Some(root) = repo(work.path(), GATES) else {
        return;
    };
    // A repository nobody trusted runs none of its checks.
    let before = swarmux(home.path(), &root, &["gate", "run"]);
    let said = text(&before);
    assert!(!before.status.success(), "{said}");
    assert!(said.contains("not trusted yet"), "{said}");
    assert!(!said.contains("gate passed"), "{said}");
    grant(home.path(), &root);
    let run = swarmux(home.path(), &root, &["gate", "run"]);
    let said = text(&run);
    assert!(run.status.success(), "{said}");
    assert!(said.contains("in a clean copy of"), "{said}");
    assert!(said.contains("gate passed"), "{said}");
    // Never "the checks ran in the folder itself" without saying so.
    let in_place = swarmux(home.path(), &root, &["gate", "run", "--in-place"]);
    assert!(in_place.status.success(), "{}", text(&in_place));
    // A level above the checks' own runs more, a level below runs fewer.
    let deep = swarmux(home.path(), &root, &["gate", "run", "--level", "deep"]);
    assert!(deep.status.success(), "{}", text(&deep));
    let bad = swarmux(home.path(), &root, &["gate", "run", "--level", "slow"]);
    assert!(!bad.status.success());
    assert!(
        text(&bad).contains("--level is fast, full or deep"),
        "{}",
        text(&bad)
    );
}

#[test]
fn the_selftest_refuses_known_bad_changes_and_fails_a_gate_that_allows_them() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let Some(root) = repo(work.path(), GATES) else {
        return;
    };
    grant(home.path(), &root);
    let ok = swarmux(home.path(), &root, &["gate", "selftest"]);
    let said = text(&ok);
    assert!(ok.status.success(), "{said}");
    assert!(
        said.contains("✓ refused  planted conftest.py that forces every test to pass"),
        "{said}"
    );
    assert!(
        said.contains("✓ refused  deleted an assertion from tests/test_app.py"),
        "{said}"
    );
    assert!(said.contains("gate selftest passed"), "{said}");
    assert!(
        !root.join("conftest.py").exists(),
        "the repository is not touched"
    );

    let lax = format!(
        "{GATES}\n[verifier]\nallow_substrate = [\"conftest.py\"]\ntest_edits = \"allow\"\n"
    );
    fs::write(root.join(".swarmux/gates.toml"), lax).unwrap();
    assert!(git(&root, &["add", "."]));
    assert!(git(&root, &["commit", "-q", "-m", "lax"]));
    // The file changed, so the owner is asked again: this time they agree.
    grant(home.path(), &root);
    let bad = swarmux(home.path(), &root, &["gate", "selftest"]);
    let said = text(&bad);
    assert!(!bad.status.success(), "{said}");
    assert!(said.contains("✗ ALLOWED"), "{said}");
    assert!(said.contains("the gate found nothing wrong"), "{said}");
    assert!(said.contains("gate selftest FAILED"), "{said}");

    // Both runs are in the ledger.
    let tail = text(&swarmux(home.path(), &root, &["audit", "tail", "-n", "10"]));
    assert!(tail.matches("policy").count() >= 2, "{tail}");
}
