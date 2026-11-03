//! The done-gate end to end, over the real hook socket, with real checks
//! run as child processes in a seeded repository:
//!
//! - an agent that says "done" while the check fails is blocked with the
//!   failure, and let go once the code is fixed and the run is shown;
//! - editing the oracle to make the check pass is refused by the Guard and
//!   logged, and an edit that slips past it is caught by the hash pin.

use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use swarmux_application::ledger::{Ledger, LedgerService, LedgeredDecisions};
use swarmux_application::{
    ExecutionLog, GateKeeper, GateService, GuardService, HookFeed, SharedPolicy,
};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::{GuardPolicy, HookEvent, JobId, Unmatched};
use swarmux_infrastructure::hook::{Responders, endpoint_for, run_hook};
use swarmux_infrastructure::{
    FsGateSource, HookServer, ProcessChecker, SqliteLedger, SqliteStore, SystemClock,
};

const JOB: &str = "7";

const GATES: &str = r#"
[gate]
max_blocks = 3
protected = ["tests/oracles/**"]

[[check]]
name = "answer"
run = ["git", "-c", "core.autocrlf=false", "diff", "--no-index", "--no-color", "tests/oracles/expected.txt", "src/answer.txt"]
timeout_secs = 60
"#;

/// A repository whose answer is wrong, judged by a protected oracle.
fn seed(root: &Path) {
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join(".swarmux")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests/oracles")).unwrap();
    fs::write(root.join(".swarmux/gates.toml"), GATES).unwrap();
    fs::write(root.join("tests/oracles/expected.txt"), "42\n").unwrap();
    fs::write(root.join("src/answer.txt"), "41\n").unwrap();
}

fn input(event: HookEvent, cwd: &str, extra: serde_json::Value) -> String {
    let mut v = serde_json::json!({
        "session_id": "claude-1",
        "transcript_path": "/t.jsonl",
        "cwd": cwd,
        "hook_event_name": event.as_str(),
    });
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    v.to_string()
}

struct World {
    _dir: tempfile::TempDir,
    root: String,
    pipe: String,
    keeper: Arc<GateKeeper>,
    server: Rc<HookServer>,
    gate: GateService,
    guard: GuardService,
    ledger: Rc<SqliteLedger>,
    _rt: tokio::runtime::Runtime,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    seed(&root);
    let db = dir.path().join("swarmux.db");
    let store = Rc::new(SqliteStore::open(&db).unwrap());
    let ledger = Rc::new(SqliteLedger::open(&db).unwrap());
    let clock = Rc::new(SystemClock);
    let service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));

    let (keeper, records) = GateKeeper::new(
        Arc::new(FsGateSource::new()),
        Arc::new(ProcessChecker::new()),
    );
    let keeper = Arc::new(keeper);
    let policy = SharedPolicy::new(GuardPolicy::new(vec![], vec![], Unmatched::Pass))
        .with_fence(keeper.clone());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let pipe = endpoint_for(uuid::Uuid::new_v4().as_u128() as u32);
    let server = Rc::new(
        HookServer::start_with(
            rt.handle(),
            &pipe,
            policy.clone(),
            Responders {
                stop: keeper.clone(),
                ..Responders::default()
            },
        )
        .unwrap(),
    );
    let decisions = Rc::new(LedgeredDecisions::new(store.clone(), service.clone()));
    let guard = GuardService::new(policy, server.clone(), decisions, clock.clone());
    let gate = GateService::new(records, service, store, clock);
    World {
        root: root.display().to_string(),
        _dir: dir,
        pipe,
        keeper,
        server,
        gate,
        guard,
        ledger,
        _rt: rt,
    }
}

impl World {
    fn hook(&self, event: HookEvent, extra: serde_json::Value) -> Option<serde_json::Value> {
        let stdin = input(event, &self.root, extra);
        run_hook(event, &stdin, Some(&self.pipe), Some(JOB))
            .map(|out| serde_json::from_str(&out).unwrap())
    }

