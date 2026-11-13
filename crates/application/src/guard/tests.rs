use super::*;
use std::sync::Arc;

use crate::testing::{FakeDecisionRepo, FakeHookInbox, FakeRuleStore, FixedClock, FixedRoots};
use swarmux_domain::{DAY_MS, GuardPolicy, Unmatched};

struct Fixture {
    svc: GuardService,
    inbox: Rc<FakeHookInbox>,
    repo: Rc<FakeDecisionRepo>,
    rules: Rc<FakeRuleStore>,
    clock: Rc<FixedClock>,
}

fn fixture(allow: &[&str], deny: &[&str], unmatched: Unmatched) -> Fixture {
    let parse = |l: &[&str]| l.iter().map(|r| Pattern::parse(r).unwrap()).collect();
    let clock = Rc::new(FixedClock::new(1_000_000));
    let policy = SharedPolicy::new(GuardPolicy::new(parse(allow), parse(deny), unmatched))
        .with_roots(Arc::new(FixedRoots::repo("/w")))
        .with_now(Arc::new(|| 1_000_000));
    let inbox = Rc::new(FakeHookInbox::default());
    let repo = Rc::new(FakeDecisionRepo::default());
    let rules = Rc::new(FakeRuleStore::default());
    let svc = GuardService::new(policy, inbox.clone(), repo.clone(), clock.clone())
        .with_timeout_secs(120)
        .with_rule_store(rules.clone());
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
    let kept = f.rules.rules();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].text, "Bash(make all)");
    assert_eq!(
        (kept[0].scope, kept[0].root.as_deref(), kept[0].source),
        (RuleScope::Repo, Some("/w"), RuleSource::AlwaysAllow)
    );
    assert_eq!(kept[0].expires_at, Some(1_000_000 + 30 * DAY_MS));
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
    let done = f.svc.allow_rule("Bash(cargo test:*)", one).unwrap();
    assert_eq!(done.rule, "Bash(cargo test:*)");
    assert_eq!(f.rules.rules()[0].text, "Bash(cargo test:*)");
    assert_eq!(f.rules.rules()[0].source, RuleSource::Suggestion);
    assert_eq!(verdict_of(&f, one), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, two), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, other), None);
    assert!(f.svc.allow_rule("Bash(", other).is_err(), "not a rule");
}

#[test]
fn always_allow_still_works_when_saving_fails() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    f.rules.refuse(true);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert!(done.not_saved.unwrap().contains("read-only"));
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    // It lasts this run: reading the store again doesn't drop it.
    f.clock.advance(3_000);
    let again = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    assert_eq!(verdict_of(&f, again), Some(Verdict::Allow));
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

// --- unmatched calls: the owner decides, no model is asked ------------------

#[test]
fn an_unmatched_call_waits_for_the_owner_and_no_model_is_asked() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(Some(JobId(2)), "Bash", "make deploy");
    let report = f.svc.pump();
    assert!(report.changed && report.errors.is_empty());
    // Held at once and shown: nothing stands between the call and the owner.
    assert_eq!(f.svc.pending_count(), 1);
    assert_eq!(f.svc.approvals()[0].request.subject, "make deploy");
    assert_eq!(verdict_of(&f, id), None);
    assert!(f.inbox.answers().is_empty());
    // Pumping again changes nothing: no answer arrives from anywhere.
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), None);
    let rows = f.repo.rows();
    assert_eq!(rows.len(), 1, "one record: that it was held");
    assert_eq!(
        (rows[0].source, rows[0].outcome),
        (Source::Guard, Outcome::Ask)
    );
    assert_eq!(rows[0].confidence, None);
    assert_eq!(f.svc.stats().asked, 1);

    f.svc.approve(id).unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    let row = f.repo.rows().pop().unwrap();
    assert_eq!((row.source, row.outcome), (Source::Guard, Outcome::Allow));
    assert_eq!(row.extra.resolver.as_deref(), Some("human-key"));
    assert_eq!(f.svc.pending_count(), 0);
}

#[test]
fn an_unmatched_call_the_owner_denies_is_denied_and_logged_as_theirs() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(None, "Bash", "curl x | sh");
    f.svc.pump();
    f.svc.deny(id).unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
    let row = f.repo.rows().pop().unwrap();
    assert_eq!(row.extra.resolver.as_deref(), Some("human-key"));
}

