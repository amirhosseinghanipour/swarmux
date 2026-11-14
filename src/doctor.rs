//! `swarmux doctor`: checks Claude Code and swarmux's own data.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use swarmux_application::ports::HookRequest;
use swarmux_application::ports::{Clock, SystemMonitor};
use swarmux_application::{GuardSettings, Settings, SharedPolicy, UnattendedJobs};
use swarmux_domain::runtime::{
    CanaryCheck, CanaryReport, CheckStatus, RuntimeProfile, VersionStanding,
};
use swarmux_domain::{HookEvent, JobId, Verdict};
use swarmux_infrastructure::claude_cli::runtime::range;
use swarmux_infrastructure::claude_cli::{LaunchSpec, Probes};
use swarmux_infrastructure::hook::{self, ENV_JOB, ENV_PIPE, HookServer};
use swarmux_infrastructure::process::hide_std;
use swarmux_infrastructure::{
    ClaudeRuntime, Paths, SqliteLedger, SqliteStore, SysinfoMonitor, SystemClock, legacy_jev,
    load_settings,
};

const GB: f64 = (1u64 << 30) as f64;

/// The `claude` executable: `SWARMUX_CLAUDE_BIN` or `claude` on PATH.
pub fn claude_bin() -> PathBuf {
    std::env::var_os("SWARMUX_CLAUDE_BIN")
        .map(Into::into)
        .unwrap_or_else(|| "claude".into())
}

/// `swarmux doctor`; with `conformance` it also runs the canary set against
/// the installed Claude Code (a few tiny calls to the model) and fails when a
/// required canary does not pass.
pub fn run(conformance: bool, profile: RuntimeProfile) -> anyhow::Result<()> {
    let bin = claude_bin();
    match claude_version(&bin) {
        Some(v) => println!("ok   claude {v}"),
        None => println!(
            "fail claude not found ({}). Install Claude Code or set SWARMUX_CLAUDE_BIN.",
            bin.display()
        ),
    }
    let probes = Probes::run(&bin);

    let paths = Paths::default_home()?;
    println!("ok   data dir {}", paths.data_dir.display());
    std::fs::create_dir_all(&paths.data_dir)?;
    match SqliteStore::open(&paths.db).and_then(|s| s.schema_version()) {
        Ok(v) => println!("ok   database {} (schema v{v})", paths.db.display()),
        Err(e) => println!("fail database: {e}"),
    }
    match SqliteLedger::open(&paths.db) {
        Ok(ledger) => crate::audit::doctor(&ledger),
        Err(e) => println!("fail ledger: {e}"),
    }
    match load_settings(&paths.config) {
        Ok(s) => {
            let source = if paths.config.exists() {
                paths.config.display().to_string()
            } else {
                format!("defaults, no {}", paths.config.display())
            };
            println!("ok   settings ({source})");
            println!("       max_concurrent  = {}", s.max_concurrent);
            println!("       min_free_ram_mb = {}", s.min_free_ram_mb);
            println!("       default_model   = \"{}\"", s.default_model);
            guard_summary(&s);
            runtime_summary(&s, &probes);
            for line in legacy_jev::doctor_lines() {
                println!("{line}");
            }
            crate::telemetry::doctor(&s, &paths, SystemClock.now());
        }
        Err(e) => println!("fail config {e}"),
    }
    hook_checks();
    println!("{}", crate::daemon::doctor_line(&paths));
    match SysinfoMonitor::new().free_memory_bytes() {
        Some(free) => println!("ok   free memory {:.1} GB", free as f64 / GB),
        None => println!("warn free memory unknown: the RAM guard is off"),
    }
    if conformance {
        return run_conformance(&bin, probes, profile);
    }
    Ok(())
}

