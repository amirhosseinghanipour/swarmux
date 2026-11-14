//! `swarmux config` and the folders swarmux uses, as a user runs them: the
//! real binary and a home folder of its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn swarmux_with(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut cmd = Command::new(EXE);
    cmd.args(args)
        .current_dir(home)
        .env(var, home)
        .env("LOCALAPPDATA", home.join("local"))
        .env("APPDATA", home.join("roaming"));
    for k in [
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "SWARMUX_CONFIG_DIR",
        "SWARMUX_STATE_DIR",
        "SWARMUX_MANAGED_FILE",
        "SWARMUX_WORKSPACE",
    ] {
        cmd.env_remove(k);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn swarmux(home: &Path, args: &[&str]) -> Output {
    swarmux_with(home, args, &[])
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn legacy_home(config: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    write(&home.path().join(".swarmux/config.toml"), config);
    home
}

#[cfg(unix)]
#[test]
fn a_fresh_linux_home_keeps_config_and_state_apart() {
    let home = tempfile::tempdir().unwrap();
    let out = swarmux(home.path(), &["config", "paths"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    let config = home.path().join(".config/swarmux");
    let state = home.path().join(".local/state/swarmux");
    assert!(said.contains(&config.display().to_string()), "{said}");
    assert!(said.contains(&state.display().to_string()), "{said}");
    assert!(said.contains("XDG default"), "{said}");

    // Using it puts the data in the state folder and nothing in ~/.swarmux.
    let stats = swarmux(home.path(), &["audit", "tail", "-n", "1"]);
    assert!(stats.status.success(), "{}", text(&stats));
    assert!(state.join("swarmux.db").is_file());
    assert!(!home.path().join(".swarmux").exists());
}

#[test]
fn an_existing_dot_swarmux_keeps_working_unchanged() {
    let home = legacy_home("max_concurrent = 7\n[guard]\nunmatched = \"pass\"\n");
    let said = text(&swarmux(home.path(), &["config", "paths"]));
    assert!(said.contains("~/.swarmux exists"), "{said}");
    let old = home.path().join(".swarmux");
    assert!(said.contains(&old.display().to_string()), "{said}");

    let got = swarmux(home.path(), &["config", "get", "max_concurrent"]);
    assert_eq!(text(&got).trim(), "7");
    let listed = text(&swarmux(home.path(), &["config", "list", "--changed"]));
    assert!(listed.contains("guard.unmatched"), "{listed}");
    // The database is made where it always was.
    assert!(
        swarmux(home.path(), &["audit", "tail", "-n", "1"])
            .status
            .success()
    );
    assert!(old.join("swarmux.db").is_file());
}

#[test]
fn explain_shows_the_tier_chain_of_guard_unmatched() {
    let home = legacy_home("[guard]\nunmatched = \"pass\"\n");
    let out = swarmux_with(
        home.path(),
        &["config", "explain", "guard.unmatched"],
        &[("SWARMUX_GUARD__UNMATCHED", "ask")],
    );
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    println!("{said}");
    assert!(said.contains("guard.unmatched = \"ask\""), "{said}");
    assert!(
        said.contains("lock: the strictest value of any tier wins"),
        "{said}"
    );
    assert!(said.contains("core"), "{said}");
    assert!(said.contains("user"), "{said}");
    assert!(
        said.contains("config.toml:2"),
        "the line it was written on: {said}"
    );
    assert!(said.contains("run"), "{said}");
    assert!(said.contains("environment"), "{said}");
    assert!(said.contains("locked    no"), "{said}");
}

#[test]
fn a_managed_lock_overrides_what_the_user_set() {
    let home = legacy_home("[guard]\nunmatched = \"pass\"\n");
    let managed: PathBuf = home.path().join("managed.toml");
    write(
        &managed,
        "locked = [\"guard.unmatched\"]\n[guard]\nunmatched = \"ask\"\n",
    );
    let envs = [("SWARMUX_MANAGED_FILE", managed.to_str().unwrap())];
    let got = swarmux_with(home.path(), &["config", "get", "guard.unmatched"], &envs);
    assert_eq!(text(&got).trim(), "\"ask\"");
    let said = text(&swarmux_with(
        home.path(),
        &["config", "explain", "guard.unmatched"],
        &envs,
    ));
    assert!(said.contains("locked    yes, by"), "{said}");
    // Not even a flag typed for the run beats it.
    let flagged = swarmux_with(
        home.path(),
        &[
            "--set",
            "guard.unmatched=pass",
            "config",
            "get",
            "guard.unmatched",
        ],
        &envs,
    );
    assert_eq!(text(&flagged).trim(), "\"ask\"");
    // Without the managed file the user's own value stands.
    let free = swarmux(home.path(), &["config", "get", "guard.unmatched"]);
    assert_eq!(text(&free).trim(), "\"pass\"");
}

#[test]
fn an_invalid_key_warns_and_nothing_stops() {
    let home =
        legacy_home("max_concurrent = 4\nnot_a_setting = 1\n[guard]\napproval_timeout_secs = 0\n");
    // The commands that read the config still run.
    let doctor = swarmux(home.path(), &["doctor"]);
    let said = text(&doctor);
    assert!(said.contains("not_a_setting"), "{said}");
    assert!(said.contains("approval_timeout_secs"), "{said}");
    assert!(
        said.contains("max_concurrent  = 4"),
        "the rest is read: {said}"
    );

    let lint = swarmux(home.path(), &["config", "lint"]);
    let said = text(&lint);
    assert!(
        !lint.status.success(),
        "an unusable value is an error: {said}"
    );
    assert!(said.contains("line 2") || said.contains("line 4"), "{said}");

    // An unknown key alone is a warning, and fails only under --strict.
    write(
        &home.path().join(".swarmux/config.toml"),
        "future_key = 1\n",
    );
    assert!(swarmux(home.path(), &["config", "lint"]).status.success());
    assert!(
        !swarmux(home.path(), &["config", "lint", "--strict"])
            .status
            .success()
    );
}

#[test]
fn a_config_that_is_not_toml_still_lets_commands_run() {
    let home =
        legacy_home("max_concurrent = 4\nthis is = = nonsense\n[guard\nunmatched = \"pass\"\n");
    for args in [
        &["doctor"][..],
        &["config", "list"],
        &["stats", "verification"],
    ] {
        let out = swarmux(home.path(), args);
        assert!(out.status.success(), "{args:?}: {}", text(&out));
    }
    let lint = swarmux(home.path(), &["config", "lint"]);
    assert!(!lint.status.success());
    assert!(text(&lint).contains("line 2"), "{}", text(&lint));
}

#[test]
fn export_and_schema_are_commands() {
    let home = legacy_home("max_concurrent = 4\n");
    let export = text(&swarmux(
        home.path(),
        &["config", "export", "--tier", "user"],
    ));
    assert!(export.starts_with("schema_version = 1"), "{export}");
    assert!(export.contains("max_concurrent = 4"), "{export}");
    let out = home.path().join("schemas");
    let made = swarmux(
        home.path(),
        &["config", "schema", "--out", out.to_str().unwrap()],
    );
    assert!(made.status.success(), "{}", text(&made));
    for file in [
        "config.schema.json",
        "managed.schema.json",
        "gates.schema.json",
        "mission.schema.json",
        "knowledge.schema.json",
        "keys.schema.json",
        "theme.schema.json",
    ] {
        assert!(out.join(file).is_file(), "{file}");
    }
    let one = text(&swarmux(
        home.path(),
        &["config", "schema", "--name", "gates"],
    ));
    assert!(one.contains("\"$schema\""), "{one}");
}

#[test]
fn an_unknown_key_for_get_says_what_is_known() {
    let home = tempfile::tempdir().unwrap();
    let out = swarmux(home.path(), &["config", "get", "guard.unmatchd"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("not a setting swarmux knows"),
        "{}",
        text(&out)
    );
}