#[test]
fn an_unmatched_call_nobody_answers_is_denied_after_the_timeout() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    f.clock.advance(119_000);
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), None, "still waiting");
    f.clock.advance(1_000);
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
    let row = f.repo.rows().pop().unwrap();
    assert_eq!(row.extra.resolver.as_deref(), Some("timeout"));
    assert!(f.svc.approvals().is_empty());
}

#[test]
fn matched_calls_are_never_held() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let id = f.inbox.push(None, "Read", "/w/a.rs");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    assert_eq!(f.svc.pending_count(), 0);
}

#[test]
fn unmatched_pass_leaves_the_call_to_claude_code_without_holding_it() {
    let mut f = fixture(&[], &[], Unmatched::Pass);
    let id = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Pass));
    assert_eq!(f.svc.pending_count(), 0);
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

// --- X1: irreversible calls, scoped and expiring rules, protections -------

use swarmux_domain::alarm::ARMING_MS;

/// A held `rm -rf build` and the challenge a client would have shown for it.
fn held_irreversible(f: &mut Fixture, job: Option<JobId>) -> (u64, Challenge) {
    let id = f.inbox.push(job, "Bash", "rm -rf build");
    f.svc.pump();
    let call = f
        .svc
        .approvals()
        .into_iter()
        .find(|p| p.request.request_id == id);
    let call = call.expect("an irreversible call is held for the owner");
    let challenge = Challenge::new(id, "Bash", &call.request.subject, f.clock.now());
    (id, challenge)
}

#[test]
fn an_irreversible_call_is_never_allowed_by_a_rule() {
    // Every kind of allow rule the owner can have, built-in or learned.
    let mut f = fixture(
        &["Bash", "Bash(rm:*)", "Bash(rm -rf build)"],
        &[],
        Unmatched::Ask,
    );
    let id = f.inbox.push(None, "Bash", "rm -rf build");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), None, "held, not allowed");
    let row = f.repo.rows().pop().unwrap();
    assert_eq!(row.outcome, Outcome::Ask);
    assert!(
        row.rule
            .as_deref()
            .unwrap()
            .starts_with(IRREVERSIBLE_REASON)
    );
    // With unmatched = pass the call isn't left to Claude Code either.
    let mut g = fixture(&[], &[], Unmatched::Pass);
    let id = g.inbox.push(None, "Bash", "git reset --hard HEAD~3");
    g.svc.pump();
    assert_eq!(verdict_of(&g, id), None);
    assert_eq!(g.svc.pending_count(), 1);
}

#[test]
fn only_the_typed_confirmation_allows_an_irreversible_call() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let (id, challenge) = held_irreversible(&mut f, Some(JobId(1)));

    // A key press, "always", an accepted suggestion are refused or don't allow it.
    assert!(matches!(
        f.svc.approve(id),
        Err(AppError::NeedsTypedConfirmation)
    ));
    assert!(matches!(f.svc.always_allow(id), Err(AppError::Guard(_))));
    let rule = f.svc.allow_rule("Bash(rm:*)", id).unwrap();
    assert_eq!(rule.rule, "Bash(rm:*)");
    assert_eq!(
        verdict_of(&f, id),
        None,
        "a rule made from it still doesn't allow it"
    );
    assert_eq!(f.svc.pending_count(), 1);

    // Typed, but before the prompt armed.
    let typed = Confirmation::Typed("build".into());
    assert!(f.svc.confirm_irreversible(&challenge, &typed).is_err());
    f.clock.advance(ARMING_MS);
    // The wrong target, a challenge for another call.
    let wrong = Confirmation::Typed("src".into());
    assert!(f.svc.confirm_irreversible(&challenge, &wrong).is_err());
    let forged = Challenge::new(id, "Bash", "rm -rf src", challenge.opened_at());
    assert!(f.svc.confirm_irreversible(&forged, &typed).is_err());
    let elsewhere = Challenge::new(id + 7, "Bash", "rm -rf build", challenge.opened_at());
    assert!(matches!(
        f.svc.confirm_irreversible(&elsewhere, &typed),
        Err(AppError::ApprovalNotFound)
    ));
    assert_eq!(verdict_of(&f, id), None);

    f.svc.confirm_irreversible(&challenge, &typed).unwrap();
    let answer = f.inbox.answer_to(id).unwrap();
    assert_eq!(answer.verdict, Verdict::Allow);
    assert_eq!(answer.rule.as_deref(), Some("confirmed by naming build"));
    let row = f.repo.rows().pop().unwrap();
    assert_eq!(row.extra.resolver.as_deref(), Some("human-typed"));
}

