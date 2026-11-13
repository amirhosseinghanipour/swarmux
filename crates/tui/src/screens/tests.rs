//! The registry and the back stack, end to end through key presses.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::keys::{Action, Chord, Context, GLOBAL, actions};

use super::*;
use crate::app::Mode;
use crate::app::tests_support::{TestApp, test_app};

fn chars(t: &mut TestApp, text: &str) {
    t.keys(text.chars().map(KeyCode::Char));
}

fn esc(t: &mut TestApp) {
    t.keys([KeyCode::Esc]);
}

#[test]
fn every_screen_is_registered_once_with_its_own_id_title_and_nav_action() {
    let mut ids: Vec<_> = all().map(Screen::id).collect();
    let mut titles: Vec<_> = all().map(Screen::title).collect();
    let mut navs: Vec<_> = all().map(|s| s.def().nav_action()).collect();
    assert_eq!(ids.len(), 11);
    ids.sort_unstable();
    ids.dedup();
    titles.sort_unstable();
    titles.dedup();
    navs.sort();
    navs.dedup();
    assert_eq!(ids.len(), 11, "ids differ");
    assert_eq!(titles.len(), 11, "titles differ");
    assert_eq!(navs.len(), 11, "nav actions differ");
    for screen in all() {
        assert_eq!(screen.def().id(), screen);
        assert_eq!(nav_target(screen.def().nav_action()), Some(screen));
    }
    assert_eq!(Screen::default(), Screen::Mission, "the home screen");
}

#[test]
fn every_screen_declares_bindings_for_the_contexts_it_uses() {
    let (keymap, _) = crate::keys::defaults().unwrap();
    let known: Vec<_> = keymap.contexts().collect();
    for screen in all() {
        let t = test_app();
        let mut app = t.app;
        app.screen = screen;
        for variant in 0..4 {
            match variant {
                1 => app.dashboard.classic = false,
                2 => app.mission.pane = crate::mission::Pane::Wall,
                3 => app.knowledge_screen.contracts_tab = true,
                _ => {}
            }
            for context in screen.def().contexts(&app) {
                assert!(
                    known.contains(&context),
                    "{} uses {context}, which has no bindings",
                    screen.title()
                );
            }
        }
    }
}

#[test]
fn the_planned_screens_stand_in_explicitly() {
    // (keys, name, where it goes, whether the user is told)
    let promised = [
        ("g d", "deck", Some(Screen::Mission)),
        ("g r", "record", Some(Screen::Ledger)),
    ];
    assert_eq!(PLANNED.len(), promised.len());
    for (keys, name, stand_in) in promised {
        let p = PLANNED
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is not planned"));
        assert_eq!(p.keys, &[keys]);
        assert_eq!(p.stand_in, stand_in, "{name}");
        assert!(
            nav_target(p.action).is_none(),
            "{name} is not a screen that exists"
        );
    }
}

fn goes(chord: &str) -> (Screen, Option<String>) {
    let mut t = test_app();
    t.app.screen = Screen::Sessions;
    for c in chord.chars().filter(|c| *c != ' ') {
        t.keys([KeyCode::Char(c)]);
    }
    (t.app.screen, t.app.notice.clone())
}

#[test]
fn g_then_a_letter_goes_to_the_screen_or_its_nearest() {
    // Screens that exist: no notice, it simply goes there.
    assert_eq!(goes("g w"), (Screen::Work, None));
    assert_eq!(goes("g m"), (Screen::Missions, None));
    assert_eq!(goes("g p"), (Screen::Prompts, None));
    assert_eq!(goes("g k"), (Screen::Knowledge, None));
    // Screens that are planned: it goes to the nearest and says so.
    assert_eq!(
        goes("g d"),
        (
            Screen::Mission,
            Some("deck is not built yet: showing mission".into())
        )
    );
    assert_eq!(
        goes("g r"),
        (
            Screen::Ledger,
            Some("record is not built yet: showing ledger".into())
        )
    );
}

