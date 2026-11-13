use super::*;

const A: Context = Context::new("a");
const B: Context = Context::new("b");

const ONE: Action = Action::new("t.one");
const TWO: Action = Action::new("t.two");
const THREE: Action = Action::new("t.three");

fn key(text: &str) -> KeySpec {
    KeySpec::parse(text).unwrap()
}

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap()
}

fn map(binds: &[(Context, &str, Action)]) -> Keymap {
    let mut m = Keymap::new();
    for (context, keys, action) in binds {
        m.bind(*context, chord(keys), *action).unwrap();
    }
    m
}

// ---- keys

#[test]
fn keys_read_and_write_the_same_text() {
    for text in [
        "q",
        "D",
        "1",
        "+",
        "ctrl+s",
        "alt+x",
        "enter",
        "esc",
        "tab",
        "shift+tab",
        "space",
        "up",
        "pagedown",
        "f5",
        "ctrl+alt+f12",
        "shift+up",
    ] {
        assert_eq!(key(text).to_string(), text, "{text}");
    }
    assert_eq!(chord("g w").to_string(), "g w");
    assert_eq!(chord("ctrl+x ctrl+s").to_string(), "ctrl+x ctrl+s");
}

#[test]
fn keys_have_one_canonical_form() {
    assert_eq!(key("shift+d"), key("D"));
    assert_eq!(key("Shift+D"), key("D"));
    assert_eq!(key("shift+tab"), key("backtab"));
    assert_eq!(key("CTRL+s"), key("ctrl+s"));
    assert_eq!(key("return"), key("enter"));
    assert_eq!(key("ctrl++"), KeySpec::ctrl('+'));
    assert_eq!(
        KeySpec::new(Key::Char('d'), Mods::SHIFT),
        KeySpec::char('D'),
        "shift is part of the character"
    );
    assert_eq!(
        KeySpec::new(Key::BackTab, Mods::SHIFT),
        KeySpec::plain(Key::BackTab)
    );
    assert_eq!(
        KeySpec::new(Key::Tab, Mods::SHIFT),
        KeySpec::plain(Key::BackTab)
    );
    assert_ne!(key("ctrl+s"), key("s"), "modifiers count");
    assert_ne!(key("alt+s"), key("s"));
    assert_ne!(key("q"), key("Q"), "case counts");
}

#[test]
fn bad_keys_say_why() {
    assert_eq!(KeySpec::parse(""), Err(KeyError::Empty));
    assert_eq!(KeySpec::parse("  "), Err(KeyError::Empty));
    assert!(matches!(KeySpec::parse("ctrl+"), Err(KeyError::Unknown(_))));
    assert!(matches!(KeySpec::parse("hyper"), Err(KeyError::Unknown(_))));
    assert!(matches!(
        KeySpec::parse("f0"),
        Err(KeyError::FunctionKey(_))
    ));
    assert!(matches!(
        KeySpec::parse("f25"),
        Err(KeyError::FunctionKey(_))
    ));
    assert_eq!(Chord::parse(""), Err(KeyError::Empty));
    assert!(matches!(Chord::parse("a b c d"), Err(KeyError::TooLong(_))));
    assert!(Chord::parse("a b c").is_ok());
}

#[test]
fn only_esc_and_ctrl_c_are_reserved() {
    assert!(is_reserved(key("esc")));
    assert!(is_reserved(key("ctrl+c")));
    assert!(!is_reserved(key("c")));
    assert!(!is_reserved(key("ctrl+d")));
    assert!(!is_reserved(key("alt+esc")));
}

// ---- binding

#[test]
fn reserved_keys_cannot_be_bound() {
    let mut m = Keymap::new();
    assert_eq!(
        m.bind(A, chord("esc"), ONE),
        Err(BindError::Reserved(key("esc")))
    );
    assert_eq!(
        m.bind(GLOBAL, chord("ctrl+c"), ONE),
        Err(BindError::Reserved(key("ctrl+c")))
    );
    assert_eq!(
        m.bind(A, chord("g esc"), ONE),
        Err(BindError::Reserved(key("esc"))),
        "not even inside a chord"
    );
    assert_eq!(m.bindings(A), &[]);
    assert!(m.bind(A, chord("ctrl+d"), ONE).is_ok());
}

#[test]
fn one_chord_means_one_action_per_context() {
    let mut m = Keymap::new();
    m.bind(A, chord("x"), ONE).unwrap();
    assert!(m.bind(A, chord("x"), ONE).is_ok(), "the same again is fine");
    assert_eq!(m.bindings(A).len(), 1);
    assert_eq!(
        m.bind(A, chord("x"), TWO),
        Err(BindError::Conflict {
            context: A,
            chord: chord("x"),
            existing: ONE,
            new: TWO,
        })
    );
    // Another context is free to use it.
    assert!(m.bind(B, chord("x"), TWO).is_ok());
    // One action may have several keys.
    assert!(m.bind(A, chord("y"), ONE).is_ok());
}

