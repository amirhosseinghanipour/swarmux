use super::*;

fn contract(goal: &str, checks: &[&str], protected: &[&str]) -> Contract {
    Contract {
        title: "t".into(),
        goal: goal.into(),
        acceptance: Acceptance {
            checks: checks.iter().map(|c| c.to_string()).collect(),
            human_signoff: false,
        },
        constraints: Constraints {
            paths_rw: vec!["src/**".into()],
            protected: protected.iter().map(|c| c.to_string()).collect(),
        },
        budget: Budget {
            usd: Some(5.0),
            ..Budget::default()
        },
        clarifications: Vec::new(),
    }
}

fn kinds(findings: &[Finding]) -> Vec<FindingKind> {
    findings.iter().map(|f| f.kind).collect()
}

const GOAL: &str = "make search return results in under 50 ms";

#[test]
fn seeded_missing_acceptance_is_flagged_and_blocks() {
    for checks in [&[][..], &["  "][..]] {
        let f = lint(&contract(GOAL, checks, &[]));
        assert_eq!(kinds(&f), [FindingKind::MissingAcceptance]);
        assert_eq!(f[0].severity, Severity::Blocking);
        assert!(lint_report(&contract(GOAL, checks, &[])).blocking());
    }
}

#[test]
fn seeded_unprotected_oracles_are_flagged_with_a_fix() {
    let cases: [(&[&str], &[&str], &[&str]); 6] = [
        (&["cargo test --workspace"], &[], &["tests/**"]),
        (&["go test ./..."], &[], &["*_test.go"]),
        (&["pytest -q"], &[], &["tests/**", "test_*.py"]),
        (
            &["sh scripts/flake_rate.sh --max 0.005"],
            &[],
            &["scripts/flake_rate.sh"],
        ),
        (
            &["cargo test", "python tests/oracles/parse.py"],
            &["tests/"],
            &[],
        ),
        (&["npm test"], &["*.test.*"], &["__tests__/**"]),
    ];
    for (checks, protected, expected) in cases {
        let f = lint(&contract(GOAL, checks, protected));
        let flagged: Vec<&str> = f
            .iter()
            .filter(|f| f.kind == FindingKind::UnprotectedOracle)
            .filter_map(|f| f.protect.as_deref())
            .collect();
        assert_eq!(flagged, expected, "{checks:?} protected {protected:?}");
        assert!(f.iter().all(|f| f.severity != Severity::Blocking));
    }
}

#[test]
fn protecting_the_oracles_settles_the_finding() {
    let mut c = contract(GOAL, &["cargo test", "sh scripts/check.sh"], &[]);
    let before = lint(&c);
    assert_eq!(apply_fixes(&mut c, &before), 2);
    assert_eq!(c.constraints.protected, ["tests/**", "scripts/check.sh"]);
    assert!(
        !kinds(&lint(&c)).contains(&FindingKind::UnprotectedOracle),
        "{:?}",
        lint(&c)
    );
    assert_eq!(apply_fixes(&mut c, &before), 0, "no repeats");
}

#[test]
fn a_protected_writable_path_is_a_conflict() {
    let mut c = contract(GOAL, &["cargo test"], &["tests/**"]);
    c.constraints.paths_rw = vec!["src/**".into(), "tests/**".into()];
    let f = lint(&c);
    assert!(
        f.iter()
            .any(|f| f.kind == FindingKind::Conflict && f.severity == Severity::Blocking),
        "{f:?}"
    );
}

#[test]
fn a_clear_contract_lints_clean() {
    let c = contract(GOAL, &["cargo test -p search"], &["tests/**"]);
    assert_eq!(lint(&c), []);
    let report = lint_report(&c);
    assert!(!report.blocking());
    assert!(report.questions.is_empty());
}

