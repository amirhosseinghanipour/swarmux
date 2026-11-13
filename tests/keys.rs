//! `swarmux keys list`, `lint` and `export` as a user runs them: the real
//! binary, a home folder of its own and a config file.

use std::path::Path;
use std::process::{Command, Output};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

/// A home folder whose config holds `config` (none when it is empty).
fn home_with(config: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    if !config.is_empty() {
        let dir = home.path().join(".swarmux");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), config).unwrap();
    }
    home
}

fn swarmux(home: &Path, args: &[&str], no_color: bool) -> Output {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut cmd = Command::new(EXE);
    cmd.args(args)
        .env(var, home)
        .env("SWARMUX_CLAUDE_BIN", home.join("no-such-claude"))
        .env_remove("NO_COLOR");
    if no_color {
        cmd.env("NO_COLOR", "1");
    }
    cmd.output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn list_shows_every_action_and_marks_what_the_config_changed() {
    let home = home_with("[keys.queue]\n\"queue.cancel\" = \"x\"\n");
    let out = swarmux(home.path(), &["keys", "list"], false);
    assert!(out.status.success(), "{}", text(&out));
    let list = text(&out);
    assert!(list.starts_with("CONTEXT"), "{list}");
    let cancel = list
        .lines()
        .find(|l| l.contains("queue.cancel"))
        .expect("a line for queue.cancel");
    assert!(cancel.trim_end().ends_with("changed"), "{cancel}");
    assert!(list.contains("nav.go_work"), "{list}");
    assert!(
        list.lines()
            .find(|l| l.contains("decide.yes"))
            .unwrap()
            .trim_end()
            .ends_with("fixed")
    );
}

#[test]
fn list_without_a_config_has_nothing_marked_changed() {
    let home = home_with("");
    let out = swarmux(home.path(), &["keys", "list"], false);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("changed"));
}

#[test]
fn lint_of_a_good_or_missing_config_succeeds() {
    let good = home_with("[keys.queue]\n\"queue.cancel\" = \"x\"\n");
    let out = swarmux(good.path(), &["keys", "lint"], false);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("is fine"), "{}", text(&out));

    let none = home_with("");
    let out = swarmux(none.path(), &["keys", "lint"], false);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("no [keys]"), "{}", text(&out));
}

#[test]
fn lint_of_a_broken_config_prints_every_problem_and_fails() {
    let home = home_with(
        "[keys.queue]\n\
         \"queue.cancel\" = \"esc\"\n\
         \"queue.nope\" = \"x\"\n\
         \"queue.retry\" = \"+\"\n\
         [keys.nowhere]\n\
         quit = \"x\"\n\
         [keys.decide]\n\
         \"decide.yes\" = \"w\"\n\
         [keys.global]\n\
         \"nav.go_ledger\" = 5\n",
    );
    let out = swarmux(home.path(), &["keys", "lint"], false);
    assert!(!out.status.success(), "{}", text(&out));
    let report = text(&out);
    for want in [
        "[keys.queue] \"queue.cancel\" = \"esc\"",
        "no action called `queue.nope`",
        "already queue.priority_up",
        "no context called `nowhere`",
        "[keys.decide] \"decide.yes\"",
        "global.nav.go_ledger: expected a key",
    ] {
        assert!(report.contains(want), "missing {want:?} in:\n{report}");
    }
}

#[test]
fn a_config_that_is_not_toml_fails_lint_with_its_line() {
    let home = home_with("[keys.queue]\nqueue = = x\n");
    let out = swarmux(home.path(), &["keys", "lint"], false);
    assert!(!out.status.success());
    assert!(text(&out).contains("line 2"), "{}", text(&out));
}

#[test]
fn a_broken_keys_section_never_stops_the_other_commands() {
    let home = home_with("[keys.queue]\n\"queue.nope\" = \"x\"\n[keys.global]\nquit = 5\n");
    for args in [
        &["theme", "list"][..],
        &["keys", "list"],
        &["keys", "export"],
    ] {
        let out = swarmux(home.path(), args, false);
        assert!(out.status.success(), "{args:?}: {}", text(&out));
    }
}

#[test]
fn nothing_is_coloured_so_no_color_changes_nothing() {
    let home = home_with("[keys.queue]\n\"queue.nope\" = \"x\"\n");
    for args in [
        &["keys", "list"][..],
        &["keys", "lint"],
        &["keys", "export"],
    ] {
        let plain = swarmux(home.path(), args, true);
        let other = swarmux(home.path(), args, false);
        assert_eq!(text(&plain), text(&other), "{args:?}");
        assert!(!text(&plain).contains('\u{1b}'), "{args:?}");
    }
}

#[test]
fn export_is_a_config_that_lints_clean_and_exports_the_same_again() {
    let home = home_with("[keys.queue]\n\"queue.cancel\" = \"x\"\n\"queue.retry\" = []\n");
    let out = swarmux(home.path(), &["keys", "export"], false);
    assert!(out.status.success(), "{}", text(&out));
    let exported = String::from_utf8(out.stdout).unwrap();
    assert!(exported.contains("\"queue.cancel\" = \"x\""), "{exported}");
    assert!(exported.contains("\"queue.retry\" = []"), "{exported}");

    // Using the export as the config changes nothing.
    let again = home_with(&exported);
    let lint = swarmux(again.path(), &["keys", "lint"], false);
    assert!(lint.status.success(), "{}", text(&lint));
    assert!(text(&lint).contains("is fine"), "{}", text(&lint));
    let second = swarmux(again.path(), &["keys", "export"], false);
    assert_eq!(String::from_utf8(second.stdout).unwrap(), exported);
}

#[test]
fn the_toml_way_of_nesting_names_is_read_too() {
    let home = home_with("[keys.mission.wall]\nlist.next = \"z\"\n");
    let lint = swarmux(home.path(), &["keys", "lint"], false);
    assert!(lint.status.success(), "{}", text(&lint));
    let list = text(&swarmux(home.path(), &["keys", "list"], false));
    let line = list
        .lines()
        .find(|l| l.starts_with("mission.wall") && l.contains("list.next"))
        .unwrap();
    assert!(line.trim_end().ends_with("changed"), "{line}");
}
