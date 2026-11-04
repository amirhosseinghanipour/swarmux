//! The demo played without a terminal: the same ticks, the same clock and
//! the same seed must draw the same screens, and the key moments of the
//! script must show up in them.

use swarmux_tui::App;

use super::{LABEL, Options, build};

const WIDTH: u16 = 160;
const HEIGHT: u16 = 48;
/// Ticks of the whole scenario, plus a little for the last job to settle.
const TICKS: u32 = 340;

/// One update and one frame, as the run loop does them.
fn tick(app: &mut App) {
    app.update();
    app.frame += 1;
}

#[test]
fn two_runs_draw_the_same_screens() {
    let mut a = build(&Options::default()).unwrap();
    let mut b = build(&Options::default()).unwrap();
    for n in 0..TICKS {
        tick(&mut a);
        tick(&mut b);
        if n % 5 == 0 || n > TICKS - 3 {
            assert_eq!(
                a.render_buffer(WIDTH, HEIGHT),
                b.render_buffer(WIDTH, HEIGHT),
                "the screens differ at tick {n}"
            );
        }
    }
    // The other screens too, once everything has happened.
    for key in ['5', '2', '4', '6', '1'] {
        a.press_char(key);
        b.press_char(key);
        tick(&mut a);
        tick(&mut b);
        assert_eq!(
            a.render_buffer(WIDTH, HEIGHT),
            b.render_buffer(WIDTH, HEIGHT),
            "screen {key} differs"
        );
    }
}

#[test]
fn a_different_seed_changes_the_made_up_numbers() {
    let mut a = build(&Options::default()).unwrap();
    let mut b = build(&Options {
        seed: 8,
        ..Options::default()
    })
    .unwrap();
    for _ in 0..120 {
        tick(&mut a);
        tick(&mut b);
    }
    assert_ne!(
        a.render_buffer(WIDTH, HEIGHT),
        b.render_buffer(WIDTH, HEIGHT)
    );
}

/// The first tick at which `needle` is on screen, from the start.
struct Watch<'a> {
    needle: &'a str,
    seen: Option<u32>,
}

#[test]
fn the_key_moments_appear_in_order() {
    let mut app = build(&Options::default()).unwrap();
    let mut guard_deny = Watch {
        needle: "deny Edit tests/checkout_oracle.rs",
        seen: None,
    };
    let mut p1_alarm = Watch {
        needle: "P1 ◆ approval W web-checkout Bash git push",
        seen: None,
    };
    let mut flood = Watch {
        needle: "flood: 13 items from 2 causes",
        seen: None,
    };
    let mut blocked = Watch {
        needle: "gate blocked a stop (1 of",
        seen: None,
    };
    let mut passed = Watch {
        needle: "gate passed",
        seen: None,
    };
    for n in 0..TICKS {
        tick(&mut app);
        let screen = app.render_text(WIDTH, HEIGHT);
        assert!(
            screen.contains(LABEL),
            "the demo tag is missing at tick {n}"
        );
        for w in [
            &mut guard_deny,
            &mut p1_alarm,
            &mut flood,
            &mut blocked,
            &mut passed,
        ] {
            if w.seen.is_none() && screen.contains(w.needle) {
                w.seen = Some(n);
            }
        }
    }
    let at = |w: &Watch| {
        w.seen
            .unwrap_or_else(|| panic!("never showed: {}", w.needle))
    };
    assert!(
        at(&guard_deny) < at(&blocked),
        "the Guard refuses the edit before the gate sends the mission back"
    );
    assert!(at(&p1_alarm) < at(&blocked));
    assert!(at(&flood) < at(&blocked));
    assert!(at(&blocked) < at(&passed), "blocked once, then passed");

    // The delivered mission and its evidence are on the Ledger screen.
    app.press_char('5');
    tick(&mut app);
    let ledger = app.render_text(WIDTH, HEIGHT);
    assert!(
        ledger.contains("delivered Fix invoice rounding"),
        "{ledger}"
    );
    assert!(ledger.contains("evidence bundle"), "{ledger}");
    assert!(ledger.contains("1 block(s), then pass"), "{ledger}");
    assert!(ledger.contains(LABEL));
}
