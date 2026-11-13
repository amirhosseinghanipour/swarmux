//! `swarmux guard rules list | remove | prune` and `swarmux guard lint` as the
//! real binary runs them, on a home folder of their own: every change is a
//! policy entry in the ledger, and the chain still verifies.

use std::path::Path;
use std::process::{Command, Output};

use swarmux_application::ports::RuleStore;
use swarmux_domain::{RuleScope, RuleSource, StoredRule};
use swarmux_infrastructure::{TomlRuleStore, rules_file};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn swarmux(home: &Path, args: &[&str]) -> Output {
    Command::new(EXE)
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("SWARMUX_HOOK_PIPE")
        .output()
        .expect("runs swarmux")
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn seed(home: &Path) -> (StoredRule, StoredRule) {
    let dir = home.join(".swarmux");
    std::fs::create_dir_all(&dir).unwrap();
    let store = TomlRuleStore::new(rules_file(&dir));
    let now = now_ms();
    let live = StoredRule::new(
        "Bash(make all)",
        RuleScope::Repo,
        Some("/work/app"),
        RuleSource::AlwaysAllow,
        now - 1_000,
        Some(now + 20 * 86_400_000),
    );
    let old = StoredRule::new(
        "Bash(cargo fmt)",
        RuleScope::Global,
        None,
        RuleSource::Suggestion,
        now - 90 * 86_400_000,
        Some(now - 60 * 86_400_000),
    );
    store.add(&live).unwrap();
    store.add(&old).unwrap();
    (live, old)
}

#[test]
fn rules_list_remove_and_prune_are_ledgered() {
    let home = tempfile::tempdir().unwrap();
    let (live, old) = seed(home.path());
    // A rule written in config.toml before this version, with no scope or
    // expiry of its own, keeps working and is listed as such.
    std::fs::write(
        home.path().join(".swarmux").join("config.toml"),
        "[guard]
allow = [\"Bash(npm test)\"]
",
    )
    .unwrap();

    let list = swarmux(home.path(), &["guard", "rules", "list"]);
    assert!(list.status.success());
    let out = text(&list);
    println!("{out}");
    assert!(
        out.contains(&live.id) && out.contains("Bash(make all)"),
        "{out}"
    );
    assert!(out.contains("repo /work/app"), "{out}");
    assert!(out.contains("source always_allow"), "{out}");
    assert!(out.contains("EXPIRED"), "{out}");
    assert!(
        out.contains("Bash(npm test)") && out.contains("global, never expires (source config"),
        "{out}"
    );

    let removed = swarmux(home.path(), &["guard", "rules", "remove", &live.id]);
    println!("{}", text(&removed));
    assert!(removed.status.success());
    let again = swarmux(home.path(), &["guard", "rules", "remove", &live.id]);
    assert!(!again.status.success(), "no such rule any more");

    let pruned = swarmux(home.path(), &["guard", "rules", "prune"]);
    let out = text(&pruned);
    println!("{out}");
    assert!(pruned.status.success());
    assert!(out.contains(&old.id), "{out}");
    let list = text(&swarmux(home.path(), &["guard", "rules", "list"]));
    assert!(
        list.contains("Bash(npm test)")
            && !list.contains("Bash(make all)")
            && !list.contains("Bash(cargo fmt)"),
        "only the rule written in config.toml is left: {list}"
    );

    let tail = text(&swarmux(home.path(), &["audit", "tail", "-n", "10"]));
    println!("{tail}");
    assert!(tail.contains("policy"), "{tail}");
    assert!(tail.contains("removed"), "{tail}");
    assert!(tail.contains("expired"), "{tail}");
    let verify = swarmux(home.path(), &["audit", "verify"]);
    assert!(verify.status.success(), "{}", text(&verify));
}

#[test]
fn lint_names_the_lines_replaced_by_defaults() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".swarmux");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "[guard]\non_unreachable = \"sometimes\"\nalways_expiry_days = 14\n",
    )
    .unwrap();
    let lint = swarmux(home.path(), &["guard", "lint"]);
    let out = text(&lint);
    println!("{out}");
    assert!(!lint.status.success(), "a replaced line is a failure");
    assert!(out.contains("warn config.toml (line 2)"), "{out}");
    assert!(out.contains("on_unreachable"), "{out}");
    assert!(out.contains("always_expiry_days = 14"), "{out}");
    assert!(
        out.contains("unattended jobs: closed, other jobs: open"),
        "{out}"
    );

    std::fs::write(
        dir.join("config.toml"),
        "[guard]\non_unreachable = \"deny\"\n",
    )
    .unwrap();
    let ok = swarmux(home.path(), &["guard", "lint"]);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(text(&ok).contains("unattended jobs: closed, other jobs: closed"));
}

#[test]
fn an_old_jev_section_loads_and_lint_says_it_is_ignored() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".swarmux");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.toml"),
        "max_concurrent = 2\n[jev]\nprovider = \"typesafe\"\nthreshold = 0.9\nmax_allow = \"R2\"\n[guard]\nunmatched = \"ask\"\n",
    )
    .unwrap();
    let lint = swarmux(home.path(), &["guard", "lint"]);
    let out = text(&lint);
    println!("{out}");
    assert!(
        out.contains("warn config.toml (line 2): JEV was removed; this section is ignored"),
        "{out}"
    );
    assert!(
        !lint.status.success(),
        "a section left over is named, as a replaced line is"
    );
    // The file still loaded: the rest of it is read, and no model is asked.
    assert!(
        out.contains("guard.unmatched = ask (a call no rule matches; no model is asked)"),
        "{out}"
    );
    assert!(!out.contains("jev.max_allow"), "{out}");

    // Without the section there is nothing to say.
    std::fs::write(dir.join("config.toml"), "[guard]\nunmatched = \"ask\"\n").unwrap();
    let ok = swarmux(home.path(), &["guard", "lint"]);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(!text(&ok).contains("JEV"), "{}", text(&ok));
}

#[test]
fn the_jev_command_is_gone() {
    let home = tempfile::tempdir().unwrap();
    for args in [["jev", "test"], ["jev", "set-key"]] {
        let out = swarmux(home.path(), &args);
        assert!(!out.status.success(), "{args:?}");
    }
}
