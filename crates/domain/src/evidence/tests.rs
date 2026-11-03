use super::*;
use crate::JobId;

fn check(name: &str, passed: bool) -> CheckResult {
    CheckResult {
        name: name.into(),
        program: "cargo".into(),
        args: vec!["test".into(), "-p".into(), "a b".into()],
        timeout_secs: 600,
        exit: Some(if passed { 0 } else { -1 }),
        passed,
        flaky: false,
        duration_ms: 1500,
        first_failure: if passed {
            String::new()
        } else {
            "assertion `left == right` failed\n  left: 1\n right: 2".into()
        },
    }
}

fn bundle() -> Bundle {
    Bundle {
        schema: BUNDLE_SCHEMA,
        contract: ContractRef {
            slug: "fast-search".into(),
            title: "Fast \"search\"".into(),
            goal: "make search answer\nin under 50 ms".into(),
            root: "C:\\work\\repo".into(),
            checks: vec!["cargo test -p search".into()],
            protected: vec!["tests/**".into()],
            paths_rw: vec!["src/**".into()],
            budget_usd: Some(2.5),
            wall_minutes: None,
            human_signoff: false,
        },
        job: 7,
        attempt: 1,
        session: Some("s-1".into()),
        repo: "C:\\work\\repo".into(),
        branch: Some("swarmux/fast-search/7".into()),
        base_sha: Some("aaaa".into()),
        head_sha: "bbbb".into(),
        dirty: false,
        diff: DiffSummary {
            files: 3,
            insertions: 40,
            deletions: 5,
            hash: "dd".into(),
        },
        checks: vec![check("test", true)],
        hidden: None,
        gates: vec![
            GateRef {
                seq: 12,
                outcome: "block".into(),
                event: "stop".into(),
                blocks: 0,
                digest: "01".into(),
            },
            GateRef {
                seq: 15,
                outcome: "pass".into(),
                event: "stop".into(),
                blocks: 1,
                digest: "02".into(),
            },
        ],
        review: Some(ReviewRef {
            verdict: "pass".into(),
            concerns: 0,
            job: 8,
        }),
        checkpoints: vec![CheckpointRef {
            commit: "bbbb".into(),
            reason: "gate_pass".into(),
            seq: Some(16),
        }],
        cost: Cost {
            total_usd: 1.25,
            basis: "reported".into(),
            plan: 0.01,
            implement: 1.0,
            verify: 0.0,
            review: 0.24,
            by_model: vec![("sonnet".into(), 1.0), ("haiku".into(), 0.25)],
        },
        wall_ms: 61_000,
        ledger: LedgerRange {
            from_seq: 4,
            to_seq: 20,
            head_hash: "ff".into(),
        },
        sealed_at: 1_000,
        signature: None,
    }
}

#[test]
fn a_bundle_round_trips_through_json() {
    let mut b = bundle();
    b.signature = Some(Signature {
        alg: "sha256".into(),
        value: b.digest_hex(),
    });
    let text = b.to_json();
    let back = Bundle::from_json(&text).unwrap();
    assert_eq!(back, b, "{text}");
    assert_eq!(back.digest(), b.digest());
    assert_eq!(back.signature_ok(), Ok(true));
}

#[test]
fn editing_a_bundle_breaks_its_signature() {
    let mut b = bundle();
    b.signature = Some(Signature {
        alg: "sha256".into(),
        value: b.digest_hex(),
    });
    let text = b.to_json().replace("\"insertions\":40", "\"insertions\":4");
    let edited = Bundle::from_json(&text).unwrap();
    assert_eq!(edited.signature_ok(), Ok(false));
    assert_eq!(bundle().signature_ok(), Err("not signed".into()));
}

#[test]
fn bad_bundles_say_what_is_wrong() {
    assert!(Bundle::from_json("{").is_err());
    let text = bundle().to_json().replace("\"schema\":1", "\"schema\":9");
    assert!(Bundle::from_json(&text).unwrap_err().contains("schema 9"));
}

