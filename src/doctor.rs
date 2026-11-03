//! `swarmux doctor`: checks Claude Code and swarmux's own data.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use swarmux_application::ports::{Clock, ConsentStore, SystemMonitor};
use swarmux_application::{GuardSettings, Settings, SharedPolicy};
use swarmux_infrastructure::hook::{self, ENV_JOB, ENV_PIPE, HookServer};
use swarmux_infrastructure::jev::keys;
use swarmux_infrastructure::{
    Endpoint, Paths, SqliteStore, SysinfoMonitor, SystemClock, load_settings,
};

const GB: f64 = (1u64 << 30) as f64;

/// The `claude` executable: `SWARMUX_CLAUDE_BIN` or `claude` on PATH.
pub fn claude_bin() -> PathBuf {
    std::env::var_os("SWARMUX_CLAUDE_BIN")
        .map(Into::into)
        .unwrap_or_else(|| "claude".into())
}

pub fn run() -> anyhow::Result<()> {
    let bin = claude_bin();
    match claude_version(&bin) {
        Some(v) => println!("ok   claude {v}"),
        None => println!(
            "fail claude not found ({}). Install Claude Code or set SWARMUX_CLAUDE_BIN.",
            bin.display()
        ),
    }

    let paths = Paths::default_home()?;
    println!("ok   data dir {}", paths.data_dir.display());
    std::fs::create_dir_all(&paths.data_dir)?;
    match SqliteStore::open(&paths.db).and_then(|s| s.schema_version()) {
        Ok(v) => println!("ok   database {} (schema v{v})", paths.db.display()),
        Err(e) => println!("fail database: {e}"),
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
            jev_summary(&s, &paths);
            crate::telemetry::doctor(&s, &paths, SystemClock.now());
        }
        Err(e) => println!("fail config {e}"),
    }
    hook_checks();
    match SysinfoMonitor::new().free_memory_bytes() {
        Some(free) => println!("ok   free memory {:.1} GB", free as f64 / GB),
        None => println!("warn free memory unknown: the RAM guard is off"),
    }
    Ok(())
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
    for rule in &g.allow {
        println!("       allow {rule}");
    }
    for rule in policy.deny_rules() {
        println!("       deny  {rule}");
    }
}

/// The JEV provider and whether a key is found (never the key itself).
fn jev_summary(s: &Settings, paths: &Paths) {
    let j = &s.jev;
    let Some(endpoint) = Endpoint::for_provider(j.provider) else {
        println!("ok   jev off (bring your own key: swarmux jev set-key typesafe)");
        return;
    };
    let p = j.provider.as_str();
    println!(
        "ok   jev provider {p} at {} (model {}), threshold {:.2}, timeout {} ms",
        endpoint.host(),
        endpoint.model,
        j.threshold,
        j.timeout_ms()
    );
    match keys::find_key(j.provider) {
        Some((_, source)) => println!("ok   jev key found in {source}"),
        None => println!(
            "warn jev has no key: run `swarmux jev set-key {p}` or set {}",
            keys::env_var(j.provider).unwrap_or_default()
        ),
    }
    let consent = SqliteStore::open(&paths.db).and_then(|db| db.has_consent(p));
    match consent {
        Ok(true) => println!("ok   jev consent given: unmatched tool calls and routing ask {p}"),
        Ok(false) => println!("ok   jev consent not given yet: swarmux asks before first use"),
        Err(e) => println!("fail jev consent unknown: {e}"),
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
            drop(server);
        }
    }
    match run_hook(&exe, None) {
        Some((out, took)) if out.is_empty() => println!(
            "ok   hook without swarmux running stays silent and exits 0 ({:.1} ms)",
            took.as_secs_f64() * 1e3
        ),
        Some((out, _)) => println!("fail hook without swarmux printed: {out}"),
        None => println!("fail hook without swarmux didn't exit 0"),
    }
}

/// Runs `exe hook pre-tool` on [`PROBE`]; its stdout and run time if it exited 0.
fn run_hook(exe: &Path, pipe: Option<&str>) -> Option<(String, Duration)> {
    let mut cmd = Command::new(exe);
    cmd.args(["hook", "pre-tool"])
        .env_remove(ENV_PIPE)
        .env_remove(ENV_JOB)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(pipe) = pipe {
        cmd.env(ENV_PIPE, pipe);
    }
    let start = Instant::now();
    let mut child = cmd.spawn().ok()?;
    child.stdin.take()?.write_all(PROBE.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    let took = start.elapsed();
    output.status.success().then(|| {
        (
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
            took,
        )
    })
}

fn claude_version(bin: &Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