#[test]
fn the_g_chord_waits_shows_and_can_be_dropped() {
    let mut t = test_app();
    chars(&mut t, "4");
    chars(&mut t, "g");
    assert_eq!(t.app.screen, Screen::Queue, "g alone goes nowhere");
    assert_eq!(t.app.resolver.pending().len(), 1);
    let out = crate::ui::render_to_string(&t.app, 100, 8);
    assert!(out.contains("waiting for the next key"), "{out}");
    esc(&mut t);
    assert!(t.app.resolver.pending().is_empty());
    assert_eq!(
        t.app.screen,
        Screen::Queue,
        "esc dropped the chord, no more"
    );
    assert_eq!(t.app.notice, None);
    // A letter that is no chord: nothing happens, and q does not quit.
    chars(&mut t, "gq");
    assert!(!t.app.quit, "the q belonged to the chord");
    assert_eq!(t.app.notice.as_deref(), Some("g q is not bound"));
    assert_eq!(t.app.screen, Screen::Queue);
    chars(&mut t, "q");
    assert!(t.app.quit, "q is q again");
}

#[test]
fn a_chord_nobody_finishes_goes_away_by_itself() {
    let mut t = test_app();
    chars(&mut t, "g");
    t.app.ui_ms += 1_000;
    t.app.update();
    assert_eq!(t.app.resolver.pending().len(), 1, "still waiting");
    t.app.ui_ms += 1_500;
    t.app.update();
    assert!(t.app.resolver.pending().is_empty());
    chars(&mut t, "w");
    assert_eq!(
        t.app.screen,
        Screen::Mission,
        "w is not a chord end any more"
    );
}

#[test]
fn the_chord_timer_does_not_follow_the_clock_the_work_runs_by() {
    // The demo moves its clock by minutes at a time; a chord still waits.
    let mut t = test_app();
    chars(&mut t, "g");
    t.clock.advance(10 * 60_000);
    t.app.update();
    assert_eq!(t.app.resolver.pending().len(), 1);
    chars(&mut t, "w");
    assert_eq!(t.app.screen, Screen::Work);
}

#[test]
fn a_chord_is_plain_text_while_typing() {
    let mut t = test_app();
    chars(&mut t, "6n");
    assert!(matches!(t.app.mode, Mode::Input(..)));
    chars(&mut t, "gw");
    assert!(matches!(&t.app.mode, Mode::Input(_, i) if i.text == "gw"));
    assert_eq!(t.app.screen, Screen::Sessions);
    assert!(t.app.resolver.pending().is_empty());
}

#[test]
fn esc_goes_back_through_the_screens_visited() {
    let mut t = test_app();
    chars(&mut t, "145");
    assert_eq!(t.app.screen, Screen::Ledger);
    esc(&mut t);
    assert_eq!(t.app.screen, Screen::Queue);
    esc(&mut t);
    assert_eq!(t.app.screen, Screen::Mission);
    esc(&mut t);
    assert_eq!(t.app.screen, Screen::Mission, "nothing left to go back to");
    assert!(!t.app.quit, "esc never quits");
}

#[test]
fn the_history_is_bounded() {
    let mut t = test_app();
    for _ in 0..40 {
        chars(&mut t, "41");
    }
    for _ in 0..crate::nav::DEPTH {
        esc(&mut t);
    }
    let at = t.app.screen;
    esc(&mut t);
    assert_eq!(t.app.screen, at, "the history ran out after its depth");
    assert!(!t.app.quit);
}

#[test]
fn a_jump_made_for_you_can_be_undone_with_esc() {
    // Following a job from the queue opens Focus; esc returns to the queue.
    let mut t = test_app();
    t.with_session("demo");
    t.send("hi");
    chars(&mut t, "4");
    t.keys([KeyCode::Enter]);
    assert_eq!(t.app.screen, Screen::Dashboard);
    esc(&mut t);
    assert_eq!(t.app.screen, Screen::Queue);
}

#[test]
fn esc_leaves_the_constellation_before_it_goes_back() {
    let mut t = test_app();
    chars(&mut t, "4");
    chars(&mut t, "1");
    chars(&mut t, "z");
    assert!(t.app.showcase.mode.is_some());
    esc(&mut t);
    assert!(t.app.showcase.mode.is_none());
    assert_eq!(t.app.screen, Screen::Mission, "the screen stayed");
    esc(&mut t);
    assert_eq!(t.app.screen, Screen::Queue, "now it goes back");
}

