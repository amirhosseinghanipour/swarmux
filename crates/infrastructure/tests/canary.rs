//! The canaries end to end against the fake `claude`, which stands in for
//! both `claude` and the `swarmux` whose `hook canary` they run. The fake
//! behaves toward the hook as Claude Code 2.1.323 was seen to (see
//! `guide/claude-code.md`), so these tests show that the harness reads the
//! three things it looks at (the file, the hook's log, the tool result) and
//! judges them as it should. What the real Claude Code does is shown by
//! `swarmux doctor --conformance`, not here.
//!
//! Run with `cargo test -p swarmux-infrastructure --features fake-claude`.
#![cfg(feature = "fake-claude")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use swarmux_application::runtime::{Clearance, RuntimeGate, RuntimeSettings};
use swarmux_domain::runtime::{CheckStatus, RuntimeProfile};
use swarmux_infrastructure::claude_cli::runtime::canary::{CanarySetup, Scenario, run};
use swarmux_infrastructure::claude_cli::{ClaudeRuntime, LaunchSpec, Probes};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn setup(host: &str, on_failure_block: bool) -> CanarySetup {
    CanarySetup {
        bin: FAKE.into(),
        exe: FAKE.into(),
        sources: Some("user".into()),
        strict_mcp: true,
        on_failure_block,
        timeout: Duration::from_secs(30),
        env: vec![("FAKE_CLAUDE_HOOK_HOST".into(), host.into())],
    }
}

#[test]
fn a_host_that_honours_the_hook_passes_every_canary() {
    let s = setup("honor", true);
    let mut names = Vec::new();
    for scenario in Scenario::ALL {
        let check = run(&s, scenario);
        let expected = match scenario {
            // Seen on 2.1.323: invalid JSON does not stop the call, even
            // with onFailure=block, which is a note and not a failure.
            Scenario::HookInvalidJson => CheckStatus::Note,
            _ => CheckStatus::Pass,
        };
        assert_eq!(check.status, expected, "{scenario:?}: {}", check.detail);
        assert_eq!(check.required, scenario.required());
        names.push(check.name);
    }
    assert_eq!(
        names,
        [
            "deny",
            "hook_exit_1",
            "hook_timeout",
            "hook_invalid_json",
            "subagent"
        ]
    );
}

#[test]
fn a_host_that_never_asks_the_hook_fails_the_deny_canary() {
    // As when a managed setting drops the Guard's hook: the call runs.
    let check = run(&setup("ignore", true), Scenario::Deny);
    assert_eq!(check.status, CheckStatus::Fail, "{}", check.detail);
    assert!(check.detail.contains("the call ran"), "{}", check.detail);
}

#[test]
fn a_failing_hook_that_cannot_block_is_a_note_without_on_failure() {
    let check = run(&setup("honor", false), Scenario::HookExit1);
    assert_eq!(check.status, CheckStatus::Note, "{}", check.detail);
}

/// A runtime over the fake, the way `main` builds one.
fn runtime(host: &str) -> ClaudeRuntime {
    let settings = RuntimeSettings::default();
    let probes = Probes {
        version: swarmux_domain::runtime::Version::parse("2.1.323"),
        ..Probes::default()
    };
    let spec = LaunchSpec::default().resolve(&settings, &probes);
    let rt = ClaudeRuntime::new(FAKE.into(), FAKE.into(), settings, spec, probes);
    // The runtime runs the canary as it would in production; only the
    // environment that tells the fake how to behave is added.
    let host = host.to_string();
    rt.with_canary_runner(Arc::new(move |s, scenario| {
        let mut s = s.clone();
        s.env.push(("FAKE_CLAUDE_HOOK_HOST".into(), host.clone()));
        s.exe = FAKE.into();
        s.bin = FAKE.into();
        run(&s, scenario)
    }))
}

fn settled(rt: &ClaudeRuntime) -> Clearance {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        match rt.clearance(RuntimeProfile::Controlled) {
            Clearance::Pending if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

#[test]
fn a_runtime_clears_an_unattended_start_when_the_canary_passes() {
    let rt = runtime("honor");
    let Clearance::Cleared(Some(report)) = settled(&rt) else {
        panic!("the hook is honoured: the start is cleared")
    };
    assert!(report.passed(), "{}", report.refusal());
    let deny = report.checks.iter().find(|c| c.name == "deny").unwrap();
    assert_eq!(deny.status, CheckStatus::Pass);
}

#[test]
fn a_runtime_refuses_an_unattended_start_when_the_canary_fails() {
    let rt = runtime("ignore");
    let Clearance::Refused { why, report } = settled(&rt) else {
        panic!("the hook is not honoured: the start is refused")
    };
    assert!(why.contains("deny fail"), "{why}");
    assert!(!report.passed());
}
