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

#[test]
fn outcomes_round_trip() {
    for o in [
        Outcome::Allow,
        Outcome::Deny,
        Outcome::Ask,
        Outcome::Pass,
        Outcome::Stop,
    ] {
        assert_eq!(Outcome::parse(o.as_str()), Some(o));
    }
    assert_eq!(Outcome::parse("x"), None);
}
