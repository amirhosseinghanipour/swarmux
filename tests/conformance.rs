//! `swarmux doctor --conformance` end to end: the real `swarmux` binary,
//! whose own `hook canary` the canaries run, and the fake `claude`
//! behaving toward that hook as Claude Code 2.1.323 was seen to (see
//! `guide/claude-code.md`). What this shows is that the command runs the
//! set, prints a result per canary and fails when a required one fails;
//! what the real Claude Code does is shown by running the command on a
//! machine that has it.
//!
//! Run with `cargo test -p swarmux --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn fake() -> Option<PathBuf> {
    let fake =
        Path::new(EXE).with_file_name(format!("fake_claude{}", std::env::consts::EXE_SUFFIX));
    if !fake.is_file() {
        eprintln!("{} is not built; skipping", fake.display());
        return None;
    }
    Some(fake)
}

fn doctor(fake: &Path, host: &str, extra_args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".swarmux")).unwrap();
    Command::new(EXE)
        .args(["doctor", "--conformance"])
        .args(extra_args)
        .current_dir(home.path())
        .env("USERPROFILE", home.path())
        .env("HOME", home.path())
        .env("SWARMUX_CLAUDE_BIN", fake)
        .env("FAKE_CLAUDE_HOOK_HOST", host)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn conformance_reports_every_canary_and_passes_when_the_hook_is_honoured() {
    let Some(fake) = fake() else { return };
    let out = doctor(&fake, "honor", &[]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    for line in [
        "ok   version (pass)",
        "ok   host_policy (pass)",
        "ok   deny (pass)",
        "ok   hook_exit_1 (pass)",
        "ok   hook_timeout (pass)",
        "note hook_invalid_json (note)",
        "ok   subagent (pass)",
        "ok   unknown_tool (pass)",
        "ok   guard_subagent (pass)",
        "ok   conformance: every required canary passed",
    ] {
        assert!(all.contains(line), "missing {line:?} in\n{all}");
    }
}

#[test]
fn conformance_fails_when_the_hook_is_not_honoured() {
    let Some(fake) = fake() else { return };
    let out = doctor(&fake, "ignore", &[]);
    let all = text(&out);
    assert!(!out.status.success(), "{all}");
    assert!(all.contains("fail deny (fail)"), "{all}");
    assert!(all.contains("conformance failed"), "{all}");
}

#[test]
fn doctor_without_conformance_runs_no_canary() {
    let Some(fake) = fake() else { return };
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(EXE)
        .arg("doctor")
        .current_dir(home.path())
        .env("USERPROFILE", home.path())
        .env("HOME", home.path())
        .env("SWARMUX_CLAUDE_BIN", &fake)
        .env("FAKE_CLAUDE_HOOK_HOST", "ignore")
        .output()
        .unwrap();
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(
        all.contains("in the tested range 2.1.323..2.1.323"),
        "{all}"
    );
    assert!(all.contains("host policy:"), "{all}");
    assert!(all.contains("canary = each"), "{all}");
    assert!(!all.contains("conformance"), "{all}");
}

#[test]
fn a_profile_is_asked_for_by_name() {
    let Some(fake) = fake() else { return };
    let out = doctor(&fake, "honor", &["--profile", "inherit"]);
    let all = text(&out);
    assert!(out.status.success(), "{all}");
    assert!(all.contains("conformance, profile inherit"), "{all}");
    let bad = doctor(&fake, "honor", &["--profile", "paranoid"]);
    assert!(!bad.status.success());
}
