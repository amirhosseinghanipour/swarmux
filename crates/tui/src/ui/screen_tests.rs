//! Every screen the registry knows, drawn at the sizes terminals come in
//! and checked against the layout they share: the header, the tab bar with
//! the active screen filled, a body that draws something, and the status
//! bar with the screen's own hints. The pattern for a new screen's layout
//! test: register it, and it is in this matrix.

use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::snapshot::{BOTTOM_ROWS, Snapshot, TOP_ROWS};
use crate::app::tests_support::{TestApp, test_app};
use crate::keys::nav_label;
use crate::screens::{self, Screen};

/// Common terminal sizes: a small window up to a wide one.
const SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 36), (160, 48)];

fn on(screen: Screen) -> TestApp {
    let mut t = test_app();
    t.app.screen = screen;
    t
}

/// The tab label of `screen`, as the tab bar writes it.
fn label(t: &TestApp, screen: Screen) -> String {
    format!(" {} {} ", nav_label(&t.app.keymap, screen), screen.title())
}

#[test]
fn every_screen_draws_the_shared_layout_at_every_size() {
    for screen in screens::all() {
        let t = on(screen);
        let def = screen.def();
        for (w, h) in SIZES {
            let s = Snapshot::of(&t.app, w, h);
            let at = format!("{} at {w}x{h}", screen.title());
            assert_eq!((s.width(), s.height()), (w, h), "{at}");
            assert_eq!(s.body().height, h - TOP_ROWS - BOTTOM_ROWS, "{at}");

            // The header: the product's name, unless the screen has its own.
            if def.header(&t.app, &t.app.look.theme).is_none() {
                assert!(s.header().contains("SWARMUX"), "{at}: {}", s.header());
            } else {
                assert!(!s.header().trim().is_empty(), "{at}: a header of its own");
            }

            // The tab bar lists the screens in registry order; every tab
            // that fits is there, and from 120 columns all of them do.
            let mut last = None;
            let mut found = 0;
            for other in screens::all() {
                let Some((x, y)) = s.find_in(Rect::new(0, 1, w, 1), &label(&t, other)) else {
                    continue;
                };
                assert_eq!(y, 1, "{at}: tabs are on the second row");
                assert!(last.is_none_or(|l| x > l), "{at}: tabs are in order");
                last = Some(x);
                found += 1;
            }
            assert!(found >= 6, "{at}: only {found} tabs show: {}", s.tabs());
            if w >= 120 {
                assert_eq!(found, 10, "{at}: {}", s.tabs());
            }

            // The active tab is the filled one.
            if let Some((x, y)) = s.find_in(Rect::new(0, 1, w, 1), &label(&t, screen)) {
                let chip = s.look(x + 1, y);
                assert_eq!(chip.bg, t.app.look.theme.lead, "{at}: the active tab");
                assert!(chip.modifier.contains(Modifier::BOLD), "{at}");
                let other = screens::all().find(|o| *o != screen).unwrap();
                if let Some((ox, oy)) = s.find_in(Rect::new(0, 1, w, 1), &label(&t, other)) {
                    assert_ne!(s.look(ox + 1, oy).bg, t.app.look.theme.lead, "{at}");
                }
            }

            // The body draws something.
            assert!(s.has_content(s.body()), "{at}: an empty body");

            // The status bar: the screen's hints, or its own line.
            if w >= 100 && def.status(&t.app, &t.app.look.theme).is_none() {
                let hints = def.hints(&t.app).trim();
                let start: String = hints.chars().take(24).collect();
                assert!(s.status().contains(&start), "{at}: {}", s.status());
            }
        }
    }
}

#[test]
fn drawing_twice_gives_the_same_frame() {
    for screen in screens::all() {
        let t = on(screen);
        let a = Snapshot::of(&t.app, 120, 36);
        let b = Snapshot::of(&t.app, 120, 36);
        assert_eq!(a.text(), b.text(), "{}", screen.title());
    }
}

#[test]
fn the_active_tab_follows_the_user() {
    let mut t = test_app();
    let filled = |t: &TestApp, screen: Screen| {
        let s = Snapshot::of(&t.app, 140, 12);
        let (x, y) = s
            .find_in(Rect::new(0, 1, 140, 1), &label(t, screen))
            .expect("the tab shows");
        s.look(x + 1, y).bg == t.app.look.theme.lead
    };
    assert!(filled(&t, Screen::Mission));
    t.keys([crossterm::event::KeyCode::Char('4')]);
    assert!(filled(&t, Screen::Queue));
    assert!(!filled(&t, Screen::Mission));
    t.keys([crossterm::event::KeyCode::Esc]);
    assert!(filled(&t, Screen::Mission), "esc went back");
}

#[test]
fn a_half_typed_chord_shows_in_the_status_bar() {
    let mut t = test_app();
    t.keys([crossterm::event::KeyCode::Char('g')]);
    let s = Snapshot::of(&t.app, 120, 12);
    assert!(s.status().contains(" g "), "{}", s.status());
    assert!(s.status().contains("waiting for the next key"));
    t.keys([crossterm::event::KeyCode::Esc]);
    let s = Snapshot::of(&t.app, 120, 12);
    assert!(!s.status().contains("waiting for the next key"));
}

#[test]
fn the_snapshot_answers_layout_questions() {
    let t = on(Screen::Queue);
    let s = Snapshot::of(&t.app, 100, 20);
    // Where a word starts, by cell.
    let (x, y) = s.find(" 4 queue ").expect("the queue tab");
    assert_eq!(y, 1);
    assert!(x > 0);
    assert_eq!(s.find_in(s.body(), " 4 queue "), None, "not in the body");
    // Rows and regions.
    assert_eq!(s.row(1), s.tabs());
    assert_eq!(s.region(Rect::new(0, 0, 100, 1)), s.header());
    assert!(s.text().lines().count() <= 20);
    assert!(s.contains("SWARMUX"));
    assert!(!s.contains("no such text on this screen"));
    // A region outside the frame is clipped, not a panic.
    assert_eq!(s.region(Rect::new(200, 200, 5, 5)), "");
}