#[test]
fn a_chord_cannot_start_another_in_the_same_context() {
    let mut m = Keymap::new();
    m.bind(A, chord("g w"), ONE).unwrap();
    for (c, other) in [("g", "g w"), ("g w x", "g w")] {
        let err = m.bind(A, chord(c), TWO).unwrap_err();
        assert_eq!(
            err,
            BindError::Prefix {
                context: A,
                chord: chord(c),
                other: chord(other),
            }
        );
    }
    assert!(m.bind(A, chord("g d"), TWO).is_ok(), "same prefix is fine");
    assert!(m.bind(B, chord("g"), THREE).is_ok(), "other context");
}

#[test]
fn rebinding_replaces_the_keys_or_changes_nothing() {
    let mut m = map(&[(A, "w", ONE), (A, "g w", ONE), (A, "x", TWO)]);
    m.rebind(A, ONE, vec![chord("z")]).unwrap();
    assert_eq!(
        m.chords_of(A, ONE).cloned().collect::<Vec<_>>(),
        vec![chord("z")]
    );
    let before = m.clone();
    let err = m.rebind(A, ONE, vec![chord("q"), chord("x")]).unwrap_err();
    assert!(matches!(err, BindError::Conflict { existing: TWO, .. }));
    assert_eq!(m, before, "a refused rebind leaves the map alone");
    let err = m.rebind(A, ONE, vec![chord("esc")]).unwrap_err();
    assert_eq!(err, BindError::Reserved(key("esc")));
    assert_eq!(m, before);
    m.unbind(A, ONE);
    assert_eq!(m.chords_of(A, ONE).count(), 0);
}

#[test]
fn labels_come_from_the_first_binding() {
    let m = map(&[(A, "4", ONE), (A, "g q", ONE)]);
    assert_eq!(m.label_of(A, ONE).as_deref(), Some("4"));
    assert_eq!(m.label_of(A, TWO), None);
    assert_eq!(m.label_of(B, ONE), None);
}

// ---- resolving

#[test]
fn the_innermost_context_comes_first_and_the_rest_follow() {
    let m = map(&[(A, "t", ONE), (B, "t", TWO), (B, "u", THREE)]);
    assert_eq!(
        m.resolve(&[A, B], &[key("t")]),
        Lookup::Actions(vec![ONE, TWO]),
        "the first that applies is run, so the screen can pass"
    );
    assert_eq!(
        m.resolve(&[B, A], &[key("t")]),
        Lookup::Actions(vec![TWO, ONE])
    );
    assert_eq!(
        m.resolve(&[A, B], &[key("u")]),
        Lookup::Actions(vec![THREE])
    );
    assert_eq!(m.resolve(&[A], &[key("u")]), Lookup::None);
    assert_eq!(m.resolve(&[], &[key("t")]), Lookup::None);
}

#[test]
fn a_context_that_starts_a_chord_holds_the_key() {
    let m = map(&[(A, "g w", ONE), (B, "g", TWO)]);
    assert_eq!(m.resolve(&[A, B], &[key("g")]), Lookup::Pending);
    assert_eq!(
        m.resolve(&[B, A], &[key("g")]),
        Lookup::Actions(vec![TWO]),
        "an exact match further in wins over a chord further out"
    );
    assert_eq!(
        m.resolve(&[A, B], &[key("g"), key("w")]),
        Lookup::Actions(vec![ONE])
    );
    assert_eq!(m.resolve(&[A, B], &[key("g"), key("x")]), Lookup::None);
}

#[test]
fn shadows_list_what_an_inner_context_covers() {
    let m = map(&[
        (A, "t", ONE),
        (B, "t", TWO),
        (B, "q", THREE),
        (B, "g w", THREE),
    ]);
    let shadows = m.shadows(&[A, B]);
    assert_eq!(
        shadows,
        vec![Shadow {
            winner: (A, ONE, chord("t")),
            hidden: (B, TWO, chord("t")),
        }]
    );
    assert!(m.shadows(&[B]).is_empty());
    let m = map(&[(A, "g", ONE), (B, "g w", TWO)]);
    assert_eq!(m.shadows(&[A, B]).len(), 1, "a chord start hides the chord");
}

