use std::rc::Rc;

use swarmux_domain::budget::Admission;
use swarmux_domain::telemetry::{DAY, HOUR, LimitState, MINUTE, RateLimit};
use swarmux_domain::{AgentEvent, JobId, JobStatus, Limits, Session, SessionId};

use super::*;
use crate::JobService;
use crate::ports::{RunnerMsg, SessionRepository};
use crate::testing::{
    FakeMonitor, FakeRunner, FakeUsageProbe, FakeWorkspace, FixedClock, InMemoryResumeBook,
    InMemoryStore,
};

/// 2026-11-03 12:00 UTC.
const T0: i64 = 1_793_793_600_000;

fn reading(window: &str, used: f64, resets_at: i64) -> RateLimit {
    RateLimit {
        window: window.into(),
        status: if used >= 1.0 { "rejected" } else { "allowed" }.into(),
        utilization: Some(used),
        resets_at: Some(resets_at),
        surpassed_threshold: None,
    }
}

fn window(name: &str, used: f64, resets_at: i64, at: i64) -> LimitState {
    LimitState::new(reading(name, used, resets_at), at)
}

struct Rig {
    svc: JobService,
    store: Rc<InMemoryStore>,
    runner: Rc<FakeRunner>,
    clock: Rc<FixedClock>,
    probe: Rc<FakeUsageProbe>,
    monitor: Rc<FakeMonitor>,
    book: Rc<InMemoryResumeBook>,
    session: Session,
}

fn rig_with(settings: GovernorSettings) -> Rig {
    let store = Rc::new(InMemoryStore::default());
    let runner = Rc::new(FakeRunner::default());
    let clock = Rc::new(FixedClock::new(T0));
    let probe = Rc::new(FakeUsageProbe::default());
    let monitor = Rc::new(FakeMonitor::default());
    let book = Rc::new(InMemoryResumeBook::default());
    let session = Session::new(SessionId::random(), "demo", "/w".into(), "opus", 0).unwrap();
    SessionRepository::insert(&*store, &session).unwrap();
    let governor = Governor::new(&settings)
        .with_probe(probe.clone())
        .with_resume_book(book.clone());
    let svc = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone())
        .with_monitor(monitor.clone())
        .with_limits(Limits {
            max_concurrent: 4,
            min_free_ram: 0,
        })
        .with_governor(governor);
    Rig {
        svc,
        store,
        runner,
        clock,
        probe,
        monitor,
        book,
        session,
    }
}

fn rig() -> Rig {
    rig_with(GovernorSettings::default())
}

fn session(store: &InMemoryStore, name: &str) -> Session {
    let s = Session::new(SessionId::random(), name, "/w".into(), "sonnet", 0).unwrap();
    SessionRepository::insert(store, &s).unwrap();
    s
}

// --- the governor alone -------------------------------------------------

#[test]
fn freshest_reading_of_each_window_wins() {
    let probe = Rc::new(FakeUsageProbe::default());
    probe.set(vec![
        window("five_hour", 0.5, T0 + HOUR, T0 + 10),
        window("seven_day", 0.2, T0 + DAY, T0 - 10),
    ]);
    let g = Governor::new(&GovernorSettings::default()).with_probe(probe);
    let stream = [
        window("five_hour", 0.4, T0 + HOUR, T0),
        window("seven_day", 0.3, T0 + DAY, T0),
    ];
    let refs: Vec<&LimitState> = stream.iter().collect();
    let mut merged = g.windows(&refs);
    merged.sort_by(|a, b| a.limit.window.cmp(&b.limit.window));
    let used: Vec<(String, Option<f64>)> = merged
        .iter()
        .map(|w| (w.limit.window.clone(), w.limit.utilization))
        .collect();
    assert_eq!(
        used,
        [
            ("five_hour".to_string(), Some(0.5)),
            ("seven_day".to_string(), Some(0.3))
        ]
    );
}