#[test]
fn a_picked_choice_confirms_too_and_other_calls_need_no_confirmation() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let (id, challenge) = held_irreversible(&mut f, None);
    f.clock.advance(ARMING_MS);
    let right = (1..=4)
        .find(|n| challenge.accepts_choice(*n, f.clock.now()))
        .unwrap();
    f.svc
        .confirm_irreversible(&challenge, &Confirmation::Choice(right))
        .unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    // An ordinary call can't be put through the confirmation.
    let plain = f.inbox.push(None, "Bash", "make");
    f.svc.pump();
    let c = Challenge::new(plain, "Bash", "make", 0);
    assert!(matches!(
        f.svc
            .confirm_irreversible(&c, &Confirmation::Typed("make".into())),
        Err(AppError::Guard(_))
    ));
}

#[test]
fn an_unanswered_irreversible_call_times_out_as_a_denial() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let (id, _) = held_irreversible(&mut f, None);
    f.clock.advance(120_000);
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
}

#[test]
fn no_automated_resolver_can_allow_an_irreversible_call() {
    // Whoever or whatever resolves a held irreversible call (a key press, a
    // rule, a timeout, a decider of a future extension, "always allow", an
    // accepted suggestion), the allow is refused and the call stays held.
    // Only the typed confirmation allows it (see
    // `only_the_typed_confirmation_allows_an_irreversible_call`).
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let (id, _) = held_irreversible(&mut f, Some(JobId(2)));
    for by in Resolver::ALL {
        if by == Resolver::HumanTyped {
            continue;
        }
        let how = Resolution::by(Verdict::Allow, "automated".into(), by);
        assert!(
            matches!(
                f.svc.resolve(id, how),
                Err(AppError::NeedsTypedConfirmation)
            ),
            "{} allowed an irreversible call",
            by.as_str()
        );
    }
    assert!(matches!(
        f.svc.approve(id),
        Err(AppError::NeedsTypedConfirmation)
    ));
    assert!(matches!(f.svc.always_allow(id), Err(AppError::Guard(_))));
    f.svc.allow_rule("Bash(rm:*)", id).unwrap();
    // A pump only expires and reloads; it does not decide.
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), None, "still held");
    assert_eq!(f.svc.pending_count(), 1);
    assert!(f.repo.rows().iter().all(|r| r.outcome != Outcome::Allow));
    // Every automated resolver may still deny it, and a timeout does.
    f.clock.advance(120_000);
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Deny));
    assert_eq!(f.svc.pending_count(), 0);
}

#[test]
fn always_allow_refuses_what_it_cannot_scope() {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let mcp = f.inbox.push(None, "mcp__github__create_issue", "");
    let search = f.inbox.push(None, "WebSearch", "rust");
    let compound = f.inbox.push(None, "Bash", "cargo build && cargo test");
    f.svc.pump();
    for id in [mcp, search, compound] {
        assert!(
            matches!(f.svc.always_allow(id), Err(AppError::Guard(_))),
            "{id}"
        );
        assert_eq!(verdict_of(&f, id), None, "the call stays held");
    }
    assert!(f.rules.rules().is_empty());
    // It can still be allowed once.
    f.svc.approve(mcp).unwrap();
    assert_eq!(verdict_of(&f, mcp), Some(Verdict::Allow));
    // And a rule that is the bare tool name can't be accepted either.
    assert!(matches!(
        f.svc.allow_rule("WebSearch", search),
        Err(AppError::Guard(_))
    ));
}

/// A repository that follows what the test sets, and a time that follows it.
struct Moving {
    repo: std::sync::Mutex<String>,
    now: std::sync::atomic::AtomicI64,
}

impl crate::ports::RepoRoots for Moving {
    fn repo_root(&self, _: Option<JobId>, _: &str) -> Option<String> {
        Some(self.repo.lock().unwrap().clone())
    }
    fn workspace_root(&self, _: Option<JobId>, _: &str) -> Option<String> {
        None
    }
    fn tree_root(&self, cwd: &str) -> Option<String> {
        Some(cwd.to_string())
    }
}

fn moving_fixture(always: AlwaysSettings) -> (Fixture, Arc<Moving>) {
    let moving = Arc::new(Moving {
        repo: std::sync::Mutex::new("/repo-a".into()),
        now: std::sync::atomic::AtomicI64::new(1_000_000),
    });
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let reader = moving.clone();
    let policy = SharedPolicy::default()
        .with_roots(moving.clone())
        .with_now(Arc::new(move || {
            reader.now.load(std::sync::atomic::Ordering::SeqCst)
        }));
    f.svc = GuardService::new(policy, f.inbox.clone(), f.repo.clone(), f.clock.clone())
        .with_rule_store(f.rules.clone())
        .with_always(always);
    (f, moving)
}

