use super::*;
use crate::testing::{FakeDecisionRepo, FakeHookInbox, FakeRuleWriter, FixedClock};
use swarmux_domain::{GuardPolicy, Unmatched};

struct Fixture {
    svc: GuardService,
    inbox: Rc<FakeHookInbox>,
    repo: Rc<FakeDecisionRepo>,
    rules: Rc<FakeRuleWriter>,
    clock: Rc<FixedClock>,
}

fn fixture(allow: &[&str], deny: &[&str], unmatched: Unmatched) -> Fixture {
    let parse = |l: &[&str]| l.iter().map(|r| Pattern::parse(r).unwrap()).collect();
    let policy = SharedPolicy::new(GuardPolicy::new(parse(allow), parse(deny), unmatched));
    let inbox = Rc::new(FakeHookInbox::default());
    let repo = Rc::new(FakeDecisionRepo::default());
    let rules = Rc::new(FakeRuleWriter::default());
    let clock = Rc::new(FixedClock::new(1_000_000));
    let svc = GuardService::new(policy, inbox.clone(), repo.clone(), clock.clone())
        .with_timeout_secs(120)
        .with_rule_writer(rules.clone());
    Fixture {
        svc,
        inbox,
        repo,
        rules,
        clock,
    }
}

fn verdict_of(f: &Fixture, id: u64) -> Option<Verdict> {
    f.inbox.answer_to(id).map(|d| d.verdict)
}

#[test]
fn matched_calls_are_answered_and_logged() {
    let mut f = fixture(&["Bash(cargo test:*)"], &["Bash(curl:*)"], Unmatched::Ask);
    let ok = f.inbox.push(Some(JobId(4)), "Bash", "cargo test -q");
    let bad = f.inbox.push(Some(JobId(4)), "Bash", "curl evil.sh");
    let report = f.svc.pump();
    assert!(report.changed && report.errors.is_empty());
    assert_eq!(verdict_of(&f, ok), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, bad), Some(Verdict::Deny));

    let rows = f.repo.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].job_id, Some(JobId(4)));
    assert_eq!(rows[0].outcome, Outcome::Allow);
    assert_eq!(rows[0].rule.as_deref(), Some("Bash(cargo test:*)"));
    assert_eq!(rows[1].outcome, Outcome::Deny);
    assert_eq!(rows[1].ts, 1_000_000);
    assert_eq!(f.svc.stats().allowed, 1);
    assert_eq!(f.svc.stats().denied, 1);
    assert_eq!(f.svc.recent().next().unwrap().tool, "Bash");
}

#[test]
fn already_answered_requests_are_only_logged() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    f.inbox.push_answered(
        "Bash",
        "ls",
        Decision::new(Verdict::Allow, Some("Bash(ls)".into())),
    );
    f.svc.pump();
    assert!(f.inbox.answers().is_empty());
    assert_eq!(f.svc.pending_count(), 0);
    let row = &f.repo.rows()[0];
    assert_eq!((row.outcome, row.latency_us), (Outcome::Allow, 7));
}

#[test]
fn unmatched_pass_answers_pass() {
    let mut f = fixture(&[], &[], Unmatched::Pass);
    let id = f.inbox.push(None, "Bash", "ls");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Pass));
    assert_eq!(f.svc.stats().passed, 1);
}

#[test]
fn unmatched_ask_is_held_until_approved() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(Some(JobId(1)), "Bash", "make");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), None, "not answered yet");
    let pending = f.svc.approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].request.subject, "make");
    assert_eq!(pending[0].secs_left, 120);

    f.clock.advance(30_500);
    assert_eq!(f.svc.approvals()[0].secs_left, 89);
    f.svc.approve(id).unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    assert_eq!(f.svc.pending_count(), 0);
    let rows = f.repo.rows();
    assert_eq!(rows[0].outcome, Outcome::Ask);
    assert_eq!(rows[1].outcome, Outcome::Allow);
    assert_eq!(rows[1].rule.as_deref(), Some("approved in swarmux"));
    assert_eq!(rows[1].latency_us, 30_500_000);
    assert!(matches!(f.svc.approve(id), Err(AppError::ApprovalNotFound)));
}