#[test]
fn tallies() {
    let mut b = bundle();
    b.checks.push(check("lint", false));
    assert_eq!(b.check_tally(), (1, 2));
    assert_eq!(b.blocks(), 1);
    assert_eq!(b.job_id(), JobId(7));
    assert_eq!(b.checks[0].command_line(), "cargo test -p \"a b\"");
}

fn rerun(name: &str, exit: Option<i32>, passed: bool) -> Rerun {
    Rerun {
        name: name.into(),
        exit,
        passed,
        problem: None,
    }
}

#[test]
fn checks_match_when_they_end_the_same_way() {
    let b = bundle();
    let items = compare_checks(&b, &[rerun("test", Some(0), true)]);
    assert!(items.iter().all(|i| i.ok), "{items:?}");
    let items = compare_checks(&b, &[rerun("test", Some(101), false)]);
    assert!(!items[0].ok);
    assert_eq!(items[0].expected, "pass");
    assert_eq!(items[0].found, "fail (exit 101)");
    let items = compare_checks(&b, &[]);
    assert_eq!(items[0].found, "not run");
    let mut broken = rerun("test", None, false);
    broken.problem = Some("could not start: no such file".into());
    assert!(!compare_checks(&b, &[broken])[0].ok);
    let none = Bundle {
        checks: vec![],
        ..bundle()
    };
    assert!(
        !compare_checks(&none, &[])[0].ok,
        "nothing to compare is no match"
    );
}

#[test]
fn a_report_matches_only_when_everything_does() {
    let mut r = VerifyReport::default();
    assert!(!r.matches(), "an empty report proves nothing");
    r.items.push(VerifyItem::ok("digest", "abc"));
    assert_eq!(r.verdict(), "match");
    r.items.push(VerifyItem::new("head", "a", "b"));
    assert_eq!(r.verdict(), "mismatch");
    assert_eq!(r.mismatches().count(), 1);
}

fn card(job: i64, outcome: RunOutcome, usd: f64, at: i64, mission: bool) -> RunCard {
    RunCard {
        job: JobId(job),
        mission: mission.then(|| format!("m{job}")),
        title: format!("run {job}"),
        outcome,
        evidence: (outcome == RunOutcome::Delivered).then(|| RunEvidence {
            digest: "abcd1234".into(),
            checks_passed: 1,
            checks_total: 1,
            blocks: 1,
        }),
        cost_usd: usd,
        models: vec![("sonnet".into(), usd)],
        labels: Vec::new(),
        at,
    }
}

#[test]
fn stats_count_missions_only() {
    let cards = [
        card(1, RunOutcome::Delivered, 1.0, 10, true),
        card(2, RunOutcome::Delivered, 2.0, 20, true),
        card(3, RunOutcome::Escalated, 3.0, 30, true),
        card(4, RunOutcome::Done, 9.0, 40, false),
        card(5, RunOutcome::Running, 0.5, 50, true),
    ];
    let s = LedgerStats::of(&cards);
    assert_eq!((s.missions, s.delivered), (3, 2));
    let rate = s.pass_rate.unwrap();
    assert!((rate - 2.0 / 3.0).abs() < 1e-9);
    assert!((s.pass_k.unwrap() - rate.powi(3)).abs() < 1e-9);
    assert!((s.cost_per_verified.unwrap() - 3.0).abs() < 1e-9);
    // Two blocks (one per delivered card) and one escalation, over three.
    assert!((s.interventions_per_task.unwrap() - 1.0).abs() < 1e-9);
    assert!((s.spent_usd - 15.5).abs() < 1e-9);
    assert_eq!(LedgerStats::of(&[]).pass_rate, None);
}

