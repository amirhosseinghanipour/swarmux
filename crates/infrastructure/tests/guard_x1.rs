//! Guard hardening end to end, over the real hook socket and SQLite:
//!
//! * a held irreversible call is released by the typed confirmation and by
//!   nothing else, even with a decision model answering "allow" with full
//!   confidence;
//! * a job whose app can't be reached is denied, and after a restart the
//!   fallback is in the ledger.
//!
//! The transcripts print with `--nocapture`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{TryRecvError, channel};
use std::time::{Duration, Instant};

use swarmux_application::ledger::{Ledger, LedgerService, ingest_fallbacks, verify_all};
use swarmux_application::ports::{
    Advice, AdviceAnswer, AdviceReply, AdviceRequest, AdviceSink, Clock, ConsentStore,
    DecisionAdvisor, DecisionRepository,
};
use swarmux_application::{
    AppResult, Confirmation, DecisionRecord, FailMode, GuardService, Jev, SharedPolicy,
};
use swarmux_domain::alarm::{ARMING_MS, Challenge};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{GuardPolicy, Pattern, Unmatched};
use swarmux_infrastructure::hook::{HookEnv, HookServer, endpoint_for, pre_tool_in, spool_file};
use swarmux_infrastructure::{SqliteLedger, SystemClock};

/// A decision model that always says "allow", and sure of it.
#[derive(Default)]
struct YesMan {
    asked: RefCell<Vec<String>>,
}

impl DecisionAdvisor for YesMan {
    fn provider(&self) -> &str {
        "typesafe"
    }
    fn host(&self) -> &str {
        "fake.example"
    }
    fn has_key(&self) -> bool {
        true
    }
    fn advise(&self, request: AdviceRequest, sink: AdviceSink) {
        if let AdviceRequest::ToolCall { summary, .. } = request {
            self.asked.borrow_mut().push(summary);
        }
        sink(AdviceReply {
            result: Ok(Advice {
                answer: AdviceAnswer::Allow,
                confidence: 1.0,
            }),
            latency_ms: 1,
        });
    }
}

struct Consent;

impl ConsentStore for Consent {
    fn has_consent(&self, _: &str) -> AppResult<bool> {
        Ok(true)
    }
    fn grant_consent(&self, _: &str, _: i64) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Log(RefCell<Vec<DecisionRecord>>);

impl DecisionRepository for Log {
    fn record(&self, decision: &DecisionRecord) -> AppResult<()> {
        self.0.borrow_mut().push(decision.clone());
        Ok(())
    }
    fn recent_decisions(&self, _: usize) -> AppResult<Vec<DecisionRecord>> {
        Ok(self.0.borrow().clone())
    }
}

fn hook_input(command: &str) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": "/work/app",
    })
    .to_string()
}