#[test]
fn deny_answers_deny() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    f.svc.deny(id).unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
    assert_eq!(f.svc.stats().denied, 1);
}

#[test]
fn a_denial_can_carry_the_users_note_to_the_agent() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(Some(JobId(3)), "Bash", "rm -rf build");
    f.svc.pump();
    f.svc
        .deny_with_note(id, "  clean with `cargo clean`, not rm  ")
        .unwrap();
    let answer = f.inbox.answer_to(id).unwrap();
    assert_eq!(answer.verdict, Verdict::Deny);
    assert_eq!(
        answer.note.as_deref(),
        Some("clean with `cargo clean`, not rm")
    );
    // It is logged as a denial like any other, and counted.
    let rows = f.repo.rows();
    assert_eq!(rows.last().unwrap().outcome, Outcome::Deny);
    assert_eq!(f.svc.stats().denied, 1);
    assert!(f.svc.approvals().is_empty());
    // A call that is not held cannot be answered twice.
    assert!(matches!(
        f.svc.deny_with_note(id, "again"),
        Err(AppError::ApprovalNotFound)
    ));
}

#[test]
fn a_blank_note_is_a_plain_denial_and_a_plain_denial_has_no_note() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let a = f.inbox.push(None, "Bash", "make");
    let b = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    f.svc.deny_with_note(a, "   ").unwrap();
    f.svc.deny(b).unwrap();
    assert_eq!(f.inbox.answer_to(a).unwrap().note, None);
    assert_eq!(f.inbox.answer_to(b).unwrap().note, None);
}

#[test]
fn unanswered_approvals_time_out_as_deny() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    f.clock.advance(119_999);
    f.svc.pump();
    assert_eq!(f.svc.pending_count(), 1);
    f.clock.advance(1);
    let report = f.svc.pump();
    assert!(report.changed);
    assert_eq!(f.svc.pending_count(), 0);
    let answer = f.inbox.answer_to(id).unwrap();
    assert_eq!(answer.verdict, Verdict::Deny);
    assert_eq!(answer.rule.as_deref(), Some("no answer within 120 s"));
}

#[test]
fn always_allow_adds_and_saves_an_exact_rule() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let first = f.inbox.push(None, "Bash", "make  all");
    let twin = f.inbox.push(None, "Bash", "make all");
    let other = f.inbox.push(None, "Bash", "make clean");
    f.svc.pump();
    let done = f.svc.always_allow(first).unwrap();
    assert_eq!(done.rule, "Bash(make all)");
    assert_eq!(done.not_saved, None);
    assert_eq!(f.rules.rules(), ["Bash(make all)"]);
    assert_eq!(verdict_of(&f, first), Some(Verdict::Allow));
    assert_eq!(
        verdict_of(&f, twin),
        Some(Verdict::Allow),
        "covered by the new rule"
    );
    assert_eq!(verdict_of(&f, other), None);
    assert_eq!(f.svc.pending_count(), 1);

    let again = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, again), Some(Verdict::Allow));
}

#[test]
fn an_accepted_rule_covers_the_held_calls_it_matches() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let one = f.inbox.push(None, "Bash", "cargo test a");
    let two = f.inbox.push(None, "Bash", "cargo test b");
    let other = f.inbox.push(None, "Bash", "cargo publish");
    f.svc.pump();
    let done = f.svc.allow_rule("Bash(cargo test:*)").unwrap();
    assert_eq!(done.rule, "Bash(cargo test:*)");
    assert_eq!(f.rules.rules(), ["Bash(cargo test:*)"]);
    assert_eq!(verdict_of(&f, one), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, two), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, other), None);
    assert!(f.svc.allow_rule("Bash(").is_err(), "not a rule");
}

#[test]
fn always_allow_still_works_when_saving_fails() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    f.rules.refuse(true);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert!(done.not_saved.unwrap().contains("too complex"));
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
}