#[test]
fn bindings_to_undeclared_actions_are_found() {
    let m = map(&[(A, "x", ONE), (A, "y", TWO), (B, "x", ONE)]);
    let mut catalog = Catalog::new();
    catalog.add(A, ONE, "one");
    catalog.add(B, TWO, "two");
    assert_eq!(m.unknown(&catalog), vec![(A, TWO), (B, ONE)]);
    assert_eq!(catalog.find("a", "t.one"), Some(ONE));
    assert_eq!(catalog.find("a", "t.two"), None, "declared in b only");
    assert_eq!(catalog.of(A).count(), 1);
}

// ---- the resolver

fn feed(r: &mut Resolver, m: &Keymap, stack: &[Context], text: &str, now: i64) -> Outcome {
    r.feed(m, stack, key(text), now)
}

#[test]
fn a_chord_takes_two_presses() {
    let m = map(&[(GLOBAL, "g w", ONE), (GLOBAL, "q", TWO)]);
    let mut r = Resolver::new();
    assert_eq!(feed(&mut r, &m, &[GLOBAL], "g", 0), Outcome::Pending);
    assert_eq!(r.pending(), &[key("g")]);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "w", 10),
        Outcome::Actions(vec![ONE])
    );
    assert!(r.pending().is_empty());
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "q", 20),
        Outcome::Actions(vec![TWO])
    );
}

#[test]
fn a_wrong_second_key_drops_the_chord_and_is_not_used() {
    let m = map(&[(GLOBAL, "g w", ONE), (GLOBAL, "q", TWO)]);
    let mut r = Resolver::new();
    feed(&mut r, &m, &[GLOBAL], "g", 0);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "q", 1),
        Outcome::Cancelled(vec![key("g"), key("q")]),
        "q does not quit after a stray g"
    );
    assert!(r.pending().is_empty());
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "q", 2),
        Outcome::Actions(vec![TWO])
    );
    assert_eq!(feed(&mut r, &m, &[GLOBAL], "z", 3), Outcome::Unbound);
}

#[test]
fn esc_drops_a_chord_else_goes_back() {
    let m = map(&[(GLOBAL, "g w", ONE)]);
    let mut r = Resolver::new();
    feed(&mut r, &m, &[GLOBAL], "g", 0);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "esc", 1),
        Outcome::Cancelled(vec![key("g"), key("esc")])
    );
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "esc", 2),
        Outcome::Actions(vec![actions::BACK])
    );
}

#[test]
fn ctrl_c_always_quits() {
    let m = Keymap::new();
    let mut r = Resolver::new();
    assert_eq!(
        feed(&mut r, &m, &[], "ctrl+c", 0),
        Outcome::Actions(vec![actions::QUIT])
    );
    let m = map(&[(GLOBAL, "g w", ONE)]);
    feed(&mut r, &m, &[GLOBAL], "g", 0);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "ctrl+c", 1),
        Outcome::Actions(vec![actions::QUIT]),
        "even half way through a chord"
    );
    assert!(r.pending().is_empty());
}

#[test]
fn a_half_typed_chord_expires() {
    let m = map(&[(GLOBAL, "g w", ONE), (GLOBAL, "w", TWO)]);
    let mut r = Resolver::new();
    feed(&mut r, &m, &[GLOBAL], "g", 1_000);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "w", 1_000 + CHORD_TIMEOUT_MS + 1),
        Outcome::Actions(vec![TWO]),
        "too late: w is just w"
    );
    feed(&mut r, &m, &[GLOBAL], "g", 10_000);
    assert_eq!(
        feed(&mut r, &m, &[GLOBAL], "w", 10_000 + CHORD_TIMEOUT_MS),
        Outcome::Actions(vec![ONE]),
        "in time"
    );
}

#[test]
fn a_waiting_chord_can_be_expired_without_a_key() {
    let m = map(&[(GLOBAL, "g w", ONE)]);
    let mut r = Resolver::new();
    feed(&mut r, &m, &[GLOBAL], "g", 0);
    r.expire(CHORD_TIMEOUT_MS);
    assert_eq!(r.pending().len(), 1, "still within its time");
    r.expire(CHORD_TIMEOUT_MS + 1);
    assert!(r.pending().is_empty());
    r.expire(i64::MAX);
}

#[test]
fn the_contexts_active_now_decide_the_chord() {
    let m = map(&[(A, "g w", ONE), (B, "g d", TWO)]);
    let mut r = Resolver::new();
    assert_eq!(feed(&mut r, &m, &[A], "g", 0), Outcome::Pending);
    assert_eq!(
        feed(&mut r, &m, &[A], "d", 1),
        Outcome::Cancelled(vec![key("g"), key("d")]),
        "b's chord is not active on a"
    );
    assert_eq!(feed(&mut r, &m, &[A, B], "g", 2), Outcome::Pending);
    assert_eq!(
        feed(&mut r, &m, &[A, B], "d", 3),
        Outcome::Actions(vec![TWO])
    );
}
