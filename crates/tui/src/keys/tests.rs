//! The default keymap: it must be the keys swarmux has always had.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::keys::{
    Action, BindError, Chord, Context, GLOBAL, Key, KeySpec, Keymap, Lookup, Mods, actions,
};

use super::*;
use crate::screens::{self, Screen};

fn ctx(id: &str) -> Context {
    keymap_contexts()
        .into_iter()
        .find(|c| c.id() == id)
        .unwrap_or_else(|| panic!("no context {id}"))
}

fn keymap_contexts() -> Vec<Context> {
    defaults().unwrap().0.contexts().collect()
}

/// Every default binding as `(context, key, action id)`: the key map as it
/// was before keys became data, written out by hand. A key added to or
/// dropped from the defaults fails here until this list says so.
const TODAYS_KEYS: &[(&str, &str, &str)] = &[
    // Everywhere.
    ("global", "q", "quit"),
    ("global", "t", "look.next_theme"),
    ("global", "L", "look.next_layout"),
    ("global", "F", "inbox.focus_mode"),
    ("global", "z", "showcase.constellation"),
    ("global", "S", "showcase.spectrum"),
    // The screens, by number.
    ("global", "1", "nav.go_mission"),
    ("global", "2", "nav.go_missions"),
    ("global", "3", "nav.go_prompts"),
    ("global", "4", "nav.go_queue"),
    ("global", "5", "nav.go_ledger"),
    ("global", "6", "nav.go_sessions"),
    ("global", "0", "nav.go_focus"),
    ("global", "7", "nav.go_board"),
    ("global", "8", "nav.go_knowledge"),
    ("global", "9", "nav.go_followups"),
    // What answers a plan, the JEV consent or a held call.
    ("decide", "y", "decide.yes"),
    ("decide", "n", "decide.no"),
    ("decide", "a", "decide.always"),
    ("decide", "c", "decide.cancel_plan"),
    // After shelving.
    ("shelve", "1", "shelve.15_min"),
    ("shelve", "2", "shelve.60_min"),
    ("shelve", "3", "shelve.until_job_ends"),
    // Mission control.
    ("mission", "tab", "mission.switch_pane"),
    ("mission", "shift+tab", "mission.switch_pane"),
    ("mission", "x", "mission.acknowledge"),
    ("mission", "A", "mission.answer_batch"),
    ("mission", "s", "mission.shelve"),
    ("mission", "u", "mission.unshelve"),
    ("mission", "r", "mission.rule_take"),
    ("mission", "R", "mission.rule_decline"),
    ("mission", "enter", "list.open"),
    ("mission", "p", "prompt.send"),
    ("mission", "P", "prompt.route"),
    ("mission.wall", "up", "list.prev"),
    ("mission.wall", "left", "list.prev"),
    ("mission.wall", "k", "list.prev"),
    ("mission.wall", "h", "list.prev"),
    ("mission.wall", "down", "list.next"),
    ("mission.wall", "right", "list.next"),
    ("mission.wall", "j", "list.next"),
    ("mission.wall", "l", "list.next"),
    ("mission.inbox", "up", "list.prev"),
    ("mission.inbox", "k", "list.prev"),
    ("mission.inbox", "down", "list.next"),
    ("mission.inbox", "j", "list.next"),
    // A question on the board, answered from the inbox.
    ("board.ask", "r", "board.answer"),
    ("board.ask", "enter", "board.answer"),
    // Missions.
    ("missions", "up", "list.prev"),
    ("missions", "k", "list.prev"),
    ("missions", "down", "list.next"),
    ("missions", "j", "list.next"),
    ("missions", "n", "missions.new"),
    ("missions", "e", "missions.edit"),
    ("missions", "l", "missions.lint"),
    ("missions", "f", "missions.fix"),
    ("missions", "r", "missions.run"),
    ("missions", "p", "missions.plan_first"),
    ("missions", "enter", "list.open"),
    // Prompts.
    ("prompts", "n", "prompts.new"),
    ("prompts", "e", "prompts.edit"),
    ("prompts", "x", "prompts.delete"),
    ("prompts", "d", "prompts.dispatch"),
    ("prompts", "D", "prompts.route"),
    ("prompts", "up", "list.prev"),
    ("prompts", "down", "list.next"),
    // Queue.
    ("queue", "up", "list.prev"),
    ("queue", "down", "list.next"),
    ("queue", "c", "queue.cancel"),
    ("queue", "r", "queue.retry"),
    ("queue", "+", "queue.priority_up"),
    ("queue", "=", "queue.priority_up"),
    ("queue", "-", "queue.priority_down"),
    ("queue", "enter", "list.open"),
    // Ledger.
    ("ledger", "/", "ledger.search"),
    ("ledger", "o", "ledger.filter"),
    ("ledger", "l", "ledger.labels"),
    ("ledger", "up", "list.prev"),
    ("ledger", "down", "list.next"),
    ("ledger", "r", "ledger.rerun"),
    ("ledger", "f", "ledger.fork"),
    ("ledger", "enter", "list.open"),
    ("ledger", "v", "ledger.verify"),
    // Sessions.
    ("sessions", "n", "sessions.new"),
    ("sessions", "w", "sessions.on_workspace"),
    ("sessions", "l", "sessions.toggle_lean"),
    ("sessions", "up", "list.prev"),
    ("sessions", "down", "list.next"),
    // Focus, both views.
    ("focus", "D", "focus.toggle_classic"),
    ("focus", "v", "focus.toggle_review"),
    ("focus", "p", "prompt.send"),
    ("focus", "P", "prompt.route"),
    // Focus, the reading view.
    ("focus.reading", "up", "focus.scroll_up"),
    ("focus.reading", "k", "focus.scroll_up"),
    ("focus.reading", "down", "focus.scroll_down"),
    ("focus.reading", "j", "focus.scroll_down"),
    ("focus.reading", "J", "focus.answer_down"),
    ("focus.reading", "K", "focus.answer_up"),
    ("focus.reading", "pageup", "focus.page_up"),
    ("focus.reading", "pagedown", "focus.page_down"),
    ("focus.reading", "home", "focus.top"),
    ("focus.reading", "end", "focus.bottom"),
    ("focus.reading", "G", "focus.bottom"),
    ("focus.reading", "d", "focus.toggle_diff"),
    ("focus.reading", "[", "focus.checkpoint_prev"),
    ("focus.reading", "]", "focus.checkpoint_next"),
    ("focus.reading", "t", "focus.open_terminal"),
    // Focus, the classic panel grid.
    ("focus.grid", "tab", "focus.panel_next"),
    ("focus.grid", "shift+tab", "focus.panel_prev"),
    ("focus.grid", "up", "list.prev"),
    ("focus.grid", "down", "list.next"),
    // Board.
    ("board", "up", "list.prev"),
    ("board", "k", "list.prev"),
    ("board", "down", "list.next"),
    ("board", "j", "list.next"),
    ("board", "r", "board.answer"),
    ("board", "enter", "board.answer"),
    ("board", "x", "board.dismiss"),
    // Knowledge.
    ("knowledge", "t", "knowledge.toggle_contracts"),
    ("knowledge.items", "up", "list.prev"),
    ("knowledge.items", "k", "list.prev"),
    ("knowledge.items", "down", "list.next"),
    ("knowledge.items", "j", "list.next"),
    ("knowledge.items", "a", "knowledge.add"),
    ("knowledge.items", "e", "knowledge.edit"),
    ("knowledge.items", "p", "knowledge.promote"),
    ("knowledge.items", "c", "knowledge.confirm"),
    ("knowledge.items", "x", "knowledge.retire"),
    ("knowledge.contracts", "up", "list.prev"),
    ("knowledge.contracts", "k", "list.prev"),
    ("knowledge.contracts", "down", "list.next"),
    ("knowledge.contracts", "j", "list.next"),
    ("knowledge.contracts", "r", "knowledge.reread_contracts"),
    ("knowledge.contracts", "w", "knowledge.next_workspace"),
    // Follow-ups.
    ("followups", "up", "list.prev"),
    ("followups", "k", "list.prev"),
    ("followups", "down", "list.next"),
    ("followups", "j", "list.next"),
    ("followups", "a", "followups.approve"),
    ("followups", "d", "followups.decline"),
    ("followups", "l", "followups.later"),
    ("followups", "o", "followups.open_job"),
    ("followups", "m", "followups.draft_mission"),
];