#[test]
fn short_goals_and_missing_budget_and_scope_are_noted() {
    let mut c = contract("go faster", &["cargo test"], &["tests/**"]);
    c.budget = Budget::default();
    c.constraints.paths_rw.clear();
    let f = lint(&c);
    assert_eq!(
        kinds(&f),
        [
            FindingKind::VagueGoal,
            FindingKind::NoBudget,
            FindingKind::Unscoped
        ]
    );
    let q = local_questions(&f);
    assert_eq!(q.len(), 1);
    assert!(q[0].question.contains("different"));
}

#[test]
fn questions_are_capped_at_three_without_repeats() {
    let q = |s: &str| Question {
        question: s.into(),
        why: String::new(),
    };
    let report = LintReport {
        questions: vec![q("Which command shows it is done?"), q("local two")],
        ..LintReport::default()
    }
    .with_questions(vec![
        q("which command shows it is done?"),
        q("model two"),
        q(""),
        q("model three"),
    ]);
    let asked: Vec<&str> = report
        .questions
        .iter()
        .map(|q| q.question.as_str())
        .collect();
    assert_eq!(
        asked,
        [
            "which command shows it is done?",
            "model two",
            "model three"
        ]
    );
}

#[test]
fn compile_makes_the_prompt_and_the_gate() {
    let mut c = contract(
        GOAL,
        &["cargo test -p search", "\"my tool\" --x"],
        &["tests/**"],
    );
    c.clarifications = vec![
        Clarification {
            question: "Which endpoint?".into(),
            answer: "/search only".into(),
        },
        Clarification {
            question: "Unanswered?".into(),
            answer: String::new(),
        },
    ];
    let out = compile(&c).unwrap();
    let names: Vec<&str> = out.gate.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["cargo test -p search", "my tool --x"]);
    assert_eq!(out.gate.checks[1].program, "my tool");
    assert_eq!(out.gate.protected, ["tests/**"]);
    let p = &out.prompt;
    assert!(p.starts_with(GOAL), "{p}");
    assert!(p.contains("- `cargo test -p search`"), "{p}");
    assert!(p.contains("Work in: src/**"), "{p}");
    assert!(
        p.contains("Never change these (writes are refused): tests/**, .swarmux/**"),
        "{p}"
    );
    assert!(p.contains("- Which endpoint? /search only"), "{p}");
    assert!(!p.contains("Unanswered"), "{p}");
    assert!(p.contains("about $5.00"), "{p}");
}

#[test]
fn compile_refuses_what_cannot_be_checked() {
    let blocking = compile(&contract(GOAL, &[], &[])).unwrap_err();
    assert_eq!(kinds(&blocking), [FindingKind::MissingAcceptance]);
}

#[test]
fn the_happy_path_ends_delivered() {
    let mut m = Mission::new("m", "/repo", contract(GOAL, &["cargo test"], &[]), 0);
    for s in [
        Signal::Linted { blocking: false },
        Signal::Queued(JobId(4)),
        Signal::Started,
        Signal::Gate(GateOutcome::Block),
        Signal::Finished { ok: true },
        Signal::Gate(GateOutcome::Pass),
    ] {
        m.apply(s, 1).unwrap();
    }
    assert_eq!((m.state, m.job), (MissionState::Delivered, Some(JobId(4))));
    let e = m.apply(Signal::Started, 2).unwrap_err();
    assert_eq!(e.to_string(), "a delivered mission can't be started");
    m.apply(Signal::Edited, 3).unwrap();
    assert_eq!((m.state, m.job), (MissionState::Draft, None));
}

#[test]
fn a_gate_pass_before_the_job_ends_is_not_a_delivery() {
    assert_eq!(
        step(MissionState::Running, &Signal::Gate(GateOutcome::Pass)),
        Ok(MissionState::Running)
    );
    assert!(step(MissionState::Queued, &Signal::Gate(GateOutcome::Pass)).is_err());
    assert!(step(MissionState::Linted, &Signal::Gate(GateOutcome::Pass)).is_err());
}