/// What the settings, the version and the host's policy mean for a run.
fn runtime_summary(s: &Settings, probes: &Probes) {
    let r = &s.runtime;
    let range = range::range();
    let version = probes
        .version
        .map_or_else(|| "unknown".to_string(), |v| v.to_string());
    match range.standing(probes.version) {
        VersionStanding::Tested => println!(
            "ok   claude {version} is in the tested range {}..{} (floor {})",
            range.tested_min, range.tested_max, range.floor
        ),
        VersionStanding::AboveTested | VersionStanding::BelowTested => println!(
            "warn claude {version} is outside the tested range {}..{}: only `swarmux doctor --conformance` and the canary before each mission check it",
            range.tested_min, range.tested_max
        ),
        VersionStanding::BelowFloor => println!(
            "fail claude {version} is older than {}, the first version whose hooks can fail closed: unattended missions are refused",
            range.floor
        ),
        VersionStanding::Unknown => println!(
            "fail the version of claude could not be read: unattended missions are refused"
        ),
    }
    println!(
        "ok   runtime: profile = {} (unattended jobs in a repository you have not trusted run controlled, reviewers always), canary = {}, on_error = {}",
        r.profile.as_str(),
        r.canary.as_str(),
        r.on_error.as_str()
    );
    println!(
        "       controlled runs load the settings sources [{}], reviewers [{}]; MCP servers they may use besides the Board: {}; hook onFailure = {}; status line = {}",
        r.controlled_sources.join(", "),
        r.reviewer_sources.join(", "),
        if r.mcp.is_empty() {
            "none".to_string()
        } else {
            r.mcp
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        },
        r.hook_failure.as_str(),
        r.statusline.as_str(),
    );
    if !r.trusted_repos.is_empty() {
        println!(
            "       trusted folders (their own Claude Code settings apply to unattended jobs): {}",
            r.trusted_repos
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if r.canary == swarmux_application::CanaryMode::Off {
        println!("warn runtime.canary = off: the deny canary does not run before missions");
    }
    if r.on_error == swarmux_application::OnError::Warn {
        println!(
            "warn runtime.on_error = warn: a mission starts even when a canary fails or cannot be made"
        );
    }
    let host = &probes.host;
    if host.blocks.is_empty() {
        let found = if host.sources.is_empty() {
            "no managed settings found".to_string()
        } else {
            format!("managed settings found: {}", host.sources.join(", "))
        };
        println!("ok   host policy: {found}");
    } else {
        for block in &host.blocks {
            println!("fail host policy: {block}");
        }
    }
    if !host.locks.is_empty() {
        println!("       managed locks: {}", host.locks.join(", "));
    }
    for note in &host.unreadable {
        println!("       not read: {note}");
    }
}

fn print_check(c: &CanaryCheck) {
    let tag = match (c.status, c.required) {
        (CheckStatus::Pass, _) => "ok  ",
        (CheckStatus::Fail, _) | (CheckStatus::Inconclusive, true) => "fail",
        (CheckStatus::Inconclusive, false) => "warn",
        (CheckStatus::Note, _) => "note",
        (CheckStatus::Skipped, _) => "skip",
    };
    println!("{tag} {} ({}): {}", c.name, c.status.as_str(), c.detail);
}

/// The canaries of the Guard itself: what it decides for a tool it does not
/// know, and whether a subagent's call is judged like the lead's. They need
/// no model.
fn guard_canaries(s: &Settings) -> Vec<CanaryCheck> {
    let unattended = UnattendedJobs::new();
    unattended.mark(JobId(1));
    let policy = SharedPolicy::new(s.guard.policy()).with_unattended(unattended);
    let call = |agent: Option<&str>| HookRequest {
        request_id: 1,
        job_id: Some(JobId(1)),
        tool_name: "mcp__swarmux_canary__unknown".to_string(),
        input: "{}".to_string(),
        cwd: ".".to_string(),
        agent_id: agent.map(str::to_string),
        agent_type: agent.map(|_| "general-purpose".to_string()),
        ..HookRequest::default()
    };
    let lead = policy.decide(&call(None)).0;
    let sub = policy.decide(&call(Some("agent-1"))).0;
    let (status, detail) = match lead.verdict {
        Verdict::Deny | Verdict::Ask => (
            CheckStatus::Pass,
            format!(
                "an unattended job's call to a tool no rule knows is {}",
                lead.verdict.as_str()
            ),
        ),
        Verdict::Allow | Verdict::Pass => (
            CheckStatus::Fail,
            format!(
                "an unattended job's call to a tool no rule knows is {} (guard.unmatched = \"{}\")",
                lead.verdict.as_str(),
                format!("{:?}", s.guard.unmatched).to_lowercase()
            ),
        ),
    };
    let same = lead.verdict == sub.verdict;
    vec![
        CanaryCheck {
            name: "unknown_tool".into(),
            status,
            detail,
            required: true,
        },
        CanaryCheck {
            name: "guard_subagent".into(),
            status: if same {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            detail: if same {
                "a subagent's call is judged like the lead's".into()
            } else {
                format!(
                    "the lead's call is {} and a subagent's is {}",
                    lead.verdict.as_str(),
                    sub.verdict.as_str()
                )
            },
            required: true,
        },
    ]
}

/// `swarmux doctor --conformance`: the real `claude`'s canaries and the
/// Guard's own, each with its result.
fn run_conformance(bin: &Path, probes: Probes, profile: RuntimeProfile) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let settings = load_settings(&paths.config)?;
    println!();
    println!(
        "conformance, profile {profile}: {} calls to claude (--model haiku, a few turns each), then the Guard's own canaries",
        swarmux_infrastructure::claude_cli::runtime::canary::Scenario::ALL.len()
    );
    let exe = std::env::current_exe()?;
    let spec = LaunchSpec::default().resolve(&settings.runtime, &probes);
    let runtime = ClaudeRuntime::new(
        bin.to_path_buf(),
        exe,
        settings.runtime.clone(),
        spec,
        probes,
    );
    let mut report: CanaryReport = runtime.conformance(profile);
    report.checks.extend(guard_canaries(&settings));
    for c in &report.checks {
        print_check(c);
    }
    let blocking = report.blocking();
    if blocking.is_empty() {
        println!("ok   conformance: every required canary passed");
        Ok(())
    } else {
        anyhow::bail!(
            "conformance failed: {}",
            blocking
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn mode_words(m: swarmux_application::FailMode) -> &'static str {
    match m {
        swarmux_application::FailMode::Closed => "denied",
        swarmux_application::FailMode::Ask => "left to Claude Code's own question",
        swarmux_application::FailMode::Open => "left to Claude Code's own rules",
    }
}

fn guard_summary(s: &Settings) {
    let g = &s.guard;
    let policy = g.policy();
    println!(
        "ok   guard: {} allow rules (+{} built-in), {} deny rules (+{} built-in), unmatched = {}",
        g.allow.len(),
        policy.allow_rules().len() - g.allow.len(),
        g.deny.len(),
        policy.deny_rules().len() - g.deny.len(),
        format!("{:?}", g.unmatched).to_lowercase(),
    );
    println!(
        "       approval_timeout_secs = {}, loop_repeats = {}",
        g.approval_timeout_secs, g.loop_repeats
    );
    // What happens when swarmux can't be reached: said as it is, not as a
    // reassurance. Jobs nobody watches (missions, reviewers) are denied by
    // default; the rest are left to Claude Code's own rules.
    let (unattended, watched) = (g.on_unreachable.mode(true), g.on_unreachable.mode(false));
    let line = format!(
        "       on_unreachable = \"{}\": when swarmux can't be reached, unattended jobs are {}, other jobs are {}",
        g.on_unreachable.as_str(),
        mode_words(unattended),
        mode_words(watched),
    );
    if unattended == swarmux_application::FailMode::Open {
        println!("warn guard fails OPEN for unattended jobs\n{line}");
    } else {
        println!("ok   guard fail mode\n{line}");
    }
    println!(
        "       always allow: scope = {}, expires after {}; protected paths: {} built-in + {} of yours",
        g.always_scope.as_str(),
        g.always_expiry_days
            .map_or("never".to_string(), |d| format!("{d} days")),
        g.protected_globs().len() - g.protected_paths.len(),
        g.protected_paths.len(),
    );
    if g.hook_on_failure_block {
        println!(
            "       hook_on_failure_block = true: the old switch for runtime.hook_failure = \"block\": PreToolUse always carries onFailure=block"
        );
    }
    println!(
        "       tool calls no rule matches: {}; irreversible calls (R4) are allowed only by a typed confirmation",
        match g.unmatched {
            swarmux_domain::Unmatched::Ask =>
                "you are asked, no model in between (guard.unmatched = \"ask\")",
            swarmux_domain::Unmatched::Pass => "left to Claude Code (guard.unmatched = \"pass\")",
        }
    );
    for w in &s.warnings {
        println!("warn {w}");
    }
    for rule in &g.allow {
        println!("       allow {rule}");
    }
    for rule in policy.deny_rules() {
        println!("       deny  {rule}");
    }
}

/// A hook input for a call every policy allows (built-in `Glob`).
const PROBE: &str =
    r#"{"hook_event_name":"PreToolUse","tool_name":"Glob","tool_input":{"pattern":"*"},"cwd":"."}"#;

/// Starts a hook socket like the app does and asks it through the real
/// `swarmux hook pre-tool`, then checks the hook stays silent without one.
fn hook_checks() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return println!("fail cannot find the swarmux executable: {e}"),
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return println!("fail cannot start the async runtime: {e}"),
    };
    let name = hook::endpoint_for(std::process::id());
    let policy = SharedPolicy::new(GuardSettings::default().policy());
    match HookServer::start(rt.handle(), &name, policy) {
        Err(e) => println!("fail hook socket {name}: {e}"),
        Ok(server) => {
            match run_hook(&exe, Some(&name)) {
                Some((out, took)) if out.contains("\"allow\"") => println!(
                    "ok   hook socket {name} answers `swarmux hook pre-tool` in {:.1} ms",
                    took.as_secs_f64() * 1e3
                ),
                Some((out, _)) => println!("fail hook socket answered unexpectedly: {out}"),
                None => println!("fail could not run `swarmux hook pre-tool`"),
            }
            latency_checks(&exe, &name);
            drop(server);
        }
    }
    match run_hook(&exe, None) {
        Some((out, took)) if out.is_empty() => println!(
            "ok   a hook outside any swarmux job guards nothing and prints nothing ({:.1} ms)",
            took.as_secs_f64() * 1e3
        ),
        Some((out, _)) => println!("fail hook outside a job printed: {out}"),
        None => println!("fail hook outside a job didn't exit 0"),
    }
    unreachable_checks(&exe);
}

/// Runs the hook as a job's process would while the app is not running, in
/// each fail mode, with a spool of its own (the real one is left alone).
fn unreachable_checks(exe: &Path) {
    let dir = std::env::temp_dir().join(format!("swarmux-doctor-{}", std::process::id()));
    let spool = dir.join("guard-fallback.jsonl");
    let gone = hook::endpoint_for(u32::MAX - 3);
    for (mode, expect) in [("closed", "deny"), ("ask", "ask"), ("open", "")] {
        let env = [
            (ENV_PIPE, gone.as_str()),
            (ENV_JOB, "1"),
            ("SWARMUX_GUARD_FAIL", mode),
            ("SWARMUX_GUARD_SPOOL", spool.to_str().unwrap_or_default()),
        ];
        let got = run_hook_with(exe, "pre-tool", PROBE, &env);
        let printed = got.as_ref().map(|(o, _)| {
            serde_json::from_str::<serde_json::Value>(o)
                .ok()
                .and_then(|v| {
                    v["hookSpecificOutput"]["permissionDecision"]
                        .as_str()
                        .map(str::to_string)
                })
                .unwrap_or_default()
        });
        match printed {
            Some(p) if p == expect => println!(
                "ok   app unreachable, SWARMUX_GUARD_FAIL={mode}: the hook {}",
                if expect.is_empty() {
                    "prints nothing (Claude Code's rules decide)".to_string()
                } else {
                    format!("prints a PreToolUse {expect}")
                }
            ),
            Some(p) => println!("fail app unreachable, mode {mode}: the hook printed {p:?}"),
            None => println!("fail app unreachable, mode {mode}: the hook didn't exit 0"),
        }
    }
    let lines = std::fs::read_to_string(&spool).map_or(0, |t| t.lines().count());
    if lines == 3 {
        println!(
            "ok   each fallback leaves a line in the spool, which the app puts in the ledger at its next start"
        );
    } else {
        println!("fail the fallbacks left {lines} spool lines, expected 3");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A hook input for `event` that every policy lets through.
fn probe_for(event: HookEvent) -> String {
    let extra = match event {
        HookEvent::Stop | HookEvent::SubagentStop => r#","last_assistant_message":"ok""#,
        HookEvent::SessionStart => r#","source":"startup""#,
        _ => r#","tool_name":"Glob","tool_input":{"pattern":"*"}"#,
    };
    format!(
        r#"{{"hook_event_name":"{}","session_id":"doctor","cwd":"."{extra}}}"#,
        event.as_str()
    )
}

/// How long the real `swarmux hook <event>` takes, spawn included, for
/// every event Claude Code waits for.
fn latency_checks(exe: &Path, pipe: &str) {
    for event in HookEvent::ALL.into_iter().filter(|e| e.is_blocking()) {
        let arg = if event == HookEvent::PreToolUse {
            "pre-tool".to_string()
        } else {
            event.kebab()
        };
        let input = probe_for(event);
        let mut times: Vec<Duration> = (0..LATENCY_CALLS)
            .filter_map(|_| run_hook_event(exe, Some(pipe), &arg, &input).map(|(_, t)| t))
            .collect();
        times.sort();
        match percentile95(&times) {
            Some(p95) if times.len() == LATENCY_CALLS => println!(
                "ok   hook {} p95 {:.1} ms over {LATENCY_CALLS} calls",
                event.as_str(),
                p95.as_secs_f64() * 1e3
            ),
            _ => println!("fail hook {} could not be measured", event.as_str()),
        }
    }
}

/// Calls timed per event by [`latency_checks`].
const LATENCY_CALLS: usize = 5;

/// The 95th percentile of the sorted `times` (nearest rank).
fn percentile95(times: &[Duration]) -> Option<Duration> {
    let n = times.len();
    (n > 0).then(|| times[(n * 95).div_ceil(100) - 1])
}

/// Runs `exe hook pre-tool` on [`PROBE`]; its stdout and run time if it exited 0.
fn run_hook(exe: &Path, pipe: Option<&str>) -> Option<(String, Duration)> {
    run_hook_event(exe, pipe, "pre-tool", PROBE)
}

/// Runs `exe hook <arg>` on `input`; its stdout and run time if it exited 0.
fn run_hook_event(
    exe: &Path,
    pipe: Option<&str>,
    arg: &str,
    input: &str,
) -> Option<(String, Duration)> {
    match pipe {
        Some(pipe) => run_hook_with(exe, arg, input, &[(ENV_PIPE, pipe)]),
        None => run_hook_with(exe, arg, input, &[]),
    }
}

/// Runs `exe hook <arg>` on `input` with `env` set (and no socket or job
/// id unless `env` gives them); its stdout and run time if it exited 0.
fn run_hook_with(
    exe: &Path,
    arg: &str,
    input: &str,
    env: &[(&str, &str)],
) -> Option<(String, Duration)> {
    let mut cmd = Command::new(exe);
    cmd.args(["hook", arg])
        .env_remove(ENV_PIPE)
        .env_remove(ENV_JOB)
        .env_remove("SWARMUX_GUARD_FAIL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    hide_std(&mut cmd);
    let start = Instant::now();
    let mut child = cmd.spawn().ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    let took = start.elapsed();
    output.status.success().then(|| {
        (
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            took,
        )
    })
}

pub fn claude_version(bin: &Path) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    hide_std(&mut cmd);
    let output = cmd.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_is_the_nearest_rank() {
        let ms = |n: u64| Duration::from_millis(n);
        assert_eq!(percentile95(&[]), None);
        assert_eq!(percentile95(&[ms(3)]), Some(ms(3)));
        let five = [ms(1), ms(2), ms(3), ms(4), ms(50)];
        assert_eq!(percentile95(&five), Some(ms(50)));
        let twenty: Vec<_> = (1..=20).map(ms).collect();
        assert_eq!(percentile95(&twenty), Some(ms(19)));
    }

    #[test]
    fn probes_name_their_event() {
        for event in HookEvent::ALL {
            let v: serde_json::Value = serde_json::from_str(&probe_for(event)).unwrap();
            assert_eq!(v["hook_event_name"], event.as_str());
        }
    }
}