/// Keys that are new: `g` and a letter goes to a screen, including the
/// ones that are planned but not built.
const NEW_KEYS: &[(&str, &str, &str)] = &[
    ("global", "g m", "nav.go_missions"),
    ("global", "g p", "nav.go_prompts"),
    ("global", "g k", "nav.go_knowledge"),
    ("global", "g d", "nav.go_deck"),
    ("global", "g w", "nav.go_work"),
    ("global", "g r", "nav.go_record"),
];

/// The Work screen's own keys.
const WORK_KEYS: &[(&str, &str, &str)] = &[
    ("work", "up", "list.prev"),
    ("work", "k", "list.prev"),
    ("work", "down", "list.next"),
    ("work", "j", "list.next"),
    ("work", "1", "work.tab_product"),
    ("work", "2", "work.tab_stream"),
    ("work", "3", "work.tab_diff"),
    ("work", "4", "work.tab_checks"),
    ("work", "pageup", "work.scroll_up"),
    ("work", "pagedown", "work.scroll_down"),
    ("work", "l", "work.land"),
    ("work", "x", "work.kill"),
    ("work", "r", "work.retry"),
    ("work", "R", "work.resume"),
    ("work", "e", "work.evidence"),
    ("work", "enter", "list.open"),
];

/// Keys that came with customizable hotkeys: the key list.
const HELP_KEYS: &[(&str, &str, &str)] = &[
    ("global", "?", "help.toggle"),
    ("help", "up", "list.prev"),
    ("help", "k", "list.prev"),
    ("help", "down", "list.next"),
    ("help", "j", "list.next"),
    ("help", "pageup", "help.page_up"),
    ("help", "pagedown", "help.page_down"),
];