#[test]
fn the_tab_bar_shows_the_keys_in_use() {
    let t = test_app();
    let out = crate::ui::render_to_string(&t.app, 140, 6);
    for (key, title) in [
        ("1", "mission"),
        ("4", "queue"),
        ("0", "focus"),
        ("9", "follow-ups"),
    ] {
        assert!(out.contains(&format!(" {key} {title} ")), "{out}");
    }
    // A user who moves the queue to F4 sees it there, and F4 works.
    let (mut keymap, _) = crate::keys::defaults().unwrap();
    keymap
        .rebind(GLOBAL, actions::GO_QUEUE, vec![Chord::parse("f4").unwrap()])
        .unwrap();
    let mut t = test_app();
    t.app = t.app.with_keymap(keymap);
    let out = crate::ui::render_to_string(&t.app, 140, 6);
    assert!(out.contains(" f4 queue "), "{out}");
    assert!(!out.contains(" 4 queue "), "{out}");
    t.keys([KeyCode::Char('4')]);
    assert_eq!(t.app.screen, Screen::Mission, "4 is free now");
    t.keys([KeyCode::F(4)]);
    assert_eq!(t.app.screen, Screen::Queue);
}

#[test]
fn a_rebound_key_does_what_the_old_one_did_and_the_old_one_does_nothing() {
    let (mut keymap, _) = crate::keys::defaults().unwrap();
    let queue = Context::new("queue");
    keymap
        .rebind(
            queue,
            Action::new("queue.cancel"),
            vec![Chord::parse("ctrl+x").unwrap()],
        )
        .unwrap();
    let mut t = test_app();
    t.app = t.app.with_keymap(keymap);
    t.with_session("demo");
    t.send("hi");
    t.keys([KeyCode::Char('4')]);
    t.keys([KeyCode::Char('c')]);
    assert!(
        t.app
            .engine
            .jobs
            .queue()
            .iter()
            .all(|j| j.status != swarmux_domain::JobStatus::Cancelled),
        "c no longer cancels"
    );
    t.app
        .on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(
        t.app
            .engine
            .jobs
            .queue()
            .iter()
            .any(|j| j.status == swarmux_domain::JobStatus::Cancelled),
        "ctrl+x cancels the job"
    );
}

#[test]
fn gw_can_be_rebound_to_w() {
    let (mut keymap, _) = crate::keys::defaults().unwrap();
    keymap
        .rebind(GLOBAL, actions::GO_WORK, vec![Chord::parse("w").unwrap()])
        .unwrap();
    let mut t = test_app();
    t.app = t.app.with_keymap(keymap);
    chars(&mut t, "w");
    assert_eq!(t.app.screen, Screen::Work);
    assert_eq!(t.app.notice, None, "work is built: no stand-in notice");
}

#[test]
fn a_decision_overlay_takes_its_keys_before_the_screen() {
    // `a` adds knowledge on the Knowledge screen, and always-allows a held
    // call: with a call held, the call wins.
    let mut t = test_app();
    t.with_session("demo");
    t.send("hi");
    let job = t.runner.request(0).job_id;
    t.inbox.push(Some(job), "Bash", "git push origin main");
    t.app.update();
    assert!(t.app.engine.guard.pending_count() > 0);
    chars(&mut t, "n");
    assert_eq!(t.app.engine.guard.pending_count(), 0, "n denied it");
    // Nothing is asking: `n` is the screen's again.
    chars(&mut t, "6n");
    assert!(matches!(&t.app.mode, Mode::Input(..)), "n starts a session");
}

#[test]
fn the_screens_that_edit_text_take_every_key_as_typed() {
    // The Prompts form: q, 1 and g are letters here.
    let mut t = test_app();
    chars(&mut t, "3n");
    assert!(t.app.prompts.form.is_some());
    chars(&mut t, "q1gw");
    assert!(!t.app.quit);
    assert_eq!(t.app.screen, Screen::Prompts);
    assert_eq!(t.app.prompts.form.as_ref().unwrap().name.text, "q1gw");
    esc(&mut t);
    assert!(t.app.prompts.form.is_none());
    assert_eq!(t.app.screen, Screen::Prompts, "esc closed the form only");
}

#[test]
fn the_declared_context_stacks_are_every_stack_a_screen_has() {
    // `context_stacks` is what the config's overlap warnings look at; it
    // must be every stack `contexts` gives in any state of the screen.
    for screen in all() {
        let t = test_app();
        let mut app = t.app;
        app.screen = screen;
        let declared = screen.def().context_stacks();
        for variant in 0..4 {
            match variant {
                1 => app.dashboard.classic = false,
                2 => app.mission.pane = crate::mission::Pane::Wall,
                3 => app.knowledge_screen.contracts_tab = true,
                _ => {}
            }
            let now = screen.def().contexts(&app);
            assert!(
                declared.contains(&now),
                "{} in state {variant} has {now:?}, declared {declared:?}",
                screen.title()
            );
        }
    }
}