#[test]
fn switched_off_admits_everything_at_full_concurrency() {
    let g = Governor::new(&GovernorSettings {
        enabled: false,
        ..GovernorSettings::default()
    });
    let w = [window("five_hour", 0.99, T0 + HOUR, T0)];
    assert_eq!(g.admit(&w, "opus", JobClass::Work, T0), Admission::Admit);
    let base = Limits {
        max_concurrent: 5,
        min_free_ram: 0,
    };
    assert_eq!(g.limits(base, true).max_concurrent, 5);
    // The line still carries a cap, not the windows.
    assert_eq!(
        g.line(&w, Some(1.5), T0).as_deref(),
        Some("[budget] up to $1.50 for this run")
    );
}

#[test]
fn resets_and_resumes_survive_a_restart() {
    let book = Rc::new(InMemoryResumeBook::default());
    let mut g = Governor::new(&GovernorSettings::default()).with_resume_book(book.clone());
    g.resume_at(JobId(7), T0 + HOUR, "5h").unwrap();
    g.resume_at(JobId(8), T0 + 2 * HOUR, "7d").unwrap();
    let mut again = Governor::new(&GovernorSettings::default()).with_resume_book(book.clone());
    again.restore().unwrap();
    assert_eq!(again.pending().len(), 2);
    assert!(again.due(T0).unwrap().is_empty());
    let due = again.due(T0 + HOUR).unwrap();
    assert_eq!(due.iter().map(|p| p.job).collect::<Vec<_>>(), [JobId(7)]);
    assert_eq!(book.pending().unwrap().len(), 1);
    again.forget(JobId(8)).unwrap();
    assert!(book.pending().unwrap().is_empty());
}

#[test]
fn reset_comes_from_the_refusing_window_else_the_held_one() {
    let g = Governor::new(&GovernorSettings::default());
    let refused = reading("five_hour", 1.0, T0 + HOUR);
    assert_eq!(
        g.reset_for(Some(&refused), &[], T0),
        Some((T0 + HOUR, "5h".to_string()))
    );
    let w = [
        window("five_hour", 0.5, T0 + HOUR, T0),
        window("seven_day", 0.98, T0 + DAY, T0),
    ];
    assert_eq!(
        g.reset_for(None, &w, T0),
        Some((T0 + DAY, "7d".to_string()))
    );
    assert_eq!(g.reset_for(None, &w[..1], T0), None);
}

// --- the governor in the queue ------------------------------------------

#[test]
fn deferred_work_shows_why_and_starts_by_itself_after_the_reset() {
    let mut r = rig();
    let reset = T0 + 3 * HOUR + 58 * MINUTE;
    r.probe
        .set(vec![window("five_hour", 0.94, reset, T0 - MINUTE)]);
    let settings = GovernorSettings {
        hold_at: 0.94,
        ..GovernorSettings::default()
    };
    r.svc = {
        let governor = Governor::new(&settings).with_probe(r.probe.clone());
        JobService::new(
            r.store.clone(),
            r.store.clone(),
            r.runner.clone(),
            r.clock.clone(),
        )
        .with_governor(governor)
    };
    let id = r.svc.dispatch(r.session.id, "build it").unwrap();
    assert_eq!(r.runner.started(), 0, "held at the threshold");
    let row = r.svc.queue().into_iter().find(|e| e.job_id == id).unwrap();
    assert_eq!(
        row.note.as_deref(),
        Some("deferred: 5h at 94%, resumes 15:58")
    );
    assert_eq!(r.svc.stats().deferred, 1);

    // Still held a minute before the reset.
    r.clock.advance(reset - T0 - MINUTE);
    r.svc.pump();
    assert_eq!(r.runner.started(), 0);

    // The reset passes: no new reading needed, the job starts.
    r.clock.advance(MINUTE);
    r.svc.pump();
    assert_eq!(r.runner.started(), 1);
    assert_eq!(r.store.status_of(id), JobStatus::Running);
    assert_eq!(r.svc.stats().deferred, 0);
}

