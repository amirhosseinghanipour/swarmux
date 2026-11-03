use super::*;
use crate::ports::SessionRepository;
use crate::testing::{FakeHookFeed, FixedClock, InMemoryLedger, InMemoryStore};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{Session, SessionId};

struct Fixture {
    bus: HookBus,
    feed: Rc<FakeHookFeed>,
    ledger: Rc<InMemoryLedger>,
    clock: Rc<FixedClock>,
}

fn fixture() -> Fixture {
    let store = Rc::new(InMemoryStore::default());
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    let ledger = Rc::new(InMemoryLedger::default());
    let clock = Rc::new(FixedClock::new(5_000));
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
    let feed = Rc::new(FakeHookFeed::default());
    let bus = HookBus::new(feed.clone(), service, store, clock.clone());
    Fixture {
        bus,
        feed,
        ledger,
        clock,
    }
}

fn notice(event: HookEvent, job: Option<i64>) -> HookNotice {
    HookNotice {
        event,
        job: job.map(JobId),
        session: "claude-session".into(),
        cwd: "/w".into(),
        tool: None,
        summary: format!("{} happened", event.as_str()),
        detail: String::new(),
        input_sha256: "ab".repeat(32),
        input_len: 42,
        raw: String::new(),
    }
}

#[test]
fn every_event_lands_in_the_ledger_as_an_agent_event() {
    let mut f = fixture();
    for e in HookEvent::ALL {
        f.feed.push(notice(e, Some(1)));
    }
    assert!(f.bus.pump().is_empty());
    let entries = f.ledger.entries();
    let hooks: Vec<_> = entries
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::AgentEvent)
        .collect();
    // PreToolUse is the Guard's.
    assert_eq!(hooks.len(), HookEvent::ALL.len() - 1);
    assert!(hooks.iter().all(|e| e.draft.actor == "hook"));
    let p = &hooks[0].draft.payload_json;
    assert!(p.contains(r#""event":"PostToolUse""#), "{p}");
    assert!(p.contains(r#""session":"claude-session""#), "{p}");
    assert!(p.contains(r#""input_len":42"#), "{p}");
}

#[test]
fn signals_follow_the_events() {
    let mut f = fixture();
    let mut post = notice(HookEvent::PostToolUse, Some(3));
    post.tool = Some("Bash".into());
    let mut denied = notice(HookEvent::PermissionDenied, Some(3));
    denied.tool = Some("Write".into());
    let mut fail = notice(HookEvent::StopFailure, Some(3));
    fail.detail = "rate_limit".into();
    let mut ask = notice(HookEvent::Notification, Some(3));
    ask.detail = "agent_needs_input".into();
    let mut chatter = notice(HookEvent::Notification, Some(3));
    chatter.detail = "auth_success".into();
    for n in [
        post,
        notice(HookEvent::PreCompact, Some(3)),
        notice(HookEvent::PreCompact, Some(3)),
        denied,
        fail,
        ask,
        chatter,
    ] {
        f.feed.push(n);
    }
    f.clock.advance(10);
    f.bus.pump();
    let s = f.bus.signals(JobId(3)).unwrap();
    assert_eq!(s.compactions, 2);
    assert_eq!(s.permission_denied_count, 1);
    assert_eq!(s.last_tool.as_deref(), Some("Write"));
    assert_eq!(s.last_stop_failure.as_deref(), Some("rate_limit"));
    assert_eq!(s.needs_input, 1);
    assert_eq!(s.last_event_at, Some(5_010));
    assert_eq!(f.bus.signals(JobId(9)), None);
}

#[test]
fn denials_and_stop_failures_reach_the_changes_feed_only() {
    let mut f = fixture();
    let mut denied = notice(HookEvent::PermissionDenied, Some(1));
    denied.summary = "Bash rm -rf x".into();
    f.feed.push(denied);
    let mut fail = notice(HookEvent::StopFailure, None);
    fail.detail = "overloaded".into();
    f.feed.push(fail);
    f.feed.push(notice(HookEvent::PostToolUse, Some(1)));
    f.bus.pump();
    let changes = f.bus.changes();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].kind, ChangeKind::Denied);
    assert!(changes[0].text.contains("Bash rm -rf x"));
    assert_eq!(changes[1].kind, ChangeKind::Failed);
    assert!(changes[1].text.contains("overloaded"));
}

#[test]
fn events_without_a_job_are_still_recorded() {
    let mut f = fixture();
    f.feed.push(notice(HookEvent::Stop, None));
    assert!(f.bus.pump().is_empty());
    assert_eq!(f.ledger.entries().len(), 2); // genesis + the event
}

#[test]
fn default_seams_allow_and_add_nothing() {
    let stop = StopContext {
        event: HookEvent::Stop,
        job: None,
        session: String::new(),
        cwd: String::new(),
        last_message: String::new(),
    };
    assert_eq!(AllowStop.on_stop(&stop), StopAnswer::Allow);
    let start = SessionStartInfo {
        job: None,
        session: String::new(),
        source: "startup".into(),
    };
    assert_eq!(NoSessionContext.context(&start), None);
}

#[test]
fn loaded_instruction_files_are_kept_per_session() {
    let f = fixture();
    let board = Rc::new(TruthBoard::new(f.clock.clone()));
    let mut bus = f.bus.with_truth(board.clone());
    for (path, detail) in [
        (
            "/w/CLAUDE.md",
            InstructionFile::detail("session_start", "Project"),
        ),
        (
            "/home/.claude/CLAUDE.md",
            InstructionFile::detail("session_start", "User"),
        ),
    ] {
        let mut n = notice(HookEvent::InstructionsLoaded, Some(3));
        n.summary = path.into();
        n.detail = detail;
        f.feed.push(n);
    }
    assert!(bus.pump().is_empty());
    let views = board.instructions();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].job, Some(JobId(3)));
    assert_eq!(views[0].files.len(), 2);
    assert_eq!(views[0].files[0].path, "/w/CLAUDE.md");
    assert_eq!(views[0].files[1].memory_type, "User");
}