#[test]
fn labels_follow_the_pull_request() {
    let pr = |state, reverted_at| PrOutcome {
        number: 12,
        state,
        url: String::new(),
        reverted_at,
    };
    let day = 86_400_000;
    assert_eq!(
        label_of(&pr(PrState::Open, None), 5).kind,
        LabelKind::PrOpen
    );
    assert_eq!(
        label_of(&pr(PrState::Closed, None), 5).kind,
        LabelKind::Closed
    );
    let merged = pr(PrState::Merged { at: 100 }, None);
    assert_eq!(label_of(&merged, 5).kind, LabelKind::Merged);
    let soon = pr(PrState::Merged { at: 100 }, Some(100 + 6 * day));
    assert_eq!(label_of(&soon, 0).kind, LabelKind::Reverted);
    let late = pr(PrState::Merged { at: 100 }, Some(100 + 8 * day));
    assert_eq!(label_of(&late, 0).kind, LabelKind::Merged);
    for k in [
        LabelKind::PrOpen,
        LabelKind::Merged,
        LabelKind::Reverted,
        LabelKind::Closed,
    ] {
        assert_eq!(LabelKind::parse(k.as_str()), Some(k));
    }
}

#[test]
fn the_brief_sums_a_window() {
    let mut cards = vec![
        card(1, RunOutcome::Delivered, 1.0, 100, true),
        card(2, RunOutcome::Escalated, 2.0, 200, true),
        card(3, RunOutcome::Failed, 0.5, 300, true),
        card(4, RunOutcome::Delivered, 9.0, 5, true),
    ];
    cards[0].labels.push(Label {
        kind: LabelKind::Merged,
        pr: 1,
        at: 150,
    });
    let b = brief(&cards, 50, 400);
    assert_eq!((b.delivered, b.escalated, b.failed), (1, 1, 1));
    assert_eq!(b.merged, 1);
    assert!((b.spent_usd - 3.5).abs() < 1e-9);
    assert_eq!(b.needs_you, ["run 3", "run 2"]);
    assert_eq!(
        b.headline(),
        "1 delivered · 1 escalated · 1 failed · 1 merged · $3.50"
    );
    let empty = brief(&cards, 1_000, 2_000);
    assert!(empty.is_empty());
    assert_eq!(empty.headline(), "nothing happened");
}

#[test]
fn the_heat_strip_counts_days() {
    let day = 86_400_000;
    let cards = [
        card(1, RunOutcome::Done, 0.0, 10 * day + 5, false),
        card(2, RunOutcome::Done, 0.0, 10 * day + 9, false),
        card(3, RunOutcome::Done, 0.0, 8 * day, false),
        card(4, RunOutcome::Done, 0.0, 2 * day, false),
    ];
    assert_eq!(heat_strip(&cards, 10 * day + 100, 4), [0, 1, 0, 2]);
}

#[test]
fn hidden_checks_are_counted_in_a_bundle_and_never_named() {
    let plain = bundle();
    let before = plain.digest_hex();
    assert!(
        !plain.body_json().contains("hidden"),
        "old bundles keep their digest"
    );
    let mut b = bundle();
    b.hidden = Some(HiddenTally { ran: 3, passed: 3 });
    assert_ne!(b.digest_hex(), before);
    b.signature = Some(Signature {
        alg: "sha256".into(),
        value: b.digest_hex(),
    });
    let text = b.to_json();
    assert!(text.contains(r#""hidden":{"ran":3,"passed":3}"#), "{text}");
    let back = Bundle::from_json(&text).unwrap();
    assert_eq!(back.hidden, Some(HiddenTally { ran: 3, passed: 3 }));
    assert_eq!(back.signature_ok(), Ok(true));
}

#[test]
fn a_verdict_carries_its_hidden_tally() {
    let v =
        parse_verdict(r#"{"outcome":"pass","checks":[],"hidden":{"ran":2,"passed":1}}"#).unwrap();
    assert_eq!(v.hidden, Some(HiddenTally { ran: 2, passed: 1 }));
    assert_eq!(parse_verdict(r#"{"outcome":"pass"}"#).unwrap().hidden, None);
}