fn bound(keymap: &Keymap) -> Vec<(String, String, String)> {
    let mut all = Vec::new();
    for context in keymap.contexts() {
        for b in keymap.bindings(context) {
            all.push((
                context.id().to_string(),
                b.chord.to_string(),
                b.action.id().to_string(),
            ));
        }
    }
    all.sort();
    all
}

fn table(rows: &[(&str, &str, &str)]) -> Vec<(String, String, String)> {
    let mut all: Vec<_> = rows
        .iter()
        .map(|(c, k, a)| (c.to_string(), k.to_string(), a.to_string()))
        .collect();
    all.sort();
    all
}

#[test]
fn the_default_keymap_builds_without_conflicts_or_unknown_actions() {
    let (keymap, catalog) = defaults().expect("no conflicts");
    assert!(keymap.unknown(&catalog).is_empty());
    for def in catalog.defs() {
        assert!(!def.help.is_empty(), "{} has no help", def.action);
    }
}

#[test]
fn the_defaults_are_todays_keys_and_the_new_chords_and_nothing_else() {
    let (keymap, _) = defaults().unwrap();
    let mut want = table(TODAYS_KEYS);
    want.extend(table(NEW_KEYS));
    want.extend(table(HELP_KEYS));
    want.extend(table(WORK_KEYS));
    want.sort();
    assert_eq!(bound(&keymap), want);
}

