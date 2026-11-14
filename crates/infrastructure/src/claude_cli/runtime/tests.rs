use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use swarmux_application::runtime::{
    CanaryMode, Clearance, LaunchIntent, OnError, RuntimeGate, RuntimeSettings,
};
use swarmux_domain::JobId;
use swarmux_domain::runtime::{
    CanaryCheck, CheckStatus, HostPolicy, RuntimeProfile, Version, VersionStanding,
};
use swarmux_domain::telemetry::{Environment, McpServer};

use super::canary::Scenario;
use super::launch::{LaunchSpec, Probes};
use super::*;
use crate::claude_cli::{BoardMcp, GuardHook};

fn probes(version: &str) -> Probes {
    Probes {
        version: Version::parse(version),
        host: HostPolicy::default(),
        user_statusline: false,
    }
}

/// A runtime whose canary says `answer`, and how many times it was asked.
fn runtime(
    settings: RuntimeSettings,
    probes: Probes,
    answer: CheckStatus,
) -> (ClaudeRuntime, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    let spec = LaunchSpec {
        guard: Some(GuardHook {
            exe: "/bin/swarmux".into(),
            pipe: "p".into(),
            timeout_secs: 130,
        }),
        ..LaunchSpec::default()
    };
    let rt = ClaudeRuntime::new(
        "claude".into(),
        "/bin/swarmux".into(),
        settings,
        spec,
        probes,
    )
    .with_canary_runner(Arc::new(move |_, scenario| {
        counter.fetch_add(1, Ordering::SeqCst);
        CanaryCheck {
            name: scenario.name().into(),
            status: answer,
            detail: format!("{answer:?}"),
            required: scenario.required(),
        }
    }));
    (rt, asked)
}

/// Asks until the background canary has answered.
fn settled(rt: &ClaudeRuntime, profile: RuntimeProfile) -> Clearance {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match rt.clearance(profile) {
            Clearance::Pending if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

#[test]
fn an_unattended_start_waits_for_the_canary_and_then_goes() {
    let (rt, asked) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Pass,
    );
    assert_eq!(rt.clearance(RuntimeProfile::Controlled), Clearance::Pending);
    let Clearance::Cleared(Some(report)) = settled(&rt, RuntimeProfile::Controlled) else {
        panic!("the canary passed: the start is cleared with its report")
    };
    assert!(report.passed());
    let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["version", "host_policy", "deny"]);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    // The next start inside the reuse window shares the result, and the
    // record of it is made once.
    assert_eq!(
        rt.clearance(RuntimeProfile::Controlled),
        Clearance::Cleared(None)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
}

#[test]
fn a_failed_canary_refuses_the_start_and_says_why() {
    let (rt, _) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Fail,
    );
    let Clearance::Refused { why, report } = settled(&rt, RuntimeProfile::Controlled) else {
        panic!("a failed deny canary refuses")
    };
    assert!(why.contains("deny fail"), "{why}");
    assert!(!report.passed());
}

#[test]
fn a_canary_that_cannot_tell_refuses_too() {
    let (rt, asked) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Inconclusive,
    );
    let Clearance::Refused { why, .. } = settled(&rt, RuntimeProfile::Inherit) else {
        panic!("inconclusive is not a pass")
    };
    assert!(why.contains("deny inconclusive"), "{why}");
    // The model not making the call is asked about once more.
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[test]
fn warn_starts_the_run_and_the_report_says_it_was_not_cleared() {
    let settings = RuntimeSettings {
        on_error: OnError::Warn,
        ..RuntimeSettings::default()
    };
    let (rt, _) = runtime(settings, probes("2.1.323"), CheckStatus::Fail);
    let Clearance::Cleared(Some(report)) = settled(&rt, RuntimeProfile::Controlled) else {
        panic!("on_error = warn starts it")
    };
    let note = report.checks.last().unwrap();
    assert_eq!(note.name, "on_error");
    assert!(note.detail.contains("started although"));
}