    fn stop(&self, message: &str) -> Option<serde_json::Value> {
        self.hook(
            HookEvent::Stop,
            serde_json::json!({"last_assistant_message": message, "stop_hook_active": false}),
        )
    }

    fn pre_tool(&self, tool: &str, input: serde_json::Value) -> Option<serde_json::Value> {
        self.hook(
            HookEvent::PreToolUse,
            serde_json::json!({"tool_name": tool, "tool_input": input, "tool_use_id": "t"}),
        )
    }

    /// Asynchronous hooks are sent and forgotten: waits until the server
    /// has handled the last one (it hands the notice on after the gate saw
    /// it).
    fn settle(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !self
            .server
            .poll_notices()
            .iter()
            .any(|n| n.event == HookEvent::PostToolUse)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the hook never arrived"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn write(&self, rel: &str, text: &str) {
        fs::write(Path::new(&self.root).join(rel), text).unwrap();
    }

    fn verdicts(&self) -> Vec<String> {
        self.ledger
            .range(1, 1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.draft.kind == LedgerKind::GateVerdict)
            .map(|e| e.draft.payload_json)
            .collect()
    }
}

fn reason(v: &Option<serde_json::Value>) -> String {
    let v = v.as_ref().expect("the hook printed an answer");
    assert_eq!(v["decision"], "block", "{v}");
    v["reason"].as_str().unwrap().to_string()
}

