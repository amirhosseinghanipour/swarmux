//! The footer under the default keymap reads as it always did, and fits
//! the room it has without cutting a hint in half.

use super::*;
use crate::app::tests_support::test_app;
use crate::screens::Screen;

/// Every footer as it was written before the hints came from the keymap.
const TODAYS_FOOTERS: &[(&str, &str)] = &[
    (
        "board",
        " ↑↓ select · r answer a question · x close it · 0-9 screens · ? help · q quit",
    ),
    (
        "followups",
        " ↑↓ select · a approve · d decline · l later · o open job · m draft mission · 0-9 screens · ? help · q quit",
    ),
    (
        "knowledge",
        " ↑↓ select · a add (kind: text) · e edit · p promote · c confirm · x retire · 0-9 screens · ? help · q quit",
    ),
    (
        "ledger",
        " / search · o filter · v verify · l labels · r re-run · f fork · enter view · ↑↓ select · ? help · q quit",
    ),
    (
        "mission",
        " ↑↓ select · ⏎ open · y/n/a answer · A batch · s shelve · F focus · tab wall · p prompt · ? help · q quit",
    ),
    (
        "missions",
        " n new · e edit · l lint · f protect · p plan first · r run · y/n/c plan · ⏎ focus · ? help · q quit",
    ),
    (
        "prompts",
        " n new · e edit · x delete · d dispatch · D route · ↑↓ select · 0-9 screens · ? help · q quit",
    ),
    (
        "queue",
        " c cancel · r retry · +/- priority · enter view · ↑↓ select · ? help · q quit",
    ),
    (
        "sessions",
        " n new session · w on a workspace · l lean on/off · ↑↓ select · 0-9 screens · ? help · q quit",
    ),
];

fn screen(id: &str) -> Screen {
    crate::screens::all()
        .find(|s| s.id() == id)
        .unwrap_or_else(|| panic!("no screen {id}"))
}

#[test]
fn the_default_footers_read_as_they_always_did() {
    for (id, want) in TODAYS_FOOTERS {
        let mut t = test_app();
        t.app.screen = screen(id);
        assert_eq!(full(&t.app), *want, "{id}");
    }
}

#[test]
fn focus_has_two_footers_and_the_forms_have_their_own() {
    let mut t = test_app();
    t.app.screen = Screen::Dashboard;
    t.app.dashboard.classic = true;
    assert_eq!(
        full(&t.app),
        " p prompt · P route · v review · ↑↓ session · t theme · L layout · tab focus · ? help · q quit"
    );
    t.app.dashboard.classic = false;
    assert_eq!(
        full(&t.app),
        " ↑↓ scroll · d diff · t open session · p prompt · v review · D classic · ? help · q quit"
    );
    t.app.screen = Screen::Prompts;
    t.app.prompts.form = Some(crate::prompts::PromptForm::blank());
    assert_eq!(full(&t.app), " tab next field · ctrl+s save · esc cancel");
}

#[test]
fn every_footer_ends_with_a_way_out() {
    for screen in crate::screens::all() {
        let mut t = test_app();
        t.app.screen = screen;
        let line = full(&t.app);
        assert!(line.ends_with("q quit"), "{}: {line}", screen.title());
    }
}

#[test]
fn the_background_service_says_detach() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    t.app.set_service(true);
    assert!(full(&t.app).ends_with("q detach"), "{}", full(&t.app));
    assert!(!full(&t.app).contains("quit"));
}

#[test]
fn a_footer_that_fits_is_the_whole_footer() {
    let mut t = test_app();
    t.app.screen = Screen::Queue;
    let all = full(&t.app);
    assert_eq!(fit(&t.app, cells(&all)), all);
    assert_eq!(fit(&t.app, 500), all);
}

