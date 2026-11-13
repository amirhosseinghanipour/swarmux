//! `swarmux keys list`, `lint` and `export` over the real catalog.

use super::*;
use crate::keys::{config_of, effective};
use swarmux_domain::keys::{KeyConfig, KeyEntry, Keymap};

fn found(entries: &[(&str, &str, &[&str])]) -> Effective {
    effective(&config_of(entries)).unwrap()
}

fn line_of<'a>(text: &'a str, context: &str, action: &str) -> &'a str {
    text.lines()
        .find(|l| {
            let mut words = l.split_whitespace();
            words.next() == Some(context) && words.next() == Some(action)
        })
        .unwrap_or_else(|| panic!("no line for {context} {action} in:\n{text}"))
}

// ---- list

#[test]
fn list_has_every_action_with_its_keys_and_its_default() {
    let text = list(&found(&[]));
    assert!(text.starts_with("CONTEXT"), "{text}");
    let line = line_of(&text, "mission.wall", "list.prev");
    assert!(line.contains("up, left, k, h"), "{line}");
    assert!(!line.contains("changed"), "{line}");
    assert!(line_of(&text, "global", "nav.go_work").contains("g w"));
    assert!(line_of(&text, "queue", "queue.priority_up").contains("+, ="));
    // One line per action, and the header.
    let (_, catalog) = crate::keys::defaults().unwrap();
    assert_eq!(text.lines().count(), catalog.defs().len() + 1);
}

#[test]
fn list_marks_what_the_user_changed_and_what_is_fixed() {
    let text = list(&found(&[
        ("queue", "queue.cancel", &["x"]),
        ("queue", "queue.retry", &[]),
    ]));
    let cancel = line_of(&text, "queue", "queue.cancel");
    assert!(cancel.contains(" x "), "{cancel}");
    assert!(cancel.trim_end().ends_with("changed"), "{cancel}");
    let retry = line_of(&text, "queue", "queue.retry");
    assert!(
        retry.contains("(none)") && retry.trim_end().ends_with("changed"),
        "{retry}"
    );
    assert!(!line_of(&text, "queue", "queue.priority_up").contains("changed"));
    assert!(
        line_of(&text, "decide", "decide.yes")
            .trim_end()
            .ends_with("fixed")
    );
    assert!(line_of(&text, "global", "back").contains("esc"));
    assert!(line_of(&text, "global", "quit").contains("q, ctrl+c"));
}

// ---- lint

#[test]
fn lint_of_a_good_config_is_quiet_and_succeeds() {
    let (report, failed) = lint(
        &found(&[("queue", "queue.cancel", &["x"])]),
        "config.toml",
        true,
    );
    assert!(!failed);
    assert_eq!(report, "keys: config.toml is fine\n");
}

#[test]
fn lint_without_a_keys_section_says_so() {
    let (report, failed) = lint(&found(&[]), "config.toml", false);
    assert!(!failed);
    assert!(report.contains("no [keys] in config.toml"), "{report}");
}

#[test]
fn lint_prints_every_problem_and_fails() {
    let (report, failed) = lint(
        &found(&[
            ("queue", "queue.cancel", &["esc"]),
            ("queue", "queue.nope", &["x"]),
            ("nowhere", "queue.retry", &["x"]),
            ("global", "nav.go_ledger", &["ctrl+"]),
            ("queue", "queue.retry", &["+"]),
            ("decide", "decide.yes", &["w"]),
        ]),
        "config.toml",
        true,
    );
    assert!(failed);
    assert_eq!(report.matches("error:").count(), 6, "{report}");
    for want in [
        "[keys.queue] \"queue.cancel\" = \"esc\"",
        "no action called `queue.nope`",
        "no context called `nowhere`",
        "[keys.global] \"nav.go_ledger\" = \"ctrl+\"",
        "already queue.priority_up",
        "[keys.decide] \"decide.yes\"",
        "keys: 6 errors, 0 warnings in config.toml",
        "can't be rebound, on purpose",
    ] {
        assert!(report.contains(want), "missing {want:?} in:\n{report}");
    }
}

