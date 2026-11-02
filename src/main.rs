//! Composition root: the only place that wires concrete adapters to use cases.

mod doctor;

use std::rc::Rc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::{
    GuardService, HistoryService, JobService, PromptService, SessionService, SharedPolicy,
};
use swarmux_infrastructure::hook::{self, HookServer};
use swarmux_infrastructure::{
    ClaudeCli, ConfigRuleWriter, GitCli, GuardHook, Paths, SqliteStore, SysinfoMonitor,
    SystemClock, load_settings,
};
use swarmux_tui::App;

/// Claude Code may run the hook this much longer than an approval can wait.
const HOOK_GRACE_SECS: u64 = 10;

/// swarmux — a prompt-first command center for Claude Code.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Check that Claude Code is installed and swarmux's data is healthy.
    Doctor,
    /// Hooks Claude Code runs inside swarmux jobs (not for direct use).
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        event: HookEvent,
    },
}

#[derive(Subcommand)]
enum HookEvent {
    /// PreToolUse: asks the running swarmux whether a tool call may run.
    PreTool,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => run_tui(),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Hook {
            event: HookEvent::PreTool,
        }) => {
            hook::pre_tool_main();
            Ok(())
        }
    }
}

fn run_tui() -> anyhow::Result<()> {
    // Everything that can fail with a readable message happens before the
    // terminal switches into full-screen mode.
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let settings = load_settings(&paths.config)?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let guard_settings = &settings.guard;
    let policy = SharedPolicy::new(guard_settings.policy());
    let pipe = hook::endpoint_for(std::process::id());
    let server = Rc::new(
        HookServer::start(rt.handle(), &pipe, policy.clone())
            .with_context(|| format!("cannot listen on {pipe}"))?,
    );
    let guard_hook = GuardHook {
        exe: std::env::current_exe().context("cannot find the swarmux executable")?,
        pipe,
        timeout_secs: guard_settings.approval_timeout_secs + HOOK_GRACE_SECS,
    };
    let runner =
        Rc::new(ClaudeCli::new(doctor::claude_bin(), rt.handle().clone()).with_guard(guard_hook));
    let guard = GuardService::new(policy, server, store.clone(), clock.clone())
        .with_timeout_secs(guard_settings.approval_timeout_secs)
        .with_rule_writer(Rc::new(ConfigRuleWriter::new(paths.config.clone())));

    let workspace = Rc::new(GitCli::default());
    let sessions = SessionService::new(store.clone(), clock.clone())
        .with_default_model(&settings.default_model);
    let prompts = PromptService::new(store.clone(), store.clone(), workspace, clock.clone());
    let history = HistoryService::new(store.clone());
    let mut jobs = JobService::new(store.clone(), store, runner, clock)
        .with_limits(settings.limits())
        .with_loop_repeats(guard_settings.loop_repeats)
        .with_monitor(Rc::new(SysinfoMonitor::new()));
    jobs.recover()?;
    let app = App::new(
        sessions,
        jobs,
        prompts,
        history,
        guard,
        std::env::current_dir()?,
    );
    Ok(swarmux_tui::run(app)?)
}