#[test]
fn a_footer_that_is_too_long_drops_whole_hints_and_keeps_the_way_out() {
    let mut t = test_app();
    t.app.screen = Screen::Ledger;
    let all = full(&t.app);
    let pieces: Vec<&str> = all.trim_start().split(SEP).collect();
    for width in 0..cells(&all) {
        let line = fit(&t.app, width);
        assert!(cells(&line) <= width, "{width}: {line:?}");
        if line.is_empty() {
            continue;
        }
        // Whole hints only, in order, ending with the way out.
        let kept: Vec<&str> = line.trim_start().split(SEP).collect();
        assert!(kept.iter().all(|k| pieces.contains(k)), "{width}: {line:?}");
        assert_eq!(kept.last(), pieces.last(), "{width}: {line:?}");
        let mut from = 0;
        for k in &kept {
            let at = pieces[from..].iter().position(|p| p == k).unwrap();
            from += at + 1;
        }
    }
    // The first hints stay as long as any of the others do, and the help
    // and the way out are the last to go.
    let tight = fit(&t.app, 40);
    assert_eq!(tight, " / search · o filter · ? help · q quit");
    assert_eq!(fit(&t.app, 27), " / search · ? help · q quit");
    assert_eq!(fit(&t.app, 16), " ? help · q quit");
    // Too narrow for the help: the way out alone, then nothing.
    assert_eq!(fit(&t.app, 15), " q quit");
    assert_eq!(fit(&t.app, 8), " q quit");
    assert_eq!(fit(&t.app, 3), "");
}

#[test]
fn the_footer_never_runs_into_the_status_area() {
    use crate::ui::snapshot::Snapshot;
    for screen in crate::screens::all() {
        // Focus draws its own status line.
        if screen
            .def()
            .status(&test_app().app, &test_app().app.look.theme)
            .is_some()
        {
            continue;
        }
        let mut t = test_app();
        t.app.screen = screen;
        let line = Snapshot::of(&t.app, 100, 24).status();
        // No hint is cut: what stands before the stats is whole hints.
        let stats = line.find("running").unwrap_or(line.len());
        let footer = line[..stats].trim_end();
        if footer.is_empty() {
            continue;
        }
        let all = full(&t.app);
        let pieces: Vec<&str> = all.trim_start().split(SEP).collect();
        let kept: Vec<&str> = footer.trim_start().split(SEP).collect();
        assert!(
            kept.iter().all(|k| pieces.contains(k)),
            "{}: {footer:?}",
            screen.title()
        );
    }
}

#[test]
fn every_footer_has_the_help_hint_just_before_the_way_out() {
    for screen in crate::screens::all() {
        let mut t = test_app();
        t.app.screen = screen;
        let line = full(&t.app);
        assert!(
            line.ends_with(" · ? help · q quit"),
            "{}: {line}",
            screen.title()
        );
    }
}

#[test]
fn a_rebound_help_key_shows_and_an_unbound_one_leaves_the_footer() {
    use crate::keys::config_of;
    let mut t = test_app();
    t.app = t
        .app
        .with_key_config(&config_of(&[("global", "help.toggle", &["f1"])]));
    t.app.screen = Screen::Queue;
    assert!(
        full(&t.app).ends_with(" · f1 help · q quit"),
        "{}",
        full(&t.app)
    );
    let mut t = test_app();
    t.app = t
        .app
        .with_key_config(&config_of(&[("global", "help.toggle", &[])]));
    t.app.screen = Screen::Queue;
    let line = full(&t.app);
    assert!(!line.contains("help"), "{line}");
    assert!(line.ends_with(" · q quit"), "{line}");
}

#[test]
fn the_help_hint_outlives_the_other_hints_in_a_narrow_footer() {
    for screen in crate::screens::all() {
        let mut t = test_app();
        t.app.screen = screen;
        // Wide enough for the pinned two and little else.
        let line = fit(&t.app, 24);
        assert!(
            line.ends_with("? help · q quit"),
            "{}: {line:?}",
            screen.title()
        );
    }
}