#[test]
fn verification_runs_while_new_work_is_held() {
    let mut r = rig();
    r.svc = {
        let governor = Governor::new(&GovernorSettings::default()).with_probe(r.probe.clone());
        let review = crate::ReviewSettings {
            enabled: true,
            model: "haiku".into(),
            when: crate::ReviewWhen::Always,
        };
        JobService::new(
            r.store.clone(),
            r.store.clone(),
            r.runner.clone(),
            r.clock.clone(),
        )
        .with_review(Rc::new(FakeWorkspace::with_diff("+x")), &review)
        .with_governor(governor)
    };
    let work = r.svc.dispatch(r.session.id, "build it").unwrap();
    // The window fills up while the job runs.
    r.probe.set(vec![window("five_hour", 0.98, T0 + HOUR, T0)]);
    let id = r.runner.request(0).session_id.to_string();
    r.runner.emit(0, RunnerMsg::Event(init(&id)));
    r.runner.emit(0, RunnerMsg::Event(done()));
    r.svc.pump();
    assert_eq!(r.store.status_of(work), JobStatus::Done);
    // Its review is verification and runs up to the full window ...
    assert_eq!(r.runner.started(), 2);
    assert!(r.runner.request(1).review);
    // ... while new work waits.
    let other = session(&r.store, "other");
    r.svc.dispatch(other.id, "more work").unwrap();
    assert_eq!(r.runner.started(), 2);
    assert_eq!(r.svc.stats().deferred, 1);
}

#[test]
fn new_work_waits_while_the_user_is_over_the_load_line_and_starts_after() {
    let mut r = rig();
    r.svc.set_supervisor_load(Some(0.78));
    let id = r.svc.dispatch(r.session.id, "build it").unwrap();
    assert_eq!(r.runner.started(), 0, "held");
    let row = r.svc.queue().into_iter().find(|e| e.job_id == id).unwrap();
    assert_eq!(row.note.as_deref(), Some("held: you're at 78% load"));
    assert_eq!(r.svc.stats().held, 1);
    assert_eq!(r.svc.stats().deferred, 0, "not a usage hold");

    // At the line is not over it.
    r.svc.set_supervisor_load(Some(0.70));
    r.svc.pump();
    assert_eq!(r.runner.started(), 1);
    assert_eq!(r.store.status_of(id), JobStatus::Running);
    assert_eq!(r.svc.stats().held, 0);
}

#[test]
fn the_load_hold_spares_reviews_and_obeys_the_settings() {
    let g = Governor::new(&GovernorSettings::default());
    assert!(g.admit_under_load(&[], "opus", JobClass::Verify, Some(0.95), T0) == Admission::Admit);
    assert!(matches!(
        g.admit_under_load(&[], "opus", JobClass::Work, Some(0.95), T0),
        Admission::Hold { .. }
    ));
    assert_eq!(
        g.admit_under_load(&[], "opus", JobClass::Work, None, T0),
        Admission::Admit,
        "unknown load holds nothing"
    );
    let off = Governor::new(&GovernorSettings {
        load_hold_at: 1.0,
        ..GovernorSettings::default()
    });
    assert_eq!(
        off.admit_under_load(&[], "opus", JobClass::Work, Some(1.0), T0),
        Admission::Admit
    );
    // A window that already defers the job gets to say so first.
    let w = [window("five_hour", 0.99, T0 + HOUR, T0)];
    assert!(
        g.admit_under_load(&w, "opus", JobClass::Work, Some(0.95), T0)
            .is_deferred()
    );
    let disabled = Governor::new(&GovernorSettings {
        enabled: false,
        ..GovernorSettings::default()
    });
    assert_eq!(
        disabled.admit_under_load(&[], "opus", JobClass::Work, Some(0.99), T0),
        Admission::Admit
    );
}

fn init(session_id: &str) -> AgentEvent {
    AgentEvent::Init {
        session_id: session_id.into(),
        model: "claude-opus".into(),
        cwd: "/w".into(),
        permission_mode: "default".into(),
    }
}

fn done() -> AgentEvent {
    AgentEvent::Result {
        is_error: false,
        subtype: "success".into(),
        cost_usd: 0.1,
        duration_ms: 900,
        num_turns: 1,
        tokens_in: 1,
        tokens_out: 2,
        cache_read: 0,
        cache_creation: None,
        context_window: Some(200_000),
        subagents_spawned: 0,
        text: "done".into(),
    }
}