#[test]
fn an_always_allow_applies_in_its_own_repository_only() {
    let (mut f, moving) = moving_fixture(AlwaysSettings::default());
    let id = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert_eq!(done.scope, RuleScope::Repo);
    assert_eq!(done.root.as_deref(), Some("/repo-a"));
    let same = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, same), Some(Verdict::Allow));
    *moving.repo.lock().unwrap() = "/repo-b".into();
    let other = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, other), None, "another repository asks again");
    *moving.repo.lock().unwrap() = "/repo-a/".into();
    let back = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, back), Some(Verdict::Allow));
}

#[test]
fn a_global_scope_is_an_explicit_choice_and_applies_everywhere() {
    let always = AlwaysSettings {
        scope: RuleScope::Global,
        expiry_days: None,
    };
    let (mut f, moving) = moving_fixture(always);
    let id = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert_eq!(
        (done.scope, done.root, done.expires_at),
        (RuleScope::Global, None, None)
    );
    *moving.repo.lock().unwrap() = "/elsewhere".into();
    let other = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, other), Some(Verdict::Allow));
}

#[test]
fn an_always_allow_expires() {
    let (mut f, moving) = moving_fixture(AlwaysSettings {
        scope: RuleScope::Repo,
        expiry_days: Some(30),
    });
    let id = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert_eq!(done.expires_at, Some(1_000_000 + 30 * DAY_MS));
    let set = |t: i64| moving.now.store(t, std::sync::atomic::Ordering::SeqCst);
    set(1_000_000 + 30 * DAY_MS - 1);
    let before = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, before), Some(Verdict::Allow));
    set(1_000_000 + 30 * DAY_MS);
    let after = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, after), None, "expired: asked again");
}

#[test]
fn a_rule_removed_from_the_store_stops_applying() {
    let (mut f, _) = moving_fixture(AlwaysSettings::default());
    let id = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    let done = f.svc.always_allow(id).unwrap();
    assert!(f.svc.remove_rule(&done.id).unwrap());
    assert!(!f.svc.remove_rule(&done.id).unwrap());
    let again = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, again), None);
    assert!(f.rules.rules().is_empty());
}

#[test]
fn rules_kept_by_an_earlier_run_are_loaded_and_another_process_can_remove_them() {
    let (mut f, _) = moving_fixture(AlwaysSettings::default());
    let stored = StoredRule::new(
        "Bash(make all)",
        RuleScope::Repo,
        Some("/repo-a"),
        RuleSource::AlwaysAllow,
        1_000_000,
        None,
    );
    crate::ports::RuleStore::add(&*f.rules, &stored).unwrap();
    assert!(f.svc.load_rules().unwrap().is_empty());
    assert_eq!(f.svc.rules().len(), 1);
    let id = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    // `swarmux guard rules remove` in another process; the next reload sees it.
    crate::ports::RuleStore::remove(&*f.rules, &stored.id).unwrap();
    f.clock.advance(3_000);
    let later = f.inbox.push(None, "Bash", "make all");
    f.svc.pump();
    assert_eq!(verdict_of(&f, later), None);
}

fn protected_fixture(unattended: Option<JobId>) -> Fixture {
    let mut f = fixture(&[], &[], Unmatched::Ask);
    let marks = UnattendedJobs::new();
    if let Some(job) = unattended {
        marks.mark(job);
    }
    let mut p = Protection::new(
        vec![
            "/home/u/.swarmux/swarmux.db".into(),
            "/home/u/.swarmux/config.toml".into(),
        ],
        "/home/u",
    );
    p.exe = Some("/usr/bin/swarmux".into());
    p.protected_globs = swarmux_domain::AGENT_CONFIG_PROTECTED
        .iter()
        .map(|g| g.to_string())
        .collect();
    let policy = SharedPolicy::new(GuardPolicy::new(
        ["Edit(**)", "Write(**)", "Bash", "Read(**)"]
            .iter()
            .map(|r| Pattern::parse(r).unwrap())
            .collect(),
        vec![],
        Unmatched::Ask,
    ))
    .with_roots(Arc::new(FixedRoots::repo("/w")))
    .with_unattended(marks)
    .with_protection(p);
    f.svc = GuardService::new(policy, f.inbox.clone(), f.repo.clone(), f.clock.clone());
    f
}