#[test]
fn lint_warns_about_overlaps_without_failing() {
    let (report, failed) = lint(
        &found(&[("queue", "queue.cancel", &["t"])]),
        "config.toml",
        true,
    );
    assert!(!failed, "{report}");
    assert!(report.contains("warning:"), "{report}");
    assert!(report.contains("look.next_theme"), "{report}");
    assert!(report.contains("1 warning"), "{report}");
}

// ---- export

#[test]
fn export_is_the_effective_keymap_as_toml_with_the_defaults_included() {
    let text = export(&found(&[("queue", "queue.cancel", &["x"])]));
    assert!(text.contains("[keys.queue]"), "{text}");
    assert!(text.contains("\"queue.cancel\" = \"x\""), "{text}");
    assert!(
        text.contains("\"queue.retry\" = \"r\""),
        "defaults too: {text}"
    );
    assert!(text.contains("[keys.\"mission.wall\"]"), "{text}");
    assert!(
        text.contains("\"list.prev\" = [\"up\", \"left\", \"k\", \"h\"]"),
        "{text}"
    );
    assert!(text.contains("[keys.global]"), "{text}");
    assert!(text.contains("\"nav.go_work\" = \"g w\""), "{text}");
    // What can't be changed is not in it.
    assert!(!text.contains("decide.yes"), "{text}");
    assert!(!text.contains("[keys.decide]"), "{text}");
    assert!(!text.contains("\"back\""), "{text}");
}

#[test]
fn export_writes_an_unbound_action_as_an_empty_list() {
    let text = export(&found(&[("queue", "queue.retry", &[])]));
    assert!(text.contains("\"queue.retry\" = []"), "{text}");
}

#[test]
fn export_escapes_quotes_and_backslashes() {
    assert_eq!(quoted("\""), "\"\\\"\"");
    assert_eq!(quoted("\\"), "\"\\\\\"");
    assert_eq!(quoted("g w"), "\"g w\"");
}

#[test]
fn exported_keys_load_back_as_the_same_keymap_with_nothing_to_report() {
    // The loop `swarmux keys export > config` is the whole point of it.
    let user = found(&[
        ("queue", "queue.cancel", &["x"]),
        ("queue", "queue.retry", &[]),
        ("global", "nav.go_ledger", &["f5", "alt+5"]),
        ("mission.wall", "list.next", &["g g", "ctrl+n"]),
        ("global", "nav.go_work", &["w"]),
    ]);
    assert!(user.problems.is_empty(), "{:?}", user.problems);
    let toml: toml::Value = toml::from_str(&export(&user)).unwrap();
    let mut entries = Vec::new();
    collect(toml.get("keys").unwrap(), "", &mut entries);
    let again = effective(&KeyConfig {
        entries,
        malformed: Vec::new(),
    })
    .unwrap();
    assert!(again.problems.is_empty(), "{:?}", again.problems);
    for context in again.catalog.contexts() {
        let sorted = |m: &Keymap| {
            let mut all: Vec<_> = m
                .bindings(context)
                .iter()
                .map(|b| (b.chord.to_string(), b.action))
                .collect();
            all.sort();
            all
        };
        assert_eq!(sorted(&again.keymap), sorted(&user.keymap), "{context}");
    }
}

/// What the config loader does with the tables of `[keys]`.
fn collect(value: &toml::Value, section: &str, out: &mut Vec<KeyEntry>) {
    let toml::Value::Table(table) = value else {
        return;
    };
    for (name, value) in table {
        match value {
            toml::Value::Table(_) => {
                let inner = if section.is_empty() {
                    name.clone()
                } else {
                    format!("{section}.{name}")
                };
                collect(value, &inner, out);
            }
            toml::Value::String(key) => out.push(KeyEntry {
                section: section.to_string(),
                name: name.clone(),
                keys: vec![key.clone()],
            }),
            toml::Value::Array(items) => out.push(KeyEntry {
                section: section.to_string(),
                name: name.clone(),
                keys: items
                    .iter()
                    .map(|i| i.as_str().unwrap().to_string())
                    .collect(),
            }),
            other => panic!("{other:?}"),
        }
    }
}