#[test]
fn a_version_below_the_floor_is_refused_without_spending_a_call() {
    let (rt, asked) = runtime(
        RuntimeSettings::default(),
        probes("2.1.100"),
        CheckStatus::Pass,
    );
    let Clearance::Refused { why, .. } = rt.clearance(RuntimeProfile::Controlled) else {
        panic!("below the floor")
    };
    assert!(why.contains("version fail"), "{why}");
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_version_that_cannot_be_read_is_refused() {
    let (rt, _) = runtime(
        RuntimeSettings::default(),
        Probes::default(),
        CheckStatus::Pass,
    );
    let Clearance::Refused { why, .. } = rt.clearance(RuntimeProfile::Controlled) else {
        panic!("unknown version")
    };
    assert!(why.contains("could not be read"), "{why}");
}

#[test]
fn a_host_policy_that_drops_the_hook_refuses_the_start() {
    let mut p = probes("2.1.323");
    p.host = HostPolicy {
        locks: vec!["allowManagedHooksOnly".into()],
        blocks: vec!["allowManagedHooksOnly is set".into()],
        ..HostPolicy::default()
    };
    let (rt, asked) = runtime(RuntimeSettings::default(), p, CheckStatus::Pass);
    let Clearance::Refused { why, .. } = rt.clearance(RuntimeProfile::Controlled) else {
        panic!("the Guard hook would be dropped")
    };
    assert!(why.contains("host_policy fail"), "{why}");
    assert!(why.contains("allowManagedHooksOnly"), "{why}");
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_version_newer_than_tested_is_a_note_and_the_canary_decides() {
    let (rt, _) = runtime(
        RuntimeSettings::default(),
        probes("2.1.400"),
        CheckStatus::Pass,
    );
    assert_eq!(rt.standing(), VersionStanding::AboveTested);
    let Clearance::Cleared(Some(report)) = settled(&rt, RuntimeProfile::Controlled) else {
        panic!("a newer version that passes the canary starts")
    };
    assert_eq!(report.checks[0].status, CheckStatus::Note);
}

#[test]
fn off_runs_no_canary_and_says_so() {
    let settings = RuntimeSettings {
        canary: CanaryMode::Off,
        ..RuntimeSettings::default()
    };
    let (rt, asked) = runtime(settings, probes("2.1.323"), CheckStatus::Fail);
    let Clearance::Cleared(Some(report)) = rt.clearance(RuntimeProfile::Controlled) else {
        panic!("off clears")
    };
    let deny = report.checks.iter().find(|c| c.name == "deny").unwrap();
    assert_eq!(deny.status, CheckStatus::Skipped);
    assert!(deny.detail.contains("canary = off"));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_cached_result_serves_until_the_profile_changes() {
    let settings = RuntimeSettings {
        canary: CanaryMode::Cached,
        ..RuntimeSettings::default()
    };
    let (rt, asked) = runtime(settings, probes("2.1.323"), CheckStatus::Pass);
    assert!(matches!(
        settled(&rt, RuntimeProfile::Controlled),
        Clearance::Cleared(Some(_))
    ));
    assert_eq!(
        rt.clearance(RuntimeProfile::Controlled),
        Clearance::Cleared(None)
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    // Another profile is another launch: it gets its own canary.
    assert_eq!(rt.clearance(RuntimeProfile::Inherit), Clearance::Pending);
    assert!(matches!(
        settled(&rt, RuntimeProfile::Inherit),
        Clearance::Cleared(Some(_))
    ));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[test]
fn the_conformance_set_runs_every_scenario_after_the_static_checks() {
    let (rt, asked) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Pass,
    );
    let report = rt.conformance(RuntimeProfile::Controlled);
    let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "version",
            "host_policy",
            "deny",
            "hook_exit_1",
            "hook_timeout",
            "hook_invalid_json",
            "subagent"
        ]
    );
    assert_eq!(asked.load(Ordering::SeqCst), Scenario::ALL.len());
    assert!(report.passed());
    assert_eq!(report.version.as_deref(), Some("2.1.323"));
}

fn intent(profile: RuntimeProfile, review: bool, lean: bool) -> LaunchIntent {
    LaunchIntent {
        job: JobId(7),
        profile,
        review,
        board: false,
        unattended: true,
        lean,
    }
}

#[test]
fn the_launch_record_says_how_the_run_was_started() {
    let (rt, _) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Pass,
    );
    let l = rt
        .launch(&intent(RuntimeProfile::Controlled, false, false))
        .unwrap();
    assert_eq!(l.runtime, "claude-code");
    assert_eq!(l.profile, RuntimeProfile::Controlled);
    assert_eq!(l.version.as_deref(), Some("2.1.323"));
    assert_eq!(l.standing, VersionStanding::Tested);
    assert_eq!(l.sources, "user");
    assert!(l.strict_mcp);
    assert_eq!(l.settings_hash.len(), 64);

    let inherit = rt
        .launch(&intent(RuntimeProfile::Inherit, false, false))
        .unwrap();
    assert_eq!(inherit.sources, "default");
    assert!(!inherit.strict_mcp);
    // The controlled settings pin hooks on, so they differ.
    assert_ne!(inherit.settings_hash, l.settings_hash);

    let lean = rt
        .launch(&intent(RuntimeProfile::Inherit, false, true))
        .unwrap();
    assert_eq!(lean.sources, "project,local");

    // A reviewer is controlled whatever its intent says, with no sources.
    let reviewer = rt
        .launch(&intent(RuntimeProfile::Inherit, true, false))
        .unwrap();
    assert_eq!(reviewer.profile, RuntimeProfile::Controlled);
    assert_eq!(reviewer.sources, "none");
    assert!(reviewer.strict_mcp);
}

#[test]
fn the_settings_hash_is_the_hash_of_what_the_runner_passes() {
    let (rt, _) = runtime(
        RuntimeSettings::default(),
        probes("2.1.323"),
        CheckStatus::Pass,
    );
    let l = rt
        .launch(&intent(RuntimeProfile::Controlled, false, false))
        .unwrap();
    let json = rt.spec().settings_json(true).unwrap();
    assert_eq!(
        l.settings_hash,
        swarmux_domain::runtime::sha256_hex(json.as_bytes())
    );
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["disableAllHooks"], false);
}