#[test]
fn an_agent_may_not_run_the_owners_swarmux_commands() {
    let mut f = protected_fixture(None);
    let audit = f.inbox.push(None, "Bash", "swarmux audit checkpoint");
    let statusline = f.inbox.push(None, "Bash", "swarmux statusline");
    let hook = f.inbox.push(None, "Bash", "/usr/bin/swarmux hook pre-tool");
    f.svc.pump();
    assert_eq!(verdict_of(&f, audit), Some(Verdict::Deny));
    assert_eq!(verdict_of(&f, statusline), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, hook), Some(Verdict::Allow));
    let rule = f.repo.rows()[0].rule.clone().unwrap();
    assert!(rule.contains("`swarmux audit` is the owner's"), "{rule}");
}

#[test]
fn state_and_config_files_are_out_of_reach_even_with_allow_rules() {
    let mut f = protected_fixture(None);
    let db = f.inbox.push(None, "Read", "/home/u/.swarmux/swarmux.db");
    let cfg = f.inbox.push(None, "Edit", "/home/u/.swarmux/config.toml");
    let sh = f
        .inbox
        .push(None, "Bash", "cat ~/.swarmux/swarmux.db | head");
    f.svc.pump();
    for id in [db, cfg, sh] {
        assert_eq!(verdict_of(&f, id), Some(Verdict::Deny), "{id}");
    }
}

#[test]
fn writes_to_agent_config_files_are_held_for_the_owner_and_never_always_allowed() {
    let mut f = protected_fixture(None);
    let id = f.inbox.push(None, "Write", "/w/CLAUDE.md");
    f.svc.pump();
    assert_eq!(
        verdict_of(&f, id),
        None,
        "held, though Write(**) is allowed"
    );
    let row = f.repo.rows().pop().unwrap();
    assert!(row.rule.unwrap().starts_with(PROTECTED_REASON));
    assert!(matches!(f.svc.always_allow(id), Err(AppError::Guard(_))));
    f.svc.approve(id).unwrap();
    assert_eq!(verdict_of(&f, id), Some(Verdict::Allow));
    // Reading it, and writing code, are not held.
    let read = f.inbox.push(None, "Read", "/w/CLAUDE.md");
    let code = f.inbox.push(None, "Write", "/w/src/a.rs");
    f.svc.pump();
    assert_eq!(verdict_of(&f, read), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, code), Some(Verdict::Allow));
}

#[test]
fn an_unattended_job_is_denied_those_writes() {
    let mut f = protected_fixture(Some(JobId(9)));
    let mine = f
        .inbox
        .push(Some(JobId(9)), "Write", "/w/.claude/settings.json");
    let watched = f
        .inbox
        .push(Some(JobId(1)), "Write", "/w/.claude/settings.json");
    f.svc.pump();
    assert_eq!(verdict_of(&f, mine), Some(Verdict::Deny));
    assert_eq!(verdict_of(&f, watched), None, "a watched job is asked");
}

#[test]
fn subagent_calls_carry_who_made_them_into_the_record() {
    let mut f = fixture(&["Bash"], &[], Unmatched::Ask);
    f.inbox
        .push_by(Some(JobId(1)), "Bash", "ls", ("agent-7", "Explore"));
    f.svc.pump();
    let row = f.repo.rows().pop().unwrap();
    assert_eq!(row.extra.agent_id.as_deref(), Some("agent-7"));
    assert_eq!(row.extra.agent_type.as_deref(), Some("Explore"));
    f.inbox.push(Some(JobId(1)), "Bash", "ls");
    f.svc.pump();
    assert_eq!(f.repo.rows().pop().unwrap().extra.agent_id, None);
}

#[test]
fn allow_rules_written_in_config_stay_global_and_never_expire() {
    // Rules from before scopes and expiry: no repository, no end.
    let mut f = fixture(&["Bash(make all)"], &[], Unmatched::Ask);
    f.clock.advance(1_000 * DAY_MS);
    let here = f.inbox.push_in(None, "Bash", "make all", "/w");
    let elsewhere = f
        .inbox
        .push_in(None, "Bash", "make all", "/some/other/repo");
    f.svc.pump();
    assert_eq!(verdict_of(&f, here), Some(Verdict::Allow));
    assert_eq!(verdict_of(&f, elsewhere), Some(Verdict::Allow));
    let rule = f.repo.rows()[0].rule.clone().unwrap();
    assert_eq!(rule, "Bash(make all)");
}
