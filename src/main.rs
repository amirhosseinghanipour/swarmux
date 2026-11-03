//! Composition root: the only place that wires concrete adapters to use cases.

mod doctor;
mod jev;
mod telemetry;

use std::rc::Rc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::{
    GuardService, HistoryService, Jev, JobService, PromptService, RoutingService, SessionService,
    SharedPolicy,
};
use swarmux_infrastructure::hook::{self, HookServer};
use swarmux_infrastructure::{
    ClaudeCli, ConfigRuleWriter, FsAgentCatalog, GitCli, GuardHook, HaikuRouter, Paths,
    SqliteStore, SysinfoMonitor, SystemClock, load_settings,
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
    /// JEV, the optional hosted decision model (bring your own key).
    Jev {
        #[command(subcommand)]
        action: JevAction,
    },
    /// Hooks Claude Code runs inside swarmux jobs (not for direct use).
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        event: HookEvent,
    },
}

#[derive(Subcommand)]
enum JevAction {
    /// Store a provider's API key in the OS keychain (read without echo).
    SetKey {
        /// typesafe or openrouter
        provider: String,
    },
    /// Send one sample decision and print the verdict, confidence and latency.
    Test {
        /// typesafe or openrouter (default: the one in config.toml)
        provider: Option<String>,
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
        Some(Command::Jev {
            action: JevAction::SetKey { provider },
        }) => jev::set_key(&provider),
        Some(Command::Jev {
            action: JevAction::Test { provider },
        }) => jev::test(provider.as_deref()),
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
    let mut guard = GuardService::new(policy, server, store.clone(), clock.clone())
        .with_timeout_secs(guard_settings.approval_timeout_secs)
        .with_rule_writer(Rc::new(ConfigRuleWriter::new(paths.config.clone())));
    let router = HaikuRouter::new(doctor::claude_bin(), rt.handle().clone());
    let mut routing =
        RoutingService::new(store.clone(), store.clone(), store.clone(), clock.clone())
            .with_router(Rc::new(router));
    if let Some((advisor, _)) = jev::advisor(&settings.jev, rt.handle().clone())? {
        let jev = Rc::new(Jev::new(
            Rc::new(advisor),
            store.clone(),
            clock.clone(),
            settings.jev.threshold,
        )?);
        guard = guard.with_jev(jev.clone());
        routing = routing.with_jev(jev);
    }

    let workspace = Rc::new(GitCli::default());
    let sessions = SessionService::new(store.clone(), clock.clone())
        .with_default_model(&settings.default_model);
    let prompts = PromptService::new(
        store.clone(),
        store.clone(),
        workspace.clone(),
        clock.clone(),
    );
    let history = HistoryService::new(store.clone());
    // Collectors run from the start so the stations have data once shown.
    let _telemetry = telemetry::start(&settings, &paths, store.clone(), clock.clone());
    let mut jobs = JobService::new(store.clone(), store.clone(), runner, clock)
        .with_telemetry_store(store)
        .with_limits(settings.limits())
        .with_loop_repeats(guard_settings.loop_repeats)
        .with_monitor(Rc::new(SysinfoMonitor::new()))
        .with_review(workspace, &settings.review)
        .with_agent_catalog(Rc::new(FsAgentCatalog::new(Some(paths.home.clone()))));
    jobs.recover()?;
    let app = App::new(
        sessions,
        jobs,
        prompts,
        history,
        guard,
        routing,
        std::env::current_dir()?,
    );
    Ok(swarmux_tui::run(app)?)
}
