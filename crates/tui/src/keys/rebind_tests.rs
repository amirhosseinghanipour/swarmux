//! The user's `[keys]` end to end: parsed, checked, applied to the app,
//! and shown in the footer, the tabs, the messages and the key list.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::keys::{GLOBAL, KeyProblem, KeySpec, Lookup};

use super::*;
use crate::app::tests_support::{TestApp, test_app};
use crate::app::{Mode, Screen};
use crate::help::{HelpLine, lines};
use crate::ui::snapshot::Snapshot;

fn with_keys(entries: &[(&str, &str, &[&str])]) -> TestApp {
    let mut t = test_app();
    t.app = t.app.with_key_config(&config_of(entries));
    t
}

fn chars(t: &mut TestApp, text: &str) {
    t.keys(text.chars().map(KeyCode::Char));
}

fn status(t: &TestApp, width: u16) -> String {
    Snapshot::of(&t.app, width, 24).status()
}

fn row<'a>(found: &'a [HelpLine], id: &str) -> Option<&'a HelpLine> {
    found
        .iter()
        .find(|l| matches!(l, HelpLine::Row { id: i, .. } if i == id))
}

// ---- the effective keymap

#[test]
fn no_config_is_the_default_keymap_and_nothing_to_report() {
    let found = effective(&KeyConfig::default()).unwrap();
    let (keymap, _) = defaults().unwrap();
    assert_eq!(found.keymap, keymap);
    assert!(found.problems.is_empty());
    assert!(found.warnings.is_empty());
}

#[test]
fn an_approval_answer_and_back_cannot_be_rebound() {
    for (section, name) in [
        ("decide", "decide.yes"),
        ("decide", "decide.no"),
        ("decide", "decide.always"),
        ("decide", "decide.cancel_plan"),
        ("global", "back"),
    ] {
        let found = effective(&config_of(&[(section, name, &["w"])])).unwrap();
        let [KeyProblem::Locked { .. }] = &found.problems[..] else {
            panic!("{name}: {:?}", found.problems);
        };
        let (keymap, _) = defaults().unwrap();
        assert_eq!(found.keymap, keymap, "{name} keeps its keys");
        // Not even unbinding is allowed.
        let found = effective(&config_of(&[(section, name, &[])])).unwrap();
        assert_eq!(found.problems.len(), 1, "{name}");
    }
}

#[test]
fn every_other_action_of_the_catalog_can_be_rebound() {
    let (keymap, catalog) = defaults().unwrap();
    for def in catalog.defs().iter().filter(|d| !d.locked) {
        let found = effective(&config_of(&[(
            def.context.id(),
            def.action.id(),
            &["ctrl+alt+f12"],
        )]))
        .unwrap();
        assert!(
            found.problems.is_empty(),
            "{} in {}: {:?}",
            def.action,
            def.context,
            found.problems
        );
        assert_ne!(found.keymap, keymap, "{}", def.action);
    }
}

#[test]
fn the_toml_way_of_nesting_names_works() {
    // `[keys.mission.wall]` with `list.next = "z"` is read by TOML as nested
    // tables, and means the same as the quoted names.
    let nested = effective(&config_of(&[("mission.wall.list", "next", &["z"])])).unwrap();
    let quoted = effective(&config_of(&[("mission.wall", "list.next", &["z"])])).unwrap();
    assert!(nested.problems.is_empty(), "{:?}", nested.problems);
    assert_eq!(nested.keymap, quoted.keymap);
}

#[test]
fn a_binding_that_hides_a_key_further_out_is_a_warning_not_an_error() {
    // `t` is the theme everywhere; the queue's cancel now takes it there.
    let found = effective(&config_of(&[("queue", "queue.cancel", &["t"])])).unwrap();
    assert!(found.problems.is_empty(), "{:?}", found.problems);
    let text: Vec<String> = found.warnings.iter().map(ToString::to_string).collect();
    assert!(
        text.iter()
            .any(|w| w.contains("queue.cancel") && w.contains("look.next_theme")),
        "{text:?}"
    );
    // The overlaps the defaults have by design are not warned about.
    assert!(
        effective(&KeyConfig::default())
            .unwrap()
            .warnings
            .is_empty()
    );
}

