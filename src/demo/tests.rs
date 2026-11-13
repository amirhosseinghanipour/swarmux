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

/// The console read model over the demo's own services: the same engine,
/// gate, Guard, ledger and stored streams the app runs on, played headless.
mod console {
    use swarmux_application::console::{
        ConsoleSnapshot, DecisionSource, GateVerdict, GroupKind, HoldState, Lifecycle, OutcomeView,
        PriorityKind, RiskTier,
    };

    use super::{Options, TICKS, build, tick};

    const WEB_WORDS: &str = "Refused. I will push the branch with the test as it was.";
    const DENIED_WORDS: &str = "The retry test is in the way. I will edit its expectations.";

    fn played() -> ConsoleSnapshot {
        let mut app = build(&Options::default()).unwrap();
        for _ in 0..TICKS {
            tick(&mut app);
        }
        app.console_snapshot().clone()
    }

    fn group(snap: &ConsoleSnapshot, kind: GroupKind) -> &[OutcomeView] {
        &snap
            .groups
            .iter()
            .find(|g| g.kind == kind)
            .unwrap()
            .outcomes
    }

    #[test]
    fn the_demo_outcomes_are_in_the_right_groups() {
        let snap = played();
        // The held push comes first (P1), then the run of routine asks (P2).
        let decide: Vec<(&str, Option<PriorityKind>)> = group(&snap, GroupKind::Decide)
            .iter()
            .map(|o| (o.title.as_str(), o.priority))
            .collect();
        assert_eq!(
            decide,
            [
                (
                    "Add retry with backoff to the payment form submit",
                    Some(PriorityKind::P1)
                ),
                (
                    "Regenerate the API reference and fix the broken links",
                    Some(PriorityKind::P2)
                ),
            ]
        );
        assert!(group(&snap, GroupKind::ReadyToLand).is_empty());
        assert!(group(&snap, GroupKind::Running).is_empty());

        // The mission the gate sent back once is delivered, with proof.
        let landed = group(&snap, GroupKind::Landed);
        assert_eq!(landed.len(), 1);
        let m = &landed[0];
        assert_eq!(m.title, "Fix invoice rounding");
        assert_eq!(m.lifecycle, Lifecycle::Delivered);
        assert_eq!(
            m.claim.as_ref().unwrap().text,
            "Invoice totals round half up; the suite passes."
        );
        let gate = m.checks.gate.unwrap();
        assert_eq!((gate.verdict, gate.blocks), (GateVerdict::Pass, 1));
        // The gate ran in this session, so its own record is the source.
        assert_eq!(
            m.checks.source,
            swarmux_application::console::ChecksSource::Gate
        );
        assert!(m.checks.total >= 1 && m.checks.failed == 0);
        let e = m.evidence.as_ref().expect("a sealed bundle");
        assert!(e.seq_to > e.seq_from, "the ledger range tells a story");
        assert_eq!(e.bundle_hash.len(), 64);
        assert!(m.diff.is_some());
        assert_eq!(m.spend.total_usd, 1.84);

        // The two jobs nobody asks about are on no queue.
        let c = snap.header.counts;
        assert_eq!(
            (c.decide, c.ready_to_land, c.running, c.landed, c.idle),
            (2, 0, 0, 1, 2)
        );
    }

    #[test]
    fn the_held_push_shows_its_countdown_and_the_agents_own_words() {
        let snap = played();
        let held: Vec<_> = snap
            .holds
            .iter()
            .filter(|h| h.state == HoldState::Held)
            .collect();
        // The push, and the run of routine checks waiting as one row.
        assert_eq!(held.len(), 13);
        let push = held[0];
        assert_eq!(push.target, "git push origin HEAD:main");
        assert_eq!(push.tool, "Bash");
        assert_eq!(push.priority, Some(PriorityKind::P1));
        assert!(push.destructive);
        assert_eq!(push.risk.unwrap().tier, RiskTier::Destructive);
        assert_eq!(push.decided_by, DecisionSource::Guard);
        // No rule matched a push to main: the Guard asked by default, and
        // the decision log names no rule. Nothing is made up.
        assert_eq!(push.rule, None);
        assert_eq!(push.knowledge_rule, None);
        // The countdown is absolute and still ahead.
        assert!(push.deadline_at.unwrap() > snap.taken_at);
        assert!(push.at < snap.taken_at);
        // The words are quoted from the stored stream, before the call.
        let words = push.agent_words.as_ref().expect("the agent said something");
        assert_eq!(words.text, WEB_WORDS);
        assert!(words.at <= push.at);
        assert!(words.label.contains("agent's own words"));
        // The last steps are the job's, as the action stream shows them.
        assert!(!push.steps.is_empty());
        assert!(push.steps.iter().all(|s| s.at <= push.at));
        let last = push.steps.last().unwrap();
        assert_eq!(last.text, WEB_WORDS);
        // It belongs to the held job's outcome, which is the first decision.
        assert_eq!(
            push.outcome.as_deref(),
            Some(group(&snap, GroupKind::Decide)[0].id.as_str())
        );
        assert_eq!(group(&snap, GroupKind::Decide)[0].holds[0], push.id);
        // The routine checks are P2 and not destructive.
        assert!(held[1..].iter().all(|h| !h.destructive));
        assert!(
            held[1..]
                .iter()
                .all(|h| h.priority == Some(PriorityKind::P2))
        );
    }

    #[test]
    fn the_protected_test_denial_is_listed_with_its_rule_and_words() {
        let snap = played();
        let denied = snap
            .holds
            .iter()
            .find(|h| h.state == HoldState::Denied && h.target == "tests/checkout_oracle.rs")
            .expect("the Guard refused the edit of the protected test");
        assert_eq!(denied.tool, "Edit");
        assert!(
            denied
                .rule
                .as_deref()
                .is_some_and(|r| r.contains("tests/**")),
            "{:?}",
            denied.rule
        );
        assert_eq!(denied.deadline_at, None);
        assert_eq!(denied.agent_words.as_ref().unwrap().text, DENIED_WORDS);
        // The refusal is among the steps the agent took, as the stream saw it.
        assert!(denied.steps.iter().any(|s| s.text == DENIED_WORDS));
        // Held calls first, then refusals.
        let first_denied = snap
            .holds
            .iter()
            .position(|h| h.state == HoldState::Denied)
            .unwrap();
        assert!(
            snap.holds[..first_denied]
                .iter()
                .all(|h| h.state == HoldState::Held)
        );
    }

    #[test]
    fn the_demo_deck_has_a_card_per_job_and_header_figures() {
        let snap = played();
        // Four jobs and the mission: five cards. The demo runs no plan, so
        // it has no task graph, no contracts and no board claims: no edges.
        assert_eq!(snap.deck.nodes.len(), 5);
        assert!(snap.deck.edges.is_empty());
        assert_eq!(snap.deck.max_depth, 0);
        let on_queue = snap.deck.nodes.iter().filter(|n| n.group.is_some()).count();
        assert_eq!(on_queue, 3, "two decisions and the landed mission");
        // The usage windows the demo's jobs reported.
        let five = snap
            .header
            .limits
            .iter()
            .find(|l| l.window == "five_hour")
            .expect("the five-hour window");
        assert!(five.utilization.is_some_and(|u| u > 0.5));
        assert_eq!(snap.header.queue.max_concurrent, 5);
    }

    #[test]
    fn two_demo_runs_build_the_same_snapshot() {
        let (a, b) = (played(), played());
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
}
