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
    std::fs::write(home.path().join(".swarmux").join("config.toml"), "").ok();
    let (live, old) = seed(home.path());

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
    assert!(list.contains("no allow rules"), "{list}");

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