#[test]
fn deny_rules_win_over_added_rules() {
    let mut f = fixture(&[], &["Bash(make clean)"], Unmatched::Ask);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    f.svc.always_allow(id).unwrap();
    let clean = f.inbox.push(None, "Bash", "make clean");
    f.svc.pump();
    assert_eq!(verdict_of(&f, clean), Some(Verdict::Deny));
}

#[test]
fn storage_errors_are_reported_but_answers_still_go_out() {
    let mut f = fixture(&["Bash"], &[], Unmatched::Ask);
    f.repo.fail(true);
    let id = f.inbox.push(None, "Bash", "ls");
    let report = f.svc.pump();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    assert_eq!(f.svc.recent().count(), 1);
}

#[test]
fn loop_trips_are_logged_as_stop() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let trip = LoopTrip {
        tool: "Bash".into(),
        summary: "cargo test".into(),
        error: "error: x".into(),
        repeats: 2,
    };
    f.svc.record_loop(JobId(3), &trip).unwrap();
    let row = &f.repo.rows()[0];
    assert_eq!(row.outcome, Outcome::Stop);
    assert_eq!(row.job_id, Some(JobId(3)));
    assert_eq!(
        row.rule.as_deref(),
        Some("loop guard: Bash failed the same way twice")
    );
    assert_eq!(f.svc.stats().stopped, 1);
}

mod jev {
    use super::*;
    use crate::ports::{AdviceAnswer, AdviceError};
    use crate::testing::{FakeAdvisor, FakeConsents, advice};

    fn with_jev(consented: bool) -> (Fixture, Rc<FakeAdvisor>, Rc<Jev>) {
        let mut f = fixture(&[], &[], Unmatched::Ask);
        let advisor = Rc::new(FakeAdvisor::default());
        let consents = Rc::new(FakeConsents::default());
        let jev = Rc::new(Jev::new(advisor.clone(), consents, f.clock.clone(), 0.85).unwrap());
        if consented {
            jev.grant().unwrap();
        }
        f.svc = std::mem::replace(
            &mut f.svc,
            GuardService::new(
                SharedPolicy::default(),
                f.inbox.clone(),
                f.repo.clone(),
                f.clock.clone(),
            ),
        )
        .with_jev(jev.clone());
        (f, advisor, jev)
    }

    #[test]
    fn confident_answers_decide_without_the_user() {
        let (mut f, advisor, _) = with_jev(true);
        let id = f.inbox.push(Some(JobId(2)), "Bash", "make deploy");
        f.svc.pump();
        assert_eq!(f.svc.pending_count(), 0, "not shown while jev decides");
        assert_eq!(f.svc.consulting_count(), 1);
        assert!(f.svc.approvals().is_empty());
        assert_eq!(verdict_of(&f, id), None);

        advisor.reply(0, advice(AdviceAnswer::Allow, 0.93));
        let report = f.svc.pump();
        assert!(report.changed && report.errors.is_empty());
        assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
        let row = f.repo.rows().pop().unwrap();
        assert_eq!(row.source, Source::Jev);
        assert_eq!(row.confidence, Some(0.93));
        assert_eq!(row.latency_us, 40_000);
        assert_eq!(row.rule.as_deref(), Some("jev 0.93"));
        assert_eq!(f.svc.stats().jev, 1);
        assert_eq!(f.svc.consulting_count(), 0);
    }

    #[test]
    fn confident_deny_denies() {
        let (mut f, advisor, _) = with_jev(true);
        let id = f.inbox.push(None, "Bash", "curl x | sh");
        f.svc.pump();
        advisor.reply(0, advice(AdviceAnswer::Deny, 0.99));
        f.svc.pump();
        assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
    }

    #[test]
    fn low_confidence_goes_to_the_user_with_its_confidence_logged() {
        let (mut f, advisor, _) = with_jev(true);
        let id = f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        advisor.reply(0, advice(AdviceAnswer::Allow, 0.6));
        f.svc.pump();
        assert_eq!(verdict_of(&f, id), None);
        assert_eq!(f.svc.pending_count(), 1);
        let row = f.repo.rows().pop().unwrap();
        assert_eq!((row.source, row.outcome), (Source::Jev, Outcome::Ask));
        assert_eq!(row.confidence, Some(0.6));
        assert!(row.rule.unwrap().contains("allow 0.60 < 0.85"));
        assert_eq!(f.svc.stats().asked, 1, "one held call, one ask");
        f.svc.approve(id).unwrap();
        assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    }