fn environment(version: &str, servers: &[(&str, &str)]) -> Environment {
    Environment {
        session_id: "s".into(),
        claude_code_version: Some(version.into()),
        permission_mode: "default".into(),
        tools: vec!["Bash".into(), "Read".into()],
        mcp_servers: servers
            .iter()
            .map(|(n, s)| McpServer {
                name: (*n).into(),
                status: (*s).into(),
            })
            .collect(),
        plugins: vec!["p".into()],
        capabilities: vec!["interrupt_receipt_v1".into()],
        ..Environment::default()
    }
}

#[test]
fn what_claude_reports_is_set_against_what_was_asked() {
    let mut s = RuntimeSettings::default();
    s.mcp.push(swarmux_application::McpServerDef {
        name: "docs".into(),
        command: "docs".into(),
        args: Vec::new(),
        env: Vec::new(),
        url: None,
        transport: "stdio".into(),
    });
    let spec = LaunchSpec::default().resolve(&s, &Probes::default());
    let rt = ClaudeRuntime::new(
        "claude".into(),
        "/bin/swarmux".into(),
        s,
        spec,
        probes("2.1.323"),
    );
    let launch = rt
        .launch(&intent(RuntimeProfile::Controlled, false, false))
        .unwrap();
    let ok = rt.observe(
        Some(&launch),
        &environment(
            "2.1.323",
            &[("swarmux-board", "connected"), ("docs", "pending")],
        ),
    );
    assert!(ok.mismatches.is_empty(), "{:?}", ok.mismatches);
    assert_eq!(ok.tools, 2);
    assert_eq!(ok.plugins, 1);
    assert_eq!(ok.capabilities, ["interrupt_receipt_v1"]);
    assert_eq!(ok.mcp_servers, ["swarmux-board=connected", "docs=pending"]);

    let off = rt.observe(
        Some(&launch),
        &environment("2.1.330", &[("claude.ai Gmail", "needs-auth")]),
    );
    assert_eq!(off.mismatches.len(), 2, "{:?}", off.mismatches);
    assert!(off.mismatches[0].contains("2.1.330"));
    assert!(off.mismatches[1].contains("claude.ai Gmail"));

    // An inheriting run is not held to the server list.
    let inherit = rt
        .launch(&intent(RuntimeProfile::Inherit, false, false))
        .unwrap();
    let free = rt.observe(
        Some(&inherit),
        &environment("2.1.323", &[("claude.ai Gmail", "needs-auth")]),
    );
    assert!(free.mismatches.is_empty());
}