fn pump_until(guard: &mut GuardService, what: &str, done: impl Fn(&GuardService) -> bool) {
    let start = Instant::now();
    while !done(guard) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        guard.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_irreversible_call_waits_for_the_typed_confirmation_whatever_jev_says() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let pipe = endpoint_for(std::process::id() ^ 0x5151);
    // The owner allowed every shell command; that is not a confirmation.
    let policy = SharedPolicy::new(GuardPolicy::new(
        vec![Pattern::parse("Bash").unwrap()],
        vec![],
        Unmatched::Ask,
    ));
    let server = Rc::new(HookServer::start(rt.handle(), &pipe, policy.clone()).unwrap());
    let clock = Rc::new(SystemClock);
    let log = Rc::new(Log::default());
    let advisor = Rc::new(YesMan::default());
    let jev = Rc::new(Jev::new(advisor.clone(), Rc::new(Consent), clock.clone(), 0.85).unwrap());
    let mut guard = GuardService::new(policy, server.clone(), log.clone(), clock.clone())
        .with_timeout_secs(60)
        .with_jev(jev);

    // The hook process Claude Code would run, for `rm -rf build`.
    let (tx, rx) = channel();
    let env = HookEnv {
        pipe: Some(pipe.clone()),
        job: Some("7".into()),
        fail: FailMode::Closed,
        spool: None,
        now: clock.now(),
    };
    std::thread::spawn(move || {
        let _ = tx.send(pre_tool_in(&hook_input("rm -rf build"), &env));
    });

    pump_until(&mut guard, "the call is held for the owner", |g| {
        g.pending_count() == 1
    });
    let held = guard.approvals().remove(0);
    println!(
        "held:      Bash {}  (rule: {:?})",
        held.request.subject,
        log.0.borrow().last().and_then(|r| r.rule.clone())
    );
    println!("jev asked: {} time(s)", advisor.asked.borrow().len());
    assert!(
        advisor.asked.borrow().is_empty(),
        "JEV is never consulted about it"
    );

    // Nothing but the typed confirmation: a key, an allow rule, the model.
    let id = held.request.request_id;
    let key = guard.approve(id);
    println!("approve(): {key:?}");
    assert!(key.is_err());
    let always = guard.always_allow(id);
    println!("always():  {always:?}");
    assert!(always.is_err());
    guard.pump();
    assert_eq!(
        rx.try_recv(),
        Err(TryRecvError::Empty),
        "the agent is still waiting"
    );

    let challenge = Challenge::new(id, "Bash", &held.request.subject, clock.now());
    let early = guard.confirm_irreversible(&challenge, &Confirmation::Typed("build".into()));
    println!("typed before the prompt armed: {early:?}");
    assert!(early.is_err());
    std::thread::sleep(Duration::from_millis(ARMING_MS as u64 + 100));
    let wrong = guard.confirm_irreversible(&challenge, &Confirmation::Typed("src".into()));
    println!("typed the wrong target:        {wrong:?}");
    assert!(wrong.is_err());
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));

    guard
        .confirm_irreversible(&challenge, &Confirmation::Typed("build".into()))
        .unwrap();
    let out = rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .expect("the hook prints the decision");
    println!("typed `build`: the hook printed {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
    assert!(
        v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("confirmed by naming build")
    );
    assert!(advisor.asked.borrow().is_empty());
    let last = log.0.borrow().last().cloned().unwrap();
    assert_eq!(last.extra.resolver.as_deref(), Some("human-typed"));
}

#[test]
fn an_unattended_job_is_denied_when_the_app_is_unreachable_and_the_fallback_is_in_the_ledger_after_a_restart()
 {
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_file(dir.path());
    let env = HookEnv {
        // Nothing listens here: the app is not reachable.
        pipe: Some(endpoint_for(u32::MAX - 5)),
        job: Some("12".into()),
        fail: FailMode::Closed,
        spool: Some(spool.clone()),
        now: 1_700_000_123_000,
    };
    let out = pre_tool_in(&hook_input("cargo publish"), &env).expect("a denial is printed");
    println!("hook printed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(
        v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("unattended jobs fail closed")
    );

    // A job somebody watches is left to Claude Code, and the trace is left too.
    let open = HookEnv {
        fail: FailMode::Open,
        ..env.clone()
    };
    assert_eq!(pre_tool_in(&hook_input("ls"), &open), None);

    // The app starts again: the spool goes into the ledger.
    let db = dir.path().join("swarmux.db");
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&db).unwrap()),
        Rc::new(SystemClock),
    ));
    let n = ingest_fallbacks(
        &swarmux_infrastructure::hook::FileSpool::new(spool.clone()),
        &ledger,
    )
    .unwrap();
    assert_eq!(n, 2);
    assert!(!spool.exists(), "the spool is emptied");
    let raw = SqliteLedger::open(&db).unwrap();
    let report = verify_all(&raw).unwrap();
    assert!(report.is_ok(), "the chain holds");
    let entries = raw.range(0, 100).unwrap();
    let fallbacks: Vec<_> = entries
        .iter()
        .filter(|e| e.draft.kind == LedgerKind::Policy)
        .collect();
    for e in &fallbacks {
        println!(
            "ledger #{} {} {}",
            e.seq, e.draft.kind, e.draft.payload_json
        );
    }
    assert_eq!(fallbacks.len(), 2);
    assert!(
        fallbacks[0]
            .draft
            .payload_json
            .contains("\"event\":\"guard_fallback\"")
    );
    assert!(
        fallbacks[0]
            .draft
            .payload_json
            .contains("\"mode\":\"deny\"")
    );
    assert!(
        fallbacks[0]
            .draft
            .payload_json
            .contains("\"tool\":\"Bash\"")
    );
    assert!(
        fallbacks[1]
            .draft
            .payload_json
            .contains("\"mode\":\"pass\"")
    );
    assert_eq!(fallbacks[0].draft.job.map(|j| j.0), Some(12));
}
