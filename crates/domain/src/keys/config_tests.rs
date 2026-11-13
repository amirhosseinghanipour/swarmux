//! A user's `[keys]` against the defaults: every kind of mistake, and that
//! the defaults keep working around it.

use super::*;

const SCREEN: Context = Context::new("screen");
const WALL: Context = Context::new("screen.wall");
const OTHER: Context = Context::new("other");

const GO: Action = Action::new("t.go");
const STOP: Action = Action::new("t.stop");
const SKIP: Action = Action::new("t.skip");
const SHARED: Action = Action::new("list.next");
const FIXED: Action = Action::new("t.fixed");

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap()
}

fn defaults() -> (Keymap, Catalog) {
    let mut keymap = Keymap::new();
    let mut catalog = Catalog::new();
    for (context, action, keys) in [
        (SCREEN, GO, &["g"][..]),
        (SCREEN, STOP, &["s", "x"]),
        (SCREEN, SKIP, &["k"]),
        (SCREEN, FIXED, &["f"]),
        (WALL, SHARED, &["down", "j"]),
        (OTHER, SHARED, &["n"]),
        (GLOBAL, Action::new("quit"), &["q"]),
    ] {
        catalog.add(context, action, "does it");
        for k in keys {
            keymap.bind(context, chord(k), action).unwrap();
        }
    }
    catalog.lock(SCREEN, FIXED);
    (keymap, catalog)
}

fn entry(section: &str, name: &str, keys: &[&str]) -> KeyEntry {
    KeyEntry {
        section: section.into(),
        name: name.into(),
        keys: keys.iter().map(|k| k.to_string()).collect(),
    }
}

fn config(entries: Vec<KeyEntry>) -> KeyConfig {
    KeyConfig {
        entries,
        malformed: Vec::new(),
    }
}

fn run(entries: Vec<KeyEntry>) -> Applied {
    let (keymap, catalog) = defaults();
    apply(&keymap, &catalog, &config(entries))
}

fn keys_of(applied: &Applied, context: Context, action: Action) -> Vec<String> {
    applied
        .keymap
        .chords_of(context, action)
        .map(ToString::to_string)
        .collect()
}

fn texts(applied: &Applied) -> Vec<String> {
    applied.problems.iter().map(ToString::to_string).collect()
}

#[test]
fn no_config_is_the_defaults() {
    let (keymap, _) = defaults();
    let applied = run(Vec::new());
    assert_eq!(applied.keymap, keymap);
    assert!(applied.problems.is_empty());
}