#[test]
fn a_limit_hit_job_resumes_after_the_reset() {
    let mut r = rig();
    let id = r.svc.dispatch(r.session.id, "long task").unwrap();
    assert_eq!(r.runner.started(), 1);
    r.runner
        .emit(0, RunnerMsg::Event(init(&r.session.id.to_string())));
    let reset = T0 + 2 * HOUR;
    r.runner.emit(
        0,
        RunnerMsg::Event(AgentEvent::RateLimit(reading("five_hour", 1.0, reset))),
    );
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(1),
            stderr_tail: "Claude AI usage limit reached".into(),
        },
    );
    r.svc.pump();
    assert_eq!(r.store.status_of(id), JobStatus::Failed);
    assert_eq!(r.svc.stats().awaiting_reset, 1);
    assert_eq!(r.book.pending().unwrap().len(), 1);

    r.clock.advance(HOUR);
    r.svc.pump();
    assert_eq!(r.runner.started(), 1, "not before the reset");

    r.clock.advance(HOUR);
    r.svc.pump();
    assert_eq!(r.runner.started(), 2, "resumed after the reset");
    let resumed = r.runner.request(1);
    assert!(resumed.resume, "continues the Claude session");
    assert!(resumed.prompt.starts_with("The previous run stopped"));
    assert_eq!(r.svc.stats().awaiting_reset, 0);
    assert!(r.book.pending().unwrap().is_empty());
    let log = &r.svc.progress(resumed.job_id).unwrap().log;
    assert!(
        log.iter().any(|l| l.contains("after the 5h reset")),
        "{log:?}"
    );
}

#[test]
fn a_failure_without_a_limit_is_not_resumed() {
    let mut r = rig();
    r.svc.dispatch(r.session.id, "task").unwrap();
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(1),
            stderr_tail: "boom".into(),
        },
    );
    r.svc.pump();
    assert_eq!(r.svc.stats().awaiting_reset, 0);
}

#[test]
fn resumes_pending_at_quit_come_back_after_a_restart() {
    let mut r = rig();
    let job = r.svc.dispatch(r.session.id, "task").unwrap();
    r.runner
        .emit(0, RunnerMsg::Event(init(&r.session.id.to_string())));
    let refused = reading("seven_day", 1.0, T0 + 5 * HOUR);
    r.runner
        .emit(0, RunnerMsg::Event(AgentEvent::RateLimit(refused)));
    r.runner.emit(
        0,
        RunnerMsg::Exited {
            code: Some(1),
            stderr_tail: String::new(),
        },
    );
    r.svc.pump();
    drop(r.svc);

    // A new run over the same database.
    let governor = Governor::new(&GovernorSettings::default()).with_resume_book(r.book.clone());
    let mut svc = JobService::new(
        r.store.clone(),
        r.store.clone(),
        r.runner.clone(),
        r.clock.clone(),
    )
    .with_governor(governor);
    svc.recover().unwrap();
    assert_eq!(svc.stats().awaiting_reset, 1);
    svc.pump();
    assert_eq!(r.runner.started(), 1);
    r.clock.advance(5 * HOUR);
    svc.pump();
    assert_eq!(r.runner.started(), 2);
    assert_eq!(r.store.status_of(job), JobStatus::Failed);
    assert!(r.runner.request(1).resume);
}