#[test]
fn a_decision_key_over_a_screen_key_is_warned_about() {
    // `y` answers whatever asks before any screen sees it.
    let found = effective(&config_of(&[("queue", "queue.cancel", &["y"])])).unwrap();
    assert!(
        found
            .warnings
            .iter()
            .any(|w| w.to_string().contains("decide.yes")),
        "{:?}",
        found.warnings
    );
}

// ---- a rebound key works, the old one does not

#[test]
fn a_rebound_screen_key_goes_where_the_old_one_did_and_the_old_key_does_not() {
    let mut t = with_keys(&[("global", "nav.go_ledger", &["f5"])]);
    assert_eq!(t.app.screen, Screen::Mission);
    t.keys([KeyCode::Char('5')]);
    assert_eq!(t.app.screen, Screen::Mission, "the old key does nothing");
    t.keys([KeyCode::F(5)]);
    assert_eq!(t.app.screen, Screen::Ledger);
}

#[test]
fn the_plan_example_gw_becomes_w() {
    let mut t = with_keys(&[("global", "nav.go_work", &["w"])]);
    // The chord is gone from the keymap; `g r` still goes to the record.
    let keymap = t.app.keymap().clone();
    let (g, w) = (KeySpec::char('g'), KeySpec::char('w'));
    assert_eq!(keymap.resolve(&[GLOBAL], &[g, w]), Lookup::None);
    assert!(matches!(
        keymap.resolve(&[GLOBAL], &[w]),
        Lookup::Actions(_)
    ));
    t.app.screen = Screen::Ledger;
    chars(&mut t, "w");
    assert_eq!(t.app.screen, Screen::Work, "w goes to work");
    assert_eq!(t.app.notice, None, "work is built: no stand-in notice");
    t.app.screen = Screen::Sessions;
    chars(&mut t, "gr");
    assert_eq!(
        t.app.screen,
        Screen::Ledger,
        "the other chords are untouched"
    );
}

#[test]
fn a_rebound_action_key_runs_the_action_and_the_old_key_is_unbound() {
    let mut t = with_keys(&[("queue", "queue.cancel", &["x"])]);
    t.app.screen = Screen::Queue;
    let keymap = t.app.keymap().clone();
    let queue = swarmux_domain::keys::Context::new("queue");
    let lookup = |k: char| keymap.resolve(&[queue], &[KeySpec::char(k)]);
    assert!(matches!(lookup('x'), Lookup::Actions(a) if a[0].id() == "queue.cancel"));
    assert_eq!(lookup('c'), Lookup::None);
}

#[test]
fn an_unbound_action_does_nothing_and_leaves_the_rest() {
    let mut t = with_keys(&[("global", "look.next_theme", &[])]);
    let before = t.app.look.theme.name;
    chars(&mut t, "t");
    assert_eq!(t.app.look.theme.name, before);
    chars(&mut t, "L");
    assert!(t.app.toast().is_some(), "other keys still work");
}

#[test]
fn text_editors_still_take_every_key_as_typed() {
    // `x` quits now; typing a session name with an x in it must not.
    let mut t = with_keys(&[("global", "quit", &["x"])]);
    t.keys([KeyCode::Char('6'), KeyCode::Char('n')]);
    assert!(matches!(t.app.mode, Mode::Input(..)));
    chars(&mut t, "xx");
    assert!(!t.app.quit, "typed, not a command");
    t.keys([KeyCode::Esc]);
    chars(&mut t, "x");
    assert!(t.app.quit, "and out of the editor it quits");
}

#[test]
fn esc_and_ctrl_c_work_whatever_the_config_says() {
    let mut t = with_keys(&[
        ("global", "quit", &["esc"]),
        ("global", "nav.go_ledger", &["ctrl+c"]),
    ]);
    assert_eq!(t.app.key_problems(), 2);
    t.keys([KeyCode::Char('5')]);
    assert_eq!(t.app.screen, Screen::Ledger, "the default is still there");
    t.keys([KeyCode::Esc]);
    assert_eq!(t.app.screen, Screen::Mission, "esc goes back");
    t.app
        .on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(t.app.quit);
}