fn denial(v: &Option<serde_json::Value>) -> String {
    let v = v.as_ref().expect("the hook printed an answer");
    let out = &v["hookSpecificOutput"];
    assert_eq!(out["permissionDecision"], "deny", "{v}");
    out["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_premature_done_is_blocked_then_accepted_after_the_fix() {
    let mut w = world();
    w.hook(
        HookEvent::SessionStart,
        serde_json::json!({"source": "startup"}),
    );

    // 1. The agent edits the code, then says "done" while the answer is
    //    still wrong.
    w.hook(
        HookEvent::PostToolUse,
        serde_json::json!({"tool_name": "Edit",
            "tool_input": {"file_path": format!("{}/src/answer.txt", w.root)},
            "tool_response": {}}),
    );
    w.settle();
    let first = w.stop("Done. All tests pass.");
    let digest = reason(&first);
    println!("--- block 1 ---\n{digest}");
    assert!(
        digest.starts_with("swarmux gate: not done yet (block 1 of 3)."),
        "{digest}"
    );
    assert!(
        digest.contains("✗ answer: `git -c core.autocrlf=false diff --no-index"),
        "{digest}"
    );
    assert!(digest.contains("exited 1 after"), "{digest}");
    assert!(digest.contains("-42") && digest.contains("+41"), "{digest}");
    assert!(digest.contains("⚑ you said \"All tests pass\", but no test run succeeded"));
    assert!(digest.contains("Protected (writes are refused): .swarmux/**, tests/oracles/**"));

    // 2. The agent fixes the code and runs the check itself (the job runner
    //    reports what ran from the stream).
    w.write("src/answer.txt", "42\n");
    w.keeper.executed(
        JobId(7),
        "git -c core.autocrlf=false diff --no-index --no-color tests/oracles/expected.txt src/answer.txt",
        true,
    );
    let second = w.stop("Fixed the answer. All tests pass.");
    assert_eq!(second, None, "a pass prints nothing: the agent may stop");

    let (any, errors) = w.gate.pump();
    assert!(any && errors.is_empty(), "{errors:?}");
    let state = w.gate.state(JobId(7)).unwrap();
    assert_eq!(state.checks, [("answer".to_string(), true, false)]);
    let verdicts = w.verdicts();
    assert_eq!(verdicts.len(), 2);
    assert!(
        verdicts[0].contains(r#""outcome":"block""#),
        "{}",
        verdicts[0]
    );
    assert!(verdicts[0].contains(r#""exit":1"#));
    assert!(
        verdicts[1].contains(r#""outcome":"pass""#),
        "{}",
        verdicts[1]
    );
    assert!(
        swarmux_application::ledger::verify_all(&*w.ledger)
            .unwrap()
            .is_ok()
    );
}

#[test]
fn editing_the_test_to_pass_is_denied_and_a_slipped_edit_is_caught() {
    let mut w = world();
    w.hook(
        HookEvent::SessionStart,
        serde_json::json!({"source": "startup"}),
    );
    let oracle = Path::new(&w.root).join("tests/oracles/expected.txt");
    let oracle = oracle.display().to_string();

    // The Guard refuses every way of writing the oracle…
    let edit = w.pre_tool(
        "Edit",
        serde_json::json!({"file_path": oracle, "old_string": "42", "new_string": "41"}),
    );
    let why = denial(&edit);
    println!("--- edit ---\n{why}");
    assert!(
        why.contains("protected path tests/oracles/** (gate)"),
        "{why}"
    );
    let shell = w.pre_tool(
        "Bash",
        serde_json::json!({"command": "echo 41 > tests/oracles/expected.txt"}),
    );
    assert!(denial(&shell).contains("protected path tests/oracles/**"));
    let gates = w.pre_tool(
        "Write",
        serde_json::json!({"file_path": format!("{}/.swarmux/gates.toml", w.root), "content": ""}),
    );
    assert!(denial(&gates).contains("protected path .swarmux/**"));
    // …but reading and running it is fine, and so is editing the code.
    for (tool, input) in [
        ("Read", serde_json::json!({"file_path": oracle})),
        (
            "Bash",
            serde_json::json!({"command": "cat tests/oracles/expected.txt"}),
        ),
        (
            "Edit",
            serde_json::json!({"file_path": format!("{}/src/answer.txt", w.root)}),
        ),
    ] {
        let answer = w.pre_tool(tool, input.clone());
        let denied = answer
            .as_ref()
            .is_some_and(|v| v["hookSpecificOutput"]["permissionDecision"] == "deny");
        assert!(!denied, "{tool} {input} was refused: {answer:?}");
    }
    // The refusals are logged as guard decisions, in the ledger too.
    let report = w.guard.pump();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let denials: Vec<String> = w
        .ledger
        .range(1, 1000)
        .unwrap()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::GuardDecision)
        .map(|e| e.draft.payload_json)
        .filter(|p| p.contains(r#""outcome":"deny""#))
        .collect();
    assert_eq!(denials.len(), 3, "{denials:?}");
    assert!(denials[0].contains("protected path tests/oracles/** (gate)"));

    // An edit that slips past the Guard (a script writing the file) makes
    // the check pass, but the pin taken at session start catches it.
    let script = r#"python -c "open('tests/oracles/expected.txt','w').write('41')""#;
    assert!(
        w.pre_tool("Bash", serde_json::json!({"command": script}))
            .is_none(),
        "the fence can't see inside a script"
    );
    w.write("tests/oracles/expected.txt", "41\n");
    w.hook(
        HookEvent::PostToolUse,
        serde_json::json!({"tool_name": "Bash", "tool_input": {"command": script},
            "tool_response": {}}),
    );
    w.settle();
    let digest = reason(&w.stop("Done."));
    println!("--- slipped edit ---\n{digest}");
    assert!(
        digest.contains("⛔ protected file changed: tests/oracles/expected.txt"),
        "{digest}"
    );
    assert!(
        !digest.contains("✗ answer"),
        "the doctored oracle passes the check"
    );

    // Put back and fixed properly: accepted.
    w.write("tests/oracles/expected.txt", "42\n");
    w.write("src/answer.txt", "42\n");
    assert_eq!(w.stop("Fixed src/answer.txt."), None);
    w.gate.pump();
    let verdicts = w.verdicts();
    assert_eq!(verdicts.len(), 2, "{verdicts:#?}");
    assert!(verdicts[0].contains(r#""protected_changed":["changed tests/oracles/expected.txt"]"#));
    assert!(verdicts[1].contains(r#""outcome":"pass""#));
}