#[test]
fn jobs_swarmux_stops_resume_at_the_next_start_and_queued_ones_wait() {
    let mut r = rig();
    let running = r.svc.dispatch(r.session.id, "task").unwrap();
    r.runner
        .emit(0, RunnerMsg::Event(init(&r.session.id.to_string())));
    r.svc.pump();
    let other = session(&r.store, "other");
    // Stopping: nothing new starts, and what runs is stopped.
    r.svc.set_draining(true);
    let waiting = r.svc.dispatch(other.id, "later").unwrap();
    assert_eq!(r.runner.started(), 1, "nothing starts while draining");
    assert_eq!(r.svc.interrupt_running().unwrap(), [running]);
    assert_eq!(r.runner.cancelled(), [running]);
    assert_eq!(r.store.status_of(running), JobStatus::Interrupted);
    assert_eq!(r.store.status_of(waiting), JobStatus::Queued);
    drop(r.svc);

    // The next start picks both up: the stopped job in its session.
    let governor = Governor::new(&GovernorSettings::default()).with_resume_book(r.book.clone());
    let mut svc = JobService::new(
        r.store.clone(),
        r.store.clone(),
        r.runner.clone(),
        r.clock.clone(),
    )
    .with_limits(Limits {
        max_concurrent: 4,
        min_free_ram: 0,
    })
    .with_governor(governor);
    svc.recover().unwrap();
    svc.pump();
    assert_eq!(r.runner.started(), 3);
    let resumed = r.runner.request(2);
    assert!(resumed.resume);
    assert_eq!(resumed.session_id, r.session.id);
    assert!(
        svc.progress(resumed.job_id)
            .unwrap()
            .log
            .iter()
            .any(|l| l.contains("after swarmux restarted"))
    );
    assert_eq!(svc.stats().awaiting_reset, 0);
}

#[test]
fn pricey_work_is_downgraded_near_the_limit() {
    let mut r = rig();
    r.probe.set(vec![window("seven_day", 0.92, T0 + DAY, T0)]);
    let id = r.svc.dispatch(r.session.id, "big job").unwrap();
    assert_eq!(r.runner.started(), 1);
    assert_eq!(r.runner.request(0).model, "sonnet");
    let row = r.svc.queue().into_iter().find(|e| e.job_id == id).unwrap();
    assert_eq!(row.note.as_deref(), Some("opus → sonnet: 7d at 92%"));
    assert_eq!(row.model, "sonnet");
}

#[test]
fn battery_runs_two_at_once() {
    let mut r = rig();
    r.monitor.set_battery(Some(true));
    for i in 0..4 {
        let s = session(&r.store, &format!("s{i}"));
        r.svc.dispatch(s.id, "work").unwrap();
    }
    assert_eq!(r.runner.started(), 2);
    let stats = r.svc.stats();
    assert!(stats.on_battery);
    assert_eq!(stats.max_concurrent, 2);
    // Back on mains (read again after 30 s): the rest start.
    r.monitor.set_battery(Some(false));
    r.clock.advance(31_000);
    r.svc.pump();
    assert_eq!(r.runner.started(), 4);
}

#[test]
fn jobs_are_told_their_budget_and_capped() {
    let mut r = rig_with(GovernorSettings {
        max_job_usd: Some(3.0),
        ..GovernorSettings::default()
    });
    r.probe
        .set(vec![window("five_hour", 0.62, T0 + 2 * HOUR, T0)]);
    r.svc.dispatch(r.session.id, "plain").unwrap();
    let req = r.runner.request(0);
    assert_eq!(req.max_budget_usd, Some(3.0));
    assert_eq!(
        req.prompt,
        "plain\n\n[budget] up to $3.00 for this run · 5h 62%, resets 14:00 · new work holds at 97%"
    );
    // A mission-like cap wins over the default.
    let s = session(&r.store, "m");
    r.svc.dispatch_capped(s.id, "capped", Some(1.25)).unwrap();
    assert_eq!(r.runner.request(1).max_budget_usd, Some(1.25));
    // The stored prompt stays as written.
    let job = r.runner.request(1).job_id;
    assert_eq!(r.store.job_of(job).prompt, "capped");
}

#[test]
fn no_line_when_switched_off() {
    let mut r = rig_with(GovernorSettings {
        budget_line: false,
        ..GovernorSettings::default()
    });
    r.probe.set(vec![window("five_hour", 0.5, T0 + HOUR, T0)]);
    r.svc.dispatch(r.session.id, "plain").unwrap();
    assert_eq!(r.runner.request(0).prompt, "plain");
    assert_eq!(r.runner.request(0).max_budget_usd, None);
}