#[test]
fn an_altgr_character_is_matched_by_what_it_types() {
    // Windows reports AltGr as Ctrl+Alt: a German `[` is that, and the
    // keymap sees a plain `[`.
    let spec = spec_of(KeyEvent::new(
        KeyCode::Char('['),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ));
    assert_eq!(spec, Some(KeySpec::char('[')));
    // And a key typed on a non-US layout is whatever character it makes.
    let spec = spec_of(KeyEvent::new(KeyCode::Char('é'), KeyModifiers::NONE));
    assert_eq!(spec, Some(KeySpec::char('é')));
    assert_eq!(KeySpec::parse("é").unwrap(), KeySpec::char('é'));
    let mut t = with_keys(&[("global", "nav.go_ledger", &["é"])]);
    t.keys([KeyCode::Char('é')]);
    assert_eq!(t.app.screen, Screen::Ledger);
}

// ---- a broken config

#[test]
fn a_broken_config_keeps_the_defaults_and_says_so_where_the_user_looks() {
    let t = with_keys(&[
        ("queue", "queue.cancel", &["x"]),
        ("queue", "queue.nope", &["y"]),
        ("nowhere", "queue.retry", &["z"]),
        ("global", "nav.go_ledger", &["ctrl+"]),
    ]);
    assert_eq!(t.app.key_problems(), 3);
    let line = status(&t, 200);
    assert!(line.contains("swarmux keys lint"), "{line}");
    assert!(line.contains("3 key settings"), "{line}");
    // The good entry took effect, the bad ones left their defaults.
    let (keymap, _) = defaults().unwrap();
    let queue = swarmux_domain::keys::Context::new("queue");
    let cancel = swarmux_domain::keys::Action::new("queue.cancel");
    let retry = swarmux_domain::keys::Action::new("queue.retry");
    assert_eq!(t.app.keymap().label_of(queue, cancel).as_deref(), Some("x"));
    assert_eq!(
        t.app.keymap().label_of(queue, retry),
        keymap.label_of(queue, retry)
    );
}

#[test]
fn the_warning_goes_away_with_time_and_a_good_config_has_none() {
    let mut t = with_keys(&[("queue", "queue.nope", &["y"])]);
    assert!(t.app.key_warning().is_some());
    t.app.frame += 100_000;
    assert!(t.app.key_warning().is_none());
    let t = with_keys(&[("queue", "queue.cancel", &["x"])]);
    assert!(t.app.key_warning().is_none());
    assert!(!status(&t, 200).contains("keys lint"));
}

// ---- hints, tabs, messages, help

#[test]
fn the_footer_shows_the_key_a_rebound_action_has_now() {
    let mut t = with_keys(&[
        ("queue", "queue.cancel", &["x"]),
        ("queue", "queue.priority_up", &["f2"]),
    ]);
    t.app.screen = Screen::Queue;
    let hints = crate::ui::hints(&t.app);
    assert!(hints.contains("x cancel"), "{hints}");
    assert!(hints.contains("f2/- priority"), "{hints}");
    assert!(!hints.contains("c cancel"), "{hints}");
    assert!(status(&t, 160).contains("x cancel"), "{}", status(&t, 160));
}

#[test]
fn an_unbound_action_has_no_hint() {
    let mut t = with_keys(&[("queue", "queue.retry", &[])]);
    t.app.screen = Screen::Queue;
    let hints = crate::ui::hints(&t.app);
    assert!(!hints.contains("retry"), "{hints}");
    assert!(hints.contains("c cancel"), "{hints}");
}

#[test]
fn moving_the_list_keys_changes_the_arrows_in_the_footer() {
    let mut t = with_keys(&[
        ("queue", "list.prev", &["k"]),
        ("queue", "list.next", &["j"]),
    ]);
    t.app.screen = Screen::Queue;
    let hints = crate::ui::hints(&t.app);
    assert!(hints.contains("k/j select"), "{hints}");
}

#[test]
fn the_tab_shows_the_key_of_a_rebound_screen() {
    let t = with_keys(&[("global", "nav.go_ledger", &["f5"])]);
    let s = Snapshot::of(&t.app, 200, 12);
    assert!(s.tabs().contains(" f5 ledger "), "{}", s.tabs());
    assert!(!s.tabs().contains(" 5 ledger "), "{}", s.tabs());
}