#[test]
fn the_screens_keep_their_numbers_and_titles() {
    // (screen, tab title, the key that shows it)
    let today = [
        (Screen::Mission, "mission", "1"),
        (Screen::Work, "work", "g w"),
        (Screen::Missions, "missions", "2"),
        (Screen::Prompts, "prompts", "3"),
        (Screen::Queue, "queue", "4"),
        (Screen::Ledger, "ledger", "5"),
        (Screen::Sessions, "sessions", "6"),
        (Screen::Dashboard, "focus", "0"),
        (Screen::Board, "board", "7"),
        (Screen::Knowledge, "knowledge", "8"),
        (Screen::Followups, "follow-ups", "9"),
    ];
    let (keymap, _) = defaults().unwrap();
    assert_eq!(
        screens::all().collect::<Vec<_>>(),
        today.iter().map(|t| t.0).collect::<Vec<_>>(),
        "the tab order"
    );
    for (screen, title, key) in today {
        assert_eq!(screen.title(), title);
        assert_eq!(nav_label(&keymap, screen), key, "{title}");
        if key.contains(' ') {
            // Work has no digit: the g chord goes there.
            continue;
        }
        let spec = KeySpec::parse(key).unwrap();
        assert_eq!(
            keymap.resolve(&[GLOBAL], &[spec]),
            Lookup::Actions(vec![screen.def().nav_action()]),
            "{key} goes to {title}"
        );
    }
}

#[test]
fn the_g_chords_resolve_to_the_screens_they_stand_for() {
    let (keymap, _) = defaults().unwrap();
    let go = |text: &str| keymap.resolve(&[GLOBAL], Chord::parse(text).unwrap().keys());
    assert_eq!(go("g"), Lookup::Pending, "g alone waits for the letter");
    for (chord, action) in [
        ("g m", actions::GO_MISSIONS),
        ("g p", actions::GO_PROMPTS),
        ("g k", actions::GO_KNOWLEDGE),
        ("g d", actions::GO_DECK),
        ("g w", actions::GO_WORK),
        ("g r", actions::GO_RECORD),
    ] {
        assert_eq!(go(chord), Lookup::Actions(vec![action]), "{chord}");
    }
    assert_eq!(go("g x"), Lookup::None);
}

#[test]
fn a_plain_g_is_free_for_the_chords_on_every_screen() {
    // `g` starts the navigation chords, so no screen may bind it alone.
    let (keymap, _) = defaults().unwrap();
    let g = KeySpec::char('g');
    for context in keymap.contexts() {
        for b in keymap.bindings(context) {
            if context != GLOBAL {
                assert_ne!(b.chord.keys().first(), Some(&g), "{context}: {}", b.chord);
            }
        }
    }
}

#[test]
fn esc_and_ctrl_c_are_not_rebindable() {
    let (mut keymap, _) = defaults().unwrap();
    for key in ["esc", "ctrl+c"] {
        let chord = Chord::parse(key).unwrap();
        for context in keymap.contexts().collect::<Vec<_>>() {
            let err = keymap
                .bind(context, chord.clone(), Action::new("test.x"))
                .unwrap_err();
            assert!(matches!(err, BindError::Reserved(_)), "{context}: {key}");
        }
        assert!(
            keymap
                .rebind(GLOBAL, actions::QUIT, vec![chord.clone()])
                .is_err(),
            "quit can't move to {key}"
        );
    }
    // And no default uses them.
    for context in keymap.contexts() {
        for b in keymap.bindings(context) {
            for k in b.chord.keys() {
                assert!(!swarmux_domain::keys::is_reserved(*k), "{context}: {k}");
            }
        }
    }
}

#[test]
fn two_actions_cannot_share_a_key_in_one_context() {
    let (mut keymap, _) = defaults().unwrap();
    let err = keymap
        .bind(
            ctx("queue"),
            Chord::parse("c").unwrap(),
            Action::new("queue.reset"),
        )
        .unwrap_err();
    assert!(matches!(err, BindError::Conflict { .. }), "{err}");
    let err = keymap
        .bind(GLOBAL, Chord::parse("g").unwrap(), Action::new("test.g"))
        .unwrap_err();
    assert!(matches!(err, BindError::Prefix { .. }), "{err}");
}