#[test]
fn an_entry_replaces_the_default_keys_of_its_action_and_leaves_the_rest() {
    let (keymap, _) = defaults();
    let applied = run(vec![entry("screen", "t.go", &["w"])]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    assert_eq!(keys_of(&applied, SCREEN, GO), ["w"]);
    assert_eq!(keys_of(&applied, SCREEN, STOP), ["s", "x"], "kept");
    assert_eq!(keys_of(&applied, SCREEN, SKIP), ["k"]);
    assert_eq!(applied.keymap.bindings(OTHER), keymap.bindings(OTHER));
    assert_eq!(applied.keymap.bindings(GLOBAL), keymap.bindings(GLOBAL));
}

#[test]
fn a_list_binds_every_key_in_order_and_a_chord_is_written_with_a_space() {
    let applied = run(vec![entry(
        "screen",
        "t.go",
        &["g w", "ctrl+x", "alt+j", "f5", "shift+tab"],
    )]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    assert_eq!(
        keys_of(&applied, SCREEN, GO),
        ["g w", "ctrl+x", "alt+j", "f5", "shift+tab"]
    );
    // The same key twice is one key.
    let applied = run(vec![entry("screen", "t.go", &["w", "w"])]);
    assert_eq!(keys_of(&applied, SCREEN, GO), ["w"]);
}

#[test]
fn an_empty_list_unbinds_the_action() {
    let applied = run(vec![entry("screen", "t.stop", &[])]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    assert!(keys_of(&applied, SCREEN, STOP).is_empty());
    assert_eq!(keys_of(&applied, SCREEN, GO), ["g"]);
}

#[test]
fn keys_may_move_between_actions_in_one_go() {
    // A swap is no conflict: the end result has one action per key.
    let applied = run(vec![
        entry("screen", "t.go", &["s", "x"]),
        entry("screen", "t.stop", &["g"]),
    ]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    assert_eq!(keys_of(&applied, SCREEN, GO), ["s", "x"]);
    assert_eq!(keys_of(&applied, SCREEN, STOP), ["g"]);
}

#[test]
fn a_freed_default_key_can_be_taken_by_another_action() {
    let applied = run(vec![
        entry("screen", "t.stop", &[]),
        entry("screen", "t.go", &["s"]),
    ]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    assert_eq!(keys_of(&applied, SCREEN, GO), ["s"]);
}

#[test]
fn the_same_action_in_two_contexts_is_set_per_context() {
    let applied = run(vec![entry("other", "list.next", &["z"])]);
    assert_eq!(keys_of(&applied, OTHER, SHARED), ["z"]);
    assert_eq!(keys_of(&applied, WALL, SHARED), ["down", "j"]);
}

#[test]
fn dotted_names_work_nested_or_quoted() {
    // `[keys."screen.wall"] "list.next" = "z"`
    let quoted = run(vec![entry("screen.wall", "list.next", &["z"])]);
    // `[keys.screen.wall] list.next = "z"`: TOML nests all of it.
    let nested = run(vec![entry("screen.wall.list", "next", &["z"])]);
    for applied in [quoted, nested] {
        assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
        assert_eq!(keys_of(&applied, WALL, SHARED), ["z"]);
    }
}

#[test]
fn an_unknown_context_says_which_and_what_exists() {
    let applied = run(vec![entry("scren", "t.go", &["w"])]);
    let [problem] = &applied.problems[..] else {
        panic!("{:?}", texts(&applied));
    };
    assert!(matches!(problem, KeyProblem::UnknownContext { context, .. } if context == "scren"));
    let text = problem.to_string();
    assert!(text.contains("[keys.scren]"), "{text}");
    assert!(text.contains("t.go"), "{text}");
    assert!(text.contains("no context called `scren`"), "{text}");
    assert!(text.contains("screen.wall"), "lists the contexts: {text}");
    assert!(
        text.contains("`[keys.screen]`"),
        "says where t.go is: {text}"
    );
    let (keymap, _) = defaults();
    assert_eq!(applied.keymap, keymap, "nothing changed");
}

#[test]
fn an_action_straight_in_keys_says_to_use_a_context() {
    let applied = run(vec![entry("", "quit", &["x"])]);
    let text = applied.problems[0].to_string();
    assert!(text.contains("not directly in [keys]"), "{text}");
    assert!(text.contains("[keys.global]"), "{text}");
}

#[test]
fn an_unknown_action_says_which_and_what_the_context_has() {
    let applied = run(vec![
        entry("screen", "t.gone", &["w"]),
        entry("screen", "t.go", &["y"]),
    ]);
    let [problem] = &applied.problems[..] else {
        panic!("{:?}", texts(&applied));
    };
    assert!(
        matches!(problem, KeyProblem::UnknownAction { action, .. } if action == "t.gone"),
        "{problem:?}"
    );
    let text = problem.to_string();
    assert!(text.contains("[keys.screen] \"t.gone\""), "{text}");
    assert!(text.contains("t.go, t.stop"), "lists the actions: {text}");
    // The other line is not held back by it.
    assert_eq!(keys_of(&applied, SCREEN, GO), ["y"]);
}

#[test]
fn an_action_in_the_wrong_context_points_to_the_right_one() {
    let applied = run(vec![entry("screen", "list.next", &["z"])]);
    let text = applied.problems[0].to_string();
    assert!(
        text.contains("no action called `list.next` in `screen`"),
        "{text}"
    );
    assert!(text.contains("`[keys.\"screen.wall\"]`"), "{text}");
    assert!(text.contains("`[keys.other]`"), "{text}");
}

#[test]
fn an_unreadable_key_names_the_context_action_and_key() {
    let applied = run(vec![entry(
        "screen",
        "t.go",
        &["ctrl+", "w", "f99", "hyper"],
    )]);
    assert_eq!(applied.problems.len(), 3, "{:?}", texts(&applied));
    let text = applied.problems[0].to_string();
    assert!(
        text.contains("[keys.screen] \"t.go\" = \"ctrl+\""),
        "{text}"
    );
    assert!(
        applied
            .problems
            .iter()
            .any(|p| p.to_string().contains("f1 to f24"))
    );
    // One bad key leaves the action on its default, w is not taken.
    assert_eq!(keys_of(&applied, SCREEN, GO), ["g"]);
    let empty = run(vec![entry("screen", "t.go", &[""])]);
    assert!(empty.problems[0].to_string().contains("no key given"));
    let long = run(vec![entry("screen", "t.go", &["a b c d"])]);
    assert!(long.problems[0].to_string().contains("at most"));
}

#[test]
fn esc_and_ctrl_c_are_reserved_alone_and_in_a_chord() {
    for key in ["esc", "ctrl+c", "g esc", "Escape"] {
        let applied = run(vec![entry("screen", "t.go", &[key])]);
        let [problem] = &applied.problems[..] else {
            panic!("{key}: {:?}", texts(&applied));
        };
        assert!(matches!(problem, KeyProblem::Reserved { .. }), "{key}");
        let text = problem.to_string();
        assert!(text.contains("[keys.screen] \"t.go\""), "{text}");
        assert!(text.contains("reserved"), "{text}");
        assert_eq!(keys_of(&applied, SCREEN, GO), ["g"], "{key}");
    }
    assert!(
        run(vec![entry("screen", "t.go", &["ctrl+d"])])
            .problems
            .is_empty()
    );
}

#[test]
fn an_action_with_fixed_keys_cannot_be_rebound_or_unbound() {
    for keys in [&["w"][..], &[]] {
        let applied = run(vec![entry("screen", "t.fixed", keys)]);
        let [KeyProblem::Locked { action, .. }] = &applied.problems[..] else {
            panic!("{:?}", texts(&applied));
        };
        assert_eq!(*action, FIXED);
        assert!(applied.problems[0].to_string().contains("fixed"));
        assert_eq!(keys_of(&applied, SCREEN, FIXED), ["f"]);
    }
}

#[test]
fn two_actions_on_one_key_are_refused_and_the_action_keeps_its_default() {
    // k is skip's key.
    let applied = run(vec![
        entry("screen", "t.go", &["k"]),
        entry("screen", "t.stop", &["y"]),
    ]);
    let [problem] = &applied.problems[..] else {
        panic!("{:?}", texts(&applied));
    };
    assert!(matches!(
        problem,
        KeyProblem::Conflict { existing, action, .. } if *existing == SKIP && *action == GO
    ));
    let text = problem.to_string();
    assert!(text.contains("[keys.screen] \"t.go\" = \"k\""), "{text}");
    assert!(text.contains("already t.skip"), "{text}");
    assert!(
        text.contains("\"t.skip\" = []"),
        "says how to free it: {text}"
    );
    assert_eq!(keys_of(&applied, SCREEN, GO), ["g"], "default");
    assert_eq!(keys_of(&applied, SCREEN, SKIP), ["k"]);
    assert_eq!(
        keys_of(&applied, SCREEN, STOP),
        ["y"],
        "the good one stands"
    );
}

#[test]
fn two_entries_on_one_key_the_first_wins() {
    let applied = run(vec![
        entry("screen", "t.go", &["w"]),
        entry("screen", "t.stop", &["w"]),
    ]);
    assert_eq!(applied.problems.len(), 1, "{:?}", texts(&applied));
    assert_eq!(keys_of(&applied, SCREEN, GO), ["w"]);
    assert_eq!(keys_of(&applied, SCREEN, STOP), ["s", "x"], "default");
}

#[test]
fn a_chord_that_starts_another_is_refused() {
    // g is go's key, so `g w` would start with it.
    let applied = run(vec![entry("screen", "t.skip", &["g w"])]);
    let [problem] = &applied.problems[..] else {
        panic!("{:?}", texts(&applied));
    };
    assert!(matches!(problem, KeyProblem::Prefix { .. }));
    let text = problem.to_string();
    assert!(
        text.contains("[keys.screen] \"t.skip\" = \"g w\""),
        "{text}"
    );
    assert!(text.contains("starts the other"), "{text}");
    assert_eq!(keys_of(&applied, SCREEN, SKIP), ["k"]);
    // Moving go off g makes room.
    let applied = run(vec![
        entry("screen", "t.go", &["y"]),
        entry("screen", "t.skip", &["g w"]),
    ]);
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
}

#[test]
fn the_same_action_twice_uses_the_first() {
    let applied = run(vec![
        entry("screen", "t.go", &["w"]),
        entry("screen.wall", "list.next", &["z"]),
        entry("screen", "t.go", &["y"]),
    ]);
    let [KeyProblem::Twice { action, .. }] = &applied.problems[..] else {
        panic!("{:?}", texts(&applied));
    };
    assert_eq!(*action, GO);
    assert_eq!(keys_of(&applied, SCREEN, GO), ["w"]);
}

#[test]
fn a_value_of_the_wrong_kind_is_reported() {
    let (keymap, catalog) = defaults();
    let config = KeyConfig {
        entries: Vec::new(),
        malformed: vec![Malformed {
            at: "screen.t.go".into(),
            found: "the number 5".into(),
        }],
    };
    let applied = apply(&keymap, &catalog, &config);
    let text = applied.problems[0].to_string();
    assert!(text.contains("screen.t.go"), "{text}");
    assert!(text.contains("the number 5"), "{text}");
    assert_eq!(applied.keymap, keymap);
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let applied = run(vec![
        entry("nope", "t.go", &["w"]),
        entry("screen", "t.gone", &["w"]),
        entry("screen", "t.go", &["ctrl+"]),
        entry("screen", "t.skip", &["esc"]),
        entry("screen", "t.fixed", &["w"]),
        entry("screen", "t.stop", &["g"]),
    ]);
    assert_eq!(applied.problems.len(), 6, "{:?}", texts(&applied));
    let (keymap, _) = defaults();
    for context in [SCREEN, WALL, OTHER, GLOBAL] {
        let sorted = |m: &Keymap| {
            let mut all: Vec<_> = m
                .bindings(context)
                .iter()
                .map(|b| (b.chord.to_string(), b.action))
                .collect();
            all.sort();
            all
        };
        assert_eq!(
            sorted(&applied.keymap),
            sorted(&keymap),
            "none of them took effect in {context}"
        );
    }
}

#[test]
fn a_clash_that_cannot_be_undone_sends_the_context_back_to_its_defaults() {
    // go wants `s`, which is stop's; skip takes `y` first, so stop's `y`
    // is refused and stop goes back to s and x, which clash with go.
    let applied = run(vec![
        entry("screen", "t.go", &["s"]),
        entry("screen", "t.skip", &["y"]),
        entry("screen", "t.stop", &["y"]),
    ]);
    assert!(
        applied
            .problems
            .iter()
            .any(|p| matches!(p, KeyProblem::Reverted { context } if *context == SCREEN)),
        "{:?}",
        texts(&applied)
    );
    let (keymap, _) = defaults();
    assert_eq!(applied.keymap.bindings(SCREEN), keymap.bindings(SCREEN));
}

#[test]
fn a_config_with_every_context_broken_still_has_every_default() {
    let (keymap, catalog) = defaults();
    let applied = apply(
        &keymap,
        &catalog,
        &config(vec![
            entry("screen", "t.go", &["esc"]),
            entry("screen.wall", "list.next", &["ctrl+c"]),
            entry("global", "quit", &["x y z w"]),
        ]),
    );
    assert_eq!(applied.keymap, keymap);
    assert_eq!(applied.problems.len(), 3);
}

// ---- overlaps the user created

#[test]
fn only_overlaps_the_config_made_are_new() {
    let (defaults, catalog) = defaults();
    let stacks = vec![vec![SCREEN, GLOBAL], vec![WALL, SCREEN, GLOBAL]];
    assert!(new_shadows(&defaults, &defaults, &stacks).is_empty());

    // quit on `k`: the screen's skip comes first and hides it.
    let applied = apply(
        &defaults,
        &catalog,
        &config(vec![entry("global", "quit", &["k"])]),
    );
    assert!(applied.problems.is_empty(), "{:?}", texts(&applied));
    let found = new_shadows(&defaults, &applied.keymap, &stacks);
    assert_eq!(found.len(), 1, "once, however many stacks: {found:?}");
    let text = found[0].to_string();
    assert!(text.contains("t.skip"), "{text}");
    assert!(text.contains("quit"), "{text}");
}

#[test]
fn an_overlap_the_defaults_already_have_is_not_new() {
    let mut keymap = Keymap::new();
    keymap.bind(SCREEN, chord("t"), GO).unwrap();
    keymap
        .bind(GLOBAL, chord("t"), Action::new("theme"))
        .unwrap();
    let stacks = vec![vec![SCREEN, GLOBAL]];
    assert_eq!(keymap.shadows(&stacks[0]).len(), 1);
    assert!(new_shadows(&keymap, &keymap, &stacks).is_empty());
}

// ---- writing contexts and actions as TOML names

#[test]
fn names_with_a_dot_are_quoted() {
    assert_eq!(table_name("mission.wall"), "\"mission.wall\"");
    assert_eq!(table_name("global"), "global");
    assert_eq!(key_name("list.prev"), "\"list.prev\"");
    assert_eq!(key_name("quit"), "quit");
}