#[test]
fn the_screens_range_follows_the_digits_and_leaves_when_they_move() {
    let mut t = with_keys(&[("global", "nav.go_ledger", &["f5"])]);
    t.app.screen = Screen::Board;
    let hints = crate::ui::hints(&t.app);
    assert!(hints.contains("? help"), "{hints}");
    assert!(!hints.contains("0-9"), "{hints}");
    let mut t = test_app();
    t.app.screen = Screen::Board;
    assert!(crate::ui::hints(&t.app).contains("0-9 screens"));
}

#[test]
fn messages_name_the_keys_as_they_are_now() {
    let mut t = with_keys(&[
        ("global", "nav.go_sessions", &["f6"]),
        ("sessions", "sessions.new", &["N"]),
    ]);
    // No session yet: `p` asks for one.
    chars(&mut t, "p");
    let error = t.app.error.clone().unwrap();
    assert!(error.contains("press f6 then N"), "{error}");
    // The empty queue says it too.
    t.app.error = None;
    t.app.screen = Screen::Queue;
    let s = Snapshot::of(&t.app, 120, 20);
    assert!(
        s.contains("press 1 then p to send a prompt"),
        "{}",
        s.text()
    );
    t.app.screen = Screen::Sessions;
    let s = Snapshot::of(&t.app, 120, 20);
    assert!(s.contains("press N to create one"), "{}", s.text());
}

#[test]
fn the_shelve_prompt_names_its_keys() {
    let mut t = with_keys(&[
        ("shelve", "shelve.15_min", &["a"]),
        ("shelve", "shelve.until_job_ends", &["u"]),
    ]);
    t.app.mode = Mode::Shelve;
    let line = status(&t, 200);
    assert!(
        line.contains("shelve: a 15 min · 2 60 min · u until its job ends · esc cancel"),
        "{line}"
    );
    let mut t = test_app();
    t.app.mode = Mode::Shelve;
    let line = status(&t, 200);
    assert!(
        line.contains(
            "shelve: 1 15 min · 2 60 min · 3 until its job ends · esc cancel (it comes back by itself)"
        ),
        "{line}"
    );
}

#[test]
fn the_irreversible_message_names_the_confirm_key() {
    // The answers are fixed, so it is `y`; the text still comes from the keymap.
    let t = test_app();
    assert_eq!(crate::hints::key_or_name(&t.app, DECIDE_YES), "y");
}

// ---- the key list

#[test]
fn the_key_list_opens_with_question_mark_and_closes_with_it_or_esc() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    assert!(t.app.help.is_none());
    chars(&mut t, "?");
    assert!(t.app.help.is_some());
    chars(&mut t, "?");
    assert!(t.app.help.is_none(), "? closes it");
    chars(&mut t, "?");
    t.keys([KeyCode::Esc]);
    assert!(t.app.help.is_none(), "esc closes it");
    assert_eq!(t.app.screen, Screen::Queue, "and does not also go back");
}

#[test]
fn the_key_list_is_modal() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    chars(&mut t, "?");
    chars(&mut t, "5");
    assert_eq!(t.app.screen, Screen::Queue, "no navigation behind it");
    assert!(t.app.help.is_some());
    chars(&mut t, "t");
    assert!(t.app.toast().is_none());
    t.app
        .on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(t.app.quit, "ctrl+c still quits");
}

#[test]
fn the_key_list_shows_the_keys_of_this_screen_and_the_global_ones() {
    let mut t = with_keys(&[("queue", "queue.cancel", &["x"])]);
    t.app.screen = Screen::Queue;
    chars(&mut t, "?");
    let s = Snapshot::of(&t.app, 120, 50);
    let text = s.text();
    assert!(text.contains("keys · queue"), "{text}");
    assert!(text.contains("queue.cancel"), "{text}");
    assert!(text.contains("cancel the job"), "{text}");
    assert!(text.contains("help.toggle"), "{text}");
    assert!(text.contains("[keys.queue]"), "where to rebind: {text}");
    assert!(text.contains("(fixed)"), "the answers are marked: {text}");
    let found = lines(&t.app);
    let Some(HelpLine::Row { keys, .. }) = row(&found, "queue.cancel") else {
        panic!("no row for queue.cancel");
    };
    assert_eq!(keys, "x", "the rebound key, not the old one");
    let Some(HelpLine::Row { keys, .. }) = row(&found, "help.toggle") else {
        panic!("no row for help.toggle");
    };
    assert_eq!(keys, "?");
}