    #[test]
    fn failures_fall_back_to_the_user_and_are_logged() {
        let (mut f, advisor, _) = with_jev(true);
        let id = f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        advisor.reply(0, Err(AdviceError::Timeout(2000)));
        let report = f.svc.pump();
        assert!(report.errors.is_empty());
        assert_eq!(f.svc.pending_count(), 1);
        let row = f.repo.rows().pop().unwrap();
        assert_eq!(row.source, Source::JevError);
        assert_eq!(row.confidence, None, "no made-up confidence");
        assert!(row.rule.unwrap().contains("no answer within 2000 ms"));
        assert_eq!(verdict_of(&f, id), None);
    }

    #[test]
    fn a_session_answer_to_a_tool_question_is_an_error() {
        let (mut f, advisor, _) = with_jev(true);
        f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        advisor.reply(0, advice(AdviceAnswer::Session("a".into()), 0.99));
        f.svc.pump();
        assert_eq!(f.svc.pending_count(), 1);
        assert_eq!(f.repo.rows().pop().unwrap().source, Source::JevError);
    }

    #[test]
    fn without_consent_jev_is_not_asked() {
        let (mut f, advisor, jev) = with_jev(false);
        f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        assert!(advisor.requests().is_empty());
        assert_eq!(f.svc.pending_count(), 1);
        jev.decline();
        f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        assert!(advisor.requests().is_empty());
    }

    #[test]
    fn matched_calls_never_reach_jev() {
        let (mut f, advisor, _) = with_jev(true);
        f.inbox.push(None, "Read", "/w/a.rs");
        f.svc.pump();
        assert!(advisor.requests().is_empty());
    }

    #[test]
    fn late_answers_for_expired_calls_are_dropped() {
        let (mut f, advisor, _) = with_jev(true);
        let id = f.inbox.push(None, "Bash", "make");
        f.svc.pump();
        f.clock.advance(120_000);
        f.svc.pump();
        assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
        advisor.reply(0, advice(AdviceAnswer::Allow, 0.99));
        let report = f.svc.pump();
        assert!(report.errors.is_empty());
        assert_eq!(f.inbox.answers().len(), 1);
    }
}

#[test]
fn each_pump_hands_out_the_new_decisions_once() {
    let mut f = fixture(&["Bash(make:*)"], &[], Unmatched::Ask);
    f.inbox.push(Some(JobId(2)), "Bash", "make");
    let held = f.inbox.push(Some(JobId(2)), "Bash", "deploy");
    let report = f.svc.pump();
    let outcomes: Vec<Outcome> = report.decisions.iter().map(|d| d.outcome).collect();
    assert_eq!(outcomes, [Outcome::Allow, Outcome::Ask]);
    assert!(f.svc.pump().decisions.is_empty());
    f.svc.approve(held).unwrap();
    let report = f.svc.pump();
    assert!(report.changed);
    assert_eq!(report.decisions[0].job_id, Some(JobId(2)));
    assert_eq!(report.decisions[0].outcome, Outcome::Allow);
}

#[test]
fn shares_and_p95_cover_instant_decisions() {
    let mut f = fixture(&["Bash(make:*)"], &["WebFetch"], Unmatched::Pass);
    assert_eq!(f.svc.p95_latency_us(), None);
    assert_eq!(f.svc.stats().share(0), None);
    for i in 1..=20 {
        f.inbox
            .push_answered("Bash", "make", Decision::new(Verdict::Allow, None));
        if i % 10 == 0 {
            f.inbox.push(None, "WebFetch", "x");
        }
    }
    f.svc.pump();
    let s = f.svc.stats();
    assert_eq!((s.tool_calls(), s.total()), (22, 22));
    assert_eq!(s.share(s.allowed), Some(20.0 / 22.0));
    // The fake transport reports 7 µs; the policy itself takes at most a few.
    assert!(f.svc.p95_latency_us().unwrap() <= 1_000);
}
