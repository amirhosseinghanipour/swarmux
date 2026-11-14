//! `swarmux trust` as a user runs it: the real binary, a repository that
//! ships a gate and a home folder of its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn swarmux(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    Command::new(EXE)
        .args(args)
        .current_dir(cwd)
        .env(var, home)
        .env("LOCALAPPDATA", home.join("local"))
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("SWARMUX_STATE_DIR")
        .env_remove("SWARMUX_CONFIG_DIR")
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

/// A cloned repository whose gate names a command.
fn repo(parent: &Path, gates: &str) -> PathBuf {
    let root = parent.join("cloned");
    std::fs::create_dir_all(root.join(".swarmux")).unwrap();
    std::fs::write(root.join(".swarmux/gates.toml"), gates).unwrap();
    root
}

const SHIPS_A_CHECK: &str = "[[check]]\nname = \"unit\"\nrun = [\"git\", \"--version\"]\n";

#[test]
fn nobody_is_trusted_at_first() {
    let home = tempfile::tempdir().unwrap();
    let out = swarmux(home.path(), home.path(), &["trust", "list"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("no repository has been trusted"), "{said}");
}

#[test]
fn show_says_exactly_what_a_grant_would_cover() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = repo(work.path(), SHIPS_A_CHECK);
    let out = swarmux(home.path(), &root, &["trust", "show"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("not trusted yet"), "{said}");
    assert!(said.contains("run git --version"), "{said}");
    assert!(said.contains(".swarmux/gates.toml"), "{said}");
}

#[test]
fn grant_a_change_and_revoke() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = repo(work.path(), SHIPS_A_CHECK);
    let folder = root.to_str().unwrap();

    let granted = swarmux(home.path(), &root, &["trust", "grant", folder]);
    let said = text(&granted);
    assert!(granted.status.success(), "{said}");
    assert!(said.contains("run git --version"), "{said}");
    let again = text(&swarmux(home.path(), &root, &["trust", "grant", folder]));
    assert!(again.contains("already trusted"), "{again}");
    let listed = text(&swarmux(home.path(), &root, &["trust", "list"]));
    assert!(listed.contains("trusted"), "{listed}");
    assert!(!listed.contains("changed"), "{listed}");

    // The grant is in the ledger, which still verifies.
    let tail = text(&swarmux(home.path(), &root, &["audit", "tail", "-n", "5"]));
    assert!(tail.contains("policy"), "{tail}");
    assert!(
        swarmux(home.path(), &root, &["audit", "verify"])
            .status
            .success(),
        "the chain holds"
    );

    // A changed file asks again, and names the file.
    std::fs::write(
        root.join(".swarmux/gates.toml"),
        format!("{SHIPS_A_CHECK}\n[[check]]\nrun = [\"git\", \"status\"]\n"),
    )
    .unwrap();
    let shown = text(&swarmux(home.path(), &root, &["trust", "show"]));
    assert!(shown.contains("changed since it was trusted"), "{shown}");
    assert!(shown.contains(".swarmux/gates.toml"), "{shown}");
    let listed = text(&swarmux(home.path(), &root, &["trust", "list"]));
    assert!(listed.contains("changed since it was trusted"), "{listed}");

    // Revoking takes it back.
    let revoked = swarmux(home.path(), &root, &["trust", "revoke", folder]);
    assert!(revoked.status.success(), "{}", text(&revoked));
    let shown = text(&swarmux(home.path(), &root, &["trust", "show"]));
    assert!(shown.contains("revoked"), "{shown}");
    let nothing = swarmux(home.path(), &root, &["trust", "revoke", "not-a-repo"]);
    assert!(!nothing.status.success());
}

#[test]
fn a_gate_that_only_restricts_needs_no_trust() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = repo(
        work.path(),
        "[gate]\ndetect = false\nprotected = [\"oracles/**\"]\n",
    );
    let said = text(&swarmux(home.path(), &root, &["trust", "show"]));
    assert!(said.contains("nothing here needs it"), "{said}");
    let doctor = text(&swarmux(home.path(), &root, &["doctor"]));
    assert!(!doctor.contains("is not trusted yet"), "{doctor}");
}

#[test]
fn doctor_names_a_repository_waiting_for_trust_and_what_it_wants() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = repo(work.path(), SHIPS_A_CHECK);
    let doctor = text(&swarmux(home.path(), &root, &["doctor"]));
    assert!(doctor.contains("not trusted yet"), "{doctor}");
    assert!(doctor.contains("run git --version"), "{doctor}");
    assert!(doctor.contains("swarmux trust grant"), "{doctor}");
}