#[test]
fn the_overlaps_between_contexts_are_the_ones_meant() {
    // An inner context hides the same key further out. These are all of
    // them, by screen state: `y n a c` answer a decision before a screen
    // sees them, a question on the board is answered before the inbox
    // takes `r` and Enter (the keys pass on when no question is selected),
    // and Focus and Knowledge use `t` for their own things.
    let (keymap, _) = defaults().unwrap();
    let mut seen: Vec<String> = Vec::new();
    for screen in screens::all() {
        let t = crate::app::tests_support::test_app();
        let mut app = t.app;
        app.screen = screen;
        // Every state a screen has a context for.
        for variant in 0..4 {
            match variant {
                1 => app.dashboard.classic = false,
                2 => app.mission.pane = crate::mission::Pane::Wall,
                3 => app.knowledge_screen.contracts_tab = true,
                _ => {}
            }
            for s in keymap.shadows(&app.contexts()) {
                let line = format!(
                    "{} {}: {} over {} ({})",
                    s.winner.0, s.winner.2, s.winner.1, s.hidden.1, s.hidden.0
                );
                if !seen.contains(&line) {
                    seen.push(line);
                }
            }
        }
    }
    seen.sort();
    let decide: Vec<&String> = seen.iter().filter(|l| l.starts_with("decide ")).collect();
    let rest: Vec<&String> = seen.iter().filter(|l| !l.starts_with("decide ")).collect();
    assert_eq!(
        rest,
        [
            "board.ask enter: board.answer over list.open (mission)",
            "board.ask r: board.answer over mission.rule_take (mission)",
            "focus.reading t: focus.open_terminal over look.next_theme (global)",
            "knowledge t: knowledge.toggle_contracts over look.next_theme (global)",
        ]
        .iter()
        .collect::<Vec<_>>(),
        "screen keys over global ones"
    );
    // The decision keys cover screen keys: y/n/a/c are answers first.
    for line in decide {
        assert!(
            line.starts_with("decide y:")
                || line.starts_with("decide n:")
                || line.starts_with("decide a:")
                || line.starts_with("decide c:"),
            "{line}"
        );
    }
}

fn event(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

#[test]
fn terminal_events_become_canonical_keys() {
    let spec = |code, mods| spec_of(event(code, mods));
    assert_eq!(
        spec(KeyCode::Char('q'), KeyModifiers::NONE),
        Some(KeySpec::char('q'))
    );
    assert_eq!(
        spec(KeyCode::Char('D'), KeyModifiers::SHIFT),
        Some(KeySpec::char('D')),
        "an upper-case letter with the shift flag is the letter"
    );
    assert_eq!(
        spec(KeyCode::Char('D'), KeyModifiers::NONE),
        Some(KeySpec::char('D'))
    );
    assert_eq!(
        spec(KeyCode::Char('d'), KeyModifiers::SHIFT),
        Some(KeySpec::char('D')),
        "terminals that report the shift flag but not the case"
    );
    assert_eq!(
        spec(KeyCode::Char('s'), KeyModifiers::CONTROL),
        Some(KeySpec::ctrl('s'))
    );
    assert_eq!(
        spec(KeyCode::BackTab, KeyModifiers::SHIFT),
        Some(KeySpec::plain(Key::BackTab))
    );
    assert_eq!(
        spec(KeyCode::Tab, KeyModifiers::SHIFT),
        Some(KeySpec::plain(Key::BackTab))
    );
    assert_eq!(
        spec(KeyCode::Up, KeyModifiers::NONE),
        Some(KeySpec::plain(Key::Up))
    );
    assert_eq!(
        spec(KeyCode::F(5), KeyModifiers::NONE),
        Some(KeySpec::plain(Key::F(5)))
    );
    assert_eq!(
        spec(KeyCode::Char('x'), KeyModifiers::ALT),
        Some(KeySpec::new(Key::Char('x'), Mods::ALT))
    );
    assert_eq!(spec(KeyCode::Null, KeyModifiers::NONE), None);
    assert_eq!(spec(KeyCode::CapsLock, KeyModifiers::NONE), None);
}

#[test]
fn altgr_characters_are_the_character_not_a_modifier_chord() {
    // Windows reports AltGr as Ctrl+Alt: `[` on a German layout.
    assert_eq!(
        spec_of(event(
            KeyCode::Char('['),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )),
        Some(KeySpec::char('['))
    );
}
