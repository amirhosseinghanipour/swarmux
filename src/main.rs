//! Composition root: the only place that wires concrete adapters to use cases.

mod audit;
mod doctor;
mod gate;
mod jev;
mod telemetry;

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::ledger::{LedgerService, LedgeredDecisions, LedgeredJobs};
use swarmux_application::ports::Clock;
use swarmux_application::{
    GateKeeper, GateService, GuardService, HistoryService, HookBus, Jev, JobService, PromptService,
    RoutingService, SessionService, SharedPolicy,
};
use swarmux_infrastructure::hook::{self, HookServer, Responders};
use swarmux_infrastructure::{
    ClaudeCli, ConfigRuleWriter, FsAgentCatalog, FsGateSource, GitCli, GuardHook, HaikuRouter,
    Paths, ProcessChecker, SqliteLedger, SqliteStore, SysinfoMonitor, SystemClock, load_settings,
};
use swarmux_tui::{App, Launch};

/// Claude Code may run the hook this much longer than an approval can wait.
const HOOK_GRACE_SECS: u64 = 10;

/// swarmux — a prompt-first command center for Claude Code.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Start without the splash screen.
    #[arg(long, global = true)]
    no_splash: bool,
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
    /// The tamper-evident ledger of everything swarmux observed or decided.
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
    /// The done-gate: the checks an agent must pass before it may stop.
    Gate {
        #[command(subcommand)]
        action: GateAction,
    },
    /// Hooks Claude Code runs inside swarmux jobs (not for direct use).
    #[command(hide = true)]
    Hook {
        /// The hook event: pre-tool, stop, session-start, … (or Claude Code's own name).
        event: String,
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
enum GateAction {
    /// Run the repository's checks now; exits with 1 if one fails.
    Run {
        /// A folder inside the repository (default: the current one).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Show the gate that applies: checks, protected paths, limits.
    Show {
        /// A folder inside the repository (default: the current one).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AuditAction {
    /// Check the whole hash chain; exits with 1 if it is broken.
    Verify,
    /// Show the newest entries.
    Tail {
        /// How many entries.
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
}

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Audit {
            action: AuditAction::Verify,
        }) => {
            return Ok(if audit::verify()? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Audit {
            action: AuditAction::Tail { n },
        }) => audit::tail_cmd(n),
        Some(Command::Gate {
            action: GateAction::Run { dir },
        }) => {
            return Ok(if gate::run(dir)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Gate {
            action: GateAction::Show { dir },
        }) => gate::show(dir),
        None => run_tui(cli.no_splash),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Jev {
            action: JevAction::SetKey { provider },
        }) => jev::set_key(&provider),
        Some(Command::Jev {
            action: JevAction::Test { provider },
        }) => jev::test(provider.as_deref()),
        Some(Command::Hook { event }) => {
            hook::hook_main(&event);
            Ok(())
        }
    }?;
    Ok(ExitCode::SUCCESS)
}

fn run_tui(no_splash: bool) -> anyhow::Result<()> {
    // Everything that can fail with a readable message happens before the
    // terminal switches into full-screen mode.
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let settings = load_settings(&paths.config)?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    // Every job change and decision that is stored is also chained into the ledger.
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    let jobs_log = Rc::new(LedgeredJobs::new(store.clone(), ledger.clone()));
    let decisions = Rc::new(LedgeredDecisions::new(store.clone(), ledger.clone()));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // The done-gate judges every stop and fences protected paths.
    let (keeper, gate_records) = GateKeeper::new(
        Arc::new(FsGateSource::new()),
        Arc::new(ProcessChecker::new()),
    );
    let keeper = Arc::new(keeper);
    let guard_settings = &settings.guard;
    let policy = SharedPolicy::new(guard_settings.policy()).with_fence(keeper.clone());
    let pipe = hook::endpoint_for(std::process::id());
    let responders = Responders {
        stop: keeper.clone(),
        ..Responders::default()
    };
    let server = Rc::new(
        HookServer::start_with(rt.handle(), &pipe, policy.clone(), responders)
            .with_context(|| format!("cannot listen on {pipe}"))?,
    );
    let guard_hook = GuardHook {
        exe: std::env::current_exe().context("cannot find the swarmux executable")?,
        pipe,
        timeout_secs: guard_settings.approval_timeout_secs + HOOK_GRACE_SECS,
    };
    let runner =
        Rc::new(ClaudeCli::new(doctor::claude_bin(), rt.handle().clone()).with_guard(guard_hook));
    let gate = GateService::new(gate_records, ledger.clone(), store.clone(), clock.clone());
    let hook_bus = HookBus::new(server.clone(), ledger, store.clone(), clock.clone());
    let mut guard = GuardService::new(policy, server, decisions.clone(), clock.clone())
        .with_timeout_secs(guard_settings.approval_timeout_secs)
        .with_rule_writer(Rc::new(ConfigRuleWriter::new(paths.config.clone())));
    let router = HaikuRouter::new(doctor::claude_bin(), rt.handle().clone());
    let mut routing =
        RoutingService::new(store.clone(), jobs_log.clone(), decisions, clock.clone())
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
    let history = HistoryService::new(jobs_log.clone());
    // Collectors run from the start so the stations have data once shown.
    let telemetry = telemetry::start(&settings, &paths, store.clone(), clock.clone());
    let mut jobs = JobService::new(store.clone(), jobs_log, runner, clock)
        .with_telemetry_store(store)
        .with_limits(settings.limits())
        .with_loop_repeats(guard_settings.loop_repeats)
        .with_monitor(Rc::new(SysinfoMonitor::new()))
        .with_review(workspace, &settings.review)
        .with_agent_catalog(Rc::new(FsAgentCatalog::new(Some(paths.home.clone()))))
        .with_execution_log(Rc::new(keeper));
    jobs.recover()?;
    let app = App::new(
        sessions,
        jobs,
        prompts,
        history,
        guard,
        routing,
        std::env::current_dir()?,
    )
    .with_clock(Rc::new(SystemClock))
    .with_hook_bus(hook_bus)
    .with_gate(gate)
    .with_telemetry_feed(telemetry, &settings.telemetry)
    .with_look(
        &settings.ui,
        Launch {
            claude: doctor::claude_bin().display().to_string(),
            no_splash,
            now_ms: SystemClock.now(),
        },
    );
    Ok(swarmux_tui::run(app)?)
}