#[test]
fn the_key_list_follows_the_screen_and_its_state() {
    let mut t = test_app();
    t.app.screen = Screen::Mission;
    let inbox = lines(&t.app);
    assert!(row(&inbox, "mission.acknowledge").is_some());
    t.app.mission.pane = crate::mission::Pane::Wall;
    let wall = lines(&t.app);
    assert!(
        wall.iter()
            .any(|l| matches!(l, HelpLine::Heading { title, .. } if title == "mission.wall"))
    );
    assert!(
        !inbox
            .iter()
            .any(|l| matches!(l, HelpLine::Heading { title, .. } if title == "mission.wall"))
    );
}

#[test]
fn the_key_list_mentions_a_broken_config() {
    let mut t = with_keys(&[("queue", "queue.nope", &["y"])]);
    let found = lines(&t.app);
    assert!(
        matches!(&found[0], HelpLine::Note(n) if n.contains("swarmux keys lint")),
        "{:?}",
        found[0]
    );
    t.app.screen = Screen::Queue;
    let ok = lines(&test_app().app);
    assert!(!ok.iter().any(|l| matches!(l, HelpLine::Note(_))));
}

#[test]
fn the_key_list_scrolls_and_never_runs_off_its_end() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    chars(&mut t, "?");
    // A small terminal: not everything fits.
    let top = Snapshot::of(&t.app, 100, 12).text();
    assert!(top.contains("queue"), "{top}");
    for _ in 0..200 {
        t.keys([KeyCode::Down]);
    }
    let bottom = Snapshot::of(&t.app, 100, 12).text();
    assert_ne!(top, bottom, "it scrolled");
    assert!(bottom.contains("nav.go_record"), "the end shows: {bottom}");
    for _ in 0..200 {
        t.keys([KeyCode::Up]);
    }
    assert_eq!(Snapshot::of(&t.app, 100, 12).text(), top, "and back");
}

#[test]
fn the_key_list_follows_a_rebound_toggle() {
    let mut t = with_keys(&[("global", "help.toggle", &["f1"])]);
    chars(&mut t, "?");
    assert!(t.app.help.is_none(), "the old key does nothing");
    t.keys([KeyCode::F(1)]);
    assert!(t.app.help.is_some());
    t.keys([KeyCode::F(1)]);
    assert!(t.app.help.is_none());
}

#[test]
fn nothing_of_the_screen_shows_beside_the_key_list() {
    use crate::ui::help::MAX_WIDTH;
    use ratatui::layout::Rect;
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    let behind = Snapshot::of(&t.app, 200, 80);
    let body = behind.body();
    let side = (200 - MAX_WIDTH) / 2;
    let strip = |s: &Snapshot, x: u16, w: u16| s.region(Rect::new(x, body.y, w, body.height));
    assert!(
        !strip(&behind, 0, side).trim().is_empty(),
        "the queue draws there without the list"
    );
    chars(&mut t, "?");
    let s = Snapshot::of(&t.app, 200, 80);
    assert!(s.contains("keys · queue"), "{}", s.text());
    assert!(strip(&s, 0, side).trim().is_empty(), "left of the list");
    assert!(
        strip(&s, 200 - side, side).trim().is_empty(),
        "right of the list"
    );
    // Above and below it too, in a taller window than the list needs.
    let first = (body.y..body.bottom())
        .find(|y| s.row(*y).contains("keys · queue"))
        .unwrap();
    assert!(first > body.y, "the panel is centred");
    assert!(
        s.region(Rect::new(0, body.y, 200, first - body.y))
            .trim()
            .is_empty(),
        "above the list"
    );
    assert!(
        s.region(Rect::new(0, body.bottom() - 1, 200, 1))
            .trim()
            .is_empty(),
        "below the list"
    );
    // Narrow terminals: the list fills the width and nothing leaks either.
    let n = Snapshot::of(&t.app, 90, 40);
    assert!(n.contains("keys · queue"), "{}", n.text());
}