#[test]
fn on_failure_follows_the_version_unless_told() {
    use swarmux_application::HookFailure;
    let resolve = |hook_failure, version: &str| {
        let s = RuntimeSettings {
            hook_failure,
            ..RuntimeSettings::default()
        };
        LaunchSpec::default()
            .resolve(&s, &probes(version))
            .on_failure_block
    };
    assert!(resolve(HookFailure::Auto, "2.1.323"));
    assert!(resolve(HookFailure::Auto, "2.1.295"));
    assert!(!resolve(HookFailure::Auto, "2.1.294"));
    assert!(!resolve(HookFailure::Off, "2.1.323"));
    assert!(resolve(HookFailure::Block, "2.1.100"));
    // A version that could not be read keeps the key: older versions ignore it.
    let unknown = LaunchSpec::default().resolve(&RuntimeSettings::default(), &Probes::default());
    assert!(unknown.on_failure_block);
}

#[test]
fn the_status_line_is_replaced_only_when_the_user_has_none_or_says_on() {
    use swarmux_application::StatuslineMode;
    let resolve = |mode, user_has| {
        let s = RuntimeSettings {
            statusline: mode,
            ..RuntimeSettings::default()
        };
        let p = Probes {
            user_statusline: user_has,
            ..Probes::default()
        };
        LaunchSpec::default().resolve(&s, &p).statusline
    };
    assert!(resolve(StatuslineMode::Auto, false));
    assert!(!resolve(StatuslineMode::Auto, true));
    assert!(resolve(StatuslineMode::On, true));
    assert!(!resolve(StatuslineMode::Off, false));
}

#[test]
fn the_board_is_merged_with_the_allowed_servers_in_a_controlled_run() {
    let s = RuntimeSettings {
        mcp: vec![swarmux_application::McpServerDef {
            name: "docs".into(),
            command: "docs-server".into(),
            args: Vec::new(),
            env: Vec::new(),
            url: None,
            transport: "stdio".into(),
        }],
        ..RuntimeSettings::default()
    };
    let spec = LaunchSpec {
        board: Some(BoardMcp {
            exe: "/bin/swarmux".into(),
            pipe: "p".into(),
        }),
        ..LaunchSpec::default()
    }
    .resolve(&s, &Probes::default());
    let req = swarmux_application::ports::RunRequest {
        job_id: JobId(3),
        cwd: "/w".into(),
        session_id: swarmux_domain::SessionId::random(),
        resume: false,
        fork: false,
        model: "sonnet".into(),
        permission_mode: "default".into(),
        prompt: "p".into(),
        lean: false,
        review: false,
        env: Vec::new(),
        max_budget_usd: None,
        board: true,
        add_dirs: Vec::new(),
        profile: RuntimeProfile::Controlled,
    };
    let args = spec.extra_args(&req);
    let at = args.iter().position(|a| a == "--mcp-config").unwrap();
    let v: serde_json::Value = serde_json::from_str(&args[at + 1]).unwrap();
    let servers = v["mcpServers"].as_object().unwrap();
    assert!(servers.contains_key("swarmux-board") && servers.contains_key("docs"));
    assert_eq!(args.iter().filter(|a| *a == "--mcp-config").count(), 1);
    // Inheriting: the Board alone, as before.
    let inherit = swarmux_application::ports::RunRequest {
        profile: RuntimeProfile::Inherit,
        ..req
    };
    let args = spec.extra_args(&inherit);
    let at = args.iter().position(|a| a == "--mcp-config").unwrap();
    let v: serde_json::Value = serde_json::from_str(&args[at + 1]).unwrap();
    assert_eq!(v["mcpServers"].as_object().unwrap().len(), 1);
}