#[test]
fn failures_and_budgets_escalate_with_a_note() {
    let mut m = Mission::new("m", "/repo", Contract::default(), 0);
    m.state = MissionState::Running;
    m.apply(Signal::Exhausted("spent $5.10 of $5.00".into()), 1)
        .unwrap();
    assert_eq!(m.state, MissionState::Escalated);
    assert_eq!(m.note.as_deref(), Some("spent $5.10 of $5.00"));
    m.state = MissionState::Verifying;
    m.apply(Signal::Gate(GateOutcome::Escalate), 2).unwrap();
    assert!(m.note.as_deref().unwrap().contains("gate gave up"));
    m.state = MissionState::Linted;
    m.apply(Signal::Refuse("no checks".into()), 3).unwrap();
    assert_eq!(m.state, MissionState::Refused);
}

#[test]
fn states_round_trip() {
    for s in MissionState::ALL {
        assert_eq!(MissionState::parse(s.as_str()), Some(s));
    }
    assert_eq!(MissionState::parse("done"), None, "never a bare done");
    for k in [
        FindingKind::MissingAcceptance,
        FindingKind::UnprotectedOracle,
        FindingKind::Conflict,
        FindingKind::VagueGoal,
        FindingKind::NoBudget,
        FindingKind::Unscoped,
    ] {
        assert_eq!(FindingKind::parse(k.as_str()), Some(k));
    }
}

/// xorshift64*: a small deterministic generator, so the property test
/// needs no extra crate and fails the same way every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn any_signal(rng: &mut Rng) -> Signal {
    let outcomes = [GateOutcome::Pass, GateOutcome::Block, GateOutcome::Escalate];
    match rng.below(9) {
        0 => Signal::Edited,
        1 => Signal::Linted {
            blocking: rng.below(2) == 0,
        },
        2 => Signal::Queued(JobId(rng.below(5) as i64)),
        3 => Signal::Started,
        4 => Signal::Finished {
            ok: rng.below(3) != 0,
        },
        5 | 6 => Signal::Gate(outcomes[rng.below(3) as usize]),
        7 => Signal::Exhausted("budget".into()),
        _ => Signal::Refuse("policy".into()),
    }
}

/// Property: whatever happens, in whatever order, a mission reaches
/// `Delivered` only by a passing gate verdict taken while verifying, after
/// its job finished well; and a refused or escalated mission only leaves
/// that state by being edited.
#[test]
fn no_mission_is_delivered_without_a_gate_pass() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut delivered = 0;
    for _ in 0..20_000 {
        let mut m = Mission::new("p", "/r", Contract::default(), 0);
        let mut finished_ok_since_queued = false;
        for t in 0..rng.below(24) as i64 {
            let signal = any_signal(&mut rng);
            let before = m.state;
            let Ok(_) = m.apply(signal.clone(), t) else {
                assert_eq!(m.state, before, "a refused signal changes nothing");
                continue;
            };
            match &signal {
                Signal::Queued(_) | Signal::Edited => finished_ok_since_queued = false,
                Signal::Finished { ok: true } => finished_ok_since_queued = true,
                _ => {}
            }
            if m.state == MissionState::Delivered && before != MissionState::Delivered {
                delivered += 1;
                assert_eq!(
                    signal,
                    Signal::Gate(GateOutcome::Pass),
                    "{before} → delivered"
                );
                assert_eq!(before, MissionState::Verifying);
                assert!(finished_ok_since_queued, "delivered before its job ended");
                assert!(m.job.is_some(), "delivered without a job");
            }
            if matches!(before, MissionState::Escalated | MissionState::Refused)
                && m.state != before
            {
                assert_eq!(signal, Signal::Edited, "{before} left by {signal:?}");
            }
        }
    }
    assert!(
        delivered > 100,
        "the walk reached delivery {delivered} times"
    );
}
