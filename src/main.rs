//! Composition root: the only place that wires concrete adapters to use cases.

mod audit;
mod doctor;
mod evidence;
mod gate;
mod jev;
mod stats;
mod telemetry;
mod worktree;

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::ledger::{LedgerService, LedgeredDecisions, LedgeredJobs};
use swarmux_application::ports::Clock;
use swarmux_application::{
    CombinedStatusline, GateKeeper, GateService, Governor, GuardService, HandoffBoard,
    HistoryService, HookBus, Jev, JobService, MissionService, PromptService, RoutingService,
    SessionService, SharedPolicy, StatuslineFeed, TruthBoard,
};
use swarmux_infrastructure::hook::{self, HookServer, Responders};
use swarmux_infrastructure::{
    ClaudeCli, ConfigRuleWriter, FsAgentCatalog, FsGateSource, GitCli, GuardHook, HaikuLinter,
    HaikuRouter, MissionFiles, Paths, ProcessChecker, SqliteLedger, SqliteStore, SysinfoMonitor,
    SystemClock, WindowsTerminal, load_settings,
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
    /// Run the checks of a delivered mission's evidence bundle again, at its
    /// head commit in a clean checkout, and say whether they still hold.
    Verify {
        /// The mission's slug, or the path of a bundle file.
        target: String,
    },
    /// What happened lately: missions delivered, escalated, merged, and what
    /// it cost.
    Brief {
        /// How many hours back to look.
        #[arg(long, default_value_t = evidence::BRIEF_HOURS)]
        hours: i64,
    },
    /// How swarmux has been doing, from what it logged.
    Stats {
        #[command(subcommand)]
        what: StatsAction,
    },
    /// Ask GitHub what became of each delivered branch (merged, reverted
    /// within seven days) and record it.
    Labels,
    /// Per-job git worktrees, their port blocks and checkpoints.
    Worktree {
        #[command(subcommand)]
        action: WorktreeAction,
    },
    /// The status line command: forwards Claude Code's status line data to
    /// swarmux and prints a compact line. Add it to a session you start by
    /// hand with --print-settings.
    Statusline {
        /// Print the `statusLine` setting that runs this command, and exit.
        #[arg(long)]
        print_settings: bool,
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
enum StatsAction {
    /// The inbox's alarm figures: rate, priority mix, flood peak and how
    /// long items waited to be looked at.
    Alarms {
        /// How many days back to look.
        #[arg(long, default_value_t = 1)]
        days: i64,
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
enum WorktreeAction {
    /// Show the worktrees that have not been reclaimed.
    List,
    /// Reclaim the worktrees of jobs that ended a while ago. Files left in
    /// them are committed to the job's branch first.
    Gc {
        /// Only say what would be reclaimed.
        #[arg(long)]
        dry_run: bool,
    },
    /// Show checkpoints: those of one job, or the newest ones.
    Checkpoints {
        /// The job number.
        job: Option<i64>,
        /// How many of the newest to show when no job is given.
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
    /// Set worktree mode for a session (by name or id).
    Mode {
        session: String,
        #[arg(value_enum)]
        mode: worktree::Mode,
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
        Some(Command::Verify { target }) => {
            return Ok(if evidence::verify(&target)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Brief { hours }) => evidence::print_brief(hours),
        Some(Command::Labels) => evidence::labels(),
        Some(Command::Stats {
            what: StatsAction::Alarms { days },
        }) => stats::alarms(days.max(1)),
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
        Some(Command::Worktree { action }) => match action {
            WorktreeAction::List => worktree::list(),
            WorktreeAction::Gc { dry_run } => worktree::gc(dry_run),
            WorktreeAction::Checkpoints { job, n } => worktree::checkpoints(job, n),
            WorktreeAction::Mode { session, mode } => worktree::mode(&session, mode),
        },
        None => run_tui(cli.no_splash),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Jev {
            action: JevAction::SetKey { provider },
        }) => jev::set_key(&provider),
        Some(Command::Jev {
            action: JevAction::Test { provider },
        }) => jev::test(provider.as_deref()),
        Some(Command::Statusline { print_settings }) => statusline(print_settings),
        Some(Command::Hook { event }) => {
            hook::hook_main(&event);
            Ok(())
        }
    }?;
    Ok(ExitCode::SUCCESS)
}

fn statusline(print_settings: bool) -> anyhow::Result<()> {
    if print_settings {
        let exe = std::env::current_exe().context("cannot find the swarmux executable")?;
        println!("{}", hook::statusline_settings(&exe));
        return Ok(());
    }
    // Without a home folder there is nowhere to leave the data; the line
    // is still printed.
    let spool = Paths::default_home()
        .ok()
        .map(|p| hook::spool_dir(&p.data_dir));
    hook::statusline_main(spool.as_deref());
    Ok(())
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
    // When the user was last here: the newest entry before this run adds any.
    let away_since = ledger.ledger().head()?.map(|e| e.draft.ts);
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
    // Handoffs reach the sessions that start after a checkpoint.
    let board = HandoffBoard::new();
    let responders = Responders {
        stop: keeper.clone(),
        session_start: board.clone(),
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
    let worktree::Services {
        worktrees,
        checkpoints,
    } = worktree::services(
        &paths,
        &settings,
        store.clone(),
        ledger.clone(),
        clock.clone(),
        board,
    );
    let gate = GateService::new(gate_records, ledger.clone(), store.clone(), clock.clone())
        .with_checkpoints(checkpoints.clone());
    // What the sessions report themselves: status lines and loaded instructions.
    let truth = Rc::new(TruthBoard::new(clock.clone()));
    let statuslines: Rc<dyn StatuslineFeed> = Rc::new(CombinedStatusline(vec![
        server.clone(),
        Rc::new(hook::SpoolFeed::new(hook::spool_dir(&paths.data_dir))),
    ]));
    // Missions: contracts written into their repositories, linted by Haiku
    // (`--bare` only works with an API key), judged by the gate.
    let linter = HaikuLinter::new(doctor::claude_bin(), rt.handle().clone())
        .with_bare(std::env::var_os("ANTHROPIC_API_KEY").is_some());
    let evidence = evidence::service(
        store.clone(),
        ledger.clone(),
        clock.clone(),
        Some(checkpoints.clone()),
    );
    let missions = MissionService::new(Rc::new(MissionFiles::new(store.clone())), clock.clone())?
        .with_linter(Rc::new(linter))
        .with_gates(Rc::new(keeper.clone()))
        .with_ledger(ledger.clone())
        .with_evidence(evidence.clone());
    let hook_bus = HookBus::new(server.clone(), ledger, store.clone(), clock.clone())
        .with_checkpoints(checkpoints.clone())
        .with_truth(truth.clone());
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
    // Usage windows from the status lines and the jobs' own streams decide
    // what may start; work a limit stopped resumes after the reset.
    let governor = Governor::new(&settings.governor)
        .with_probe(truth.clone())
        .with_resume_book(store.clone())
        .with_utc_offset(swarmux_infrastructure::telemetry::localtime::utc_offset_minutes());
    // Collectors run from the start so the stations have data once shown.
    let telemetry = telemetry::start(&settings, &paths, store.clone(), clock.clone())
        .with_truth(truth, statuslines);
    let alarm_store = store.clone();
    let mut jobs = JobService::new(store.clone(), jobs_log, runner, clock)
        .with_telemetry_store(store)
        .with_limits(settings.limits())
        .with_loop_repeats(guard_settings.loop_repeats)
        .with_monitor(Rc::new(SysinfoMonitor::new()))
        .with_review(workspace, &settings.review)
        .with_agent_catalog(Rc::new(FsAgentCatalog::new(Some(paths.home.clone()))))
        .with_execution_log(Rc::new(keeper))
        .with_worktrees(worktrees)
        .with_governor(governor);
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
    .with_missions(missions)
    .with_evidence(evidence, away_since)
    .with_checkpoints(checkpoints)
    .with_alarms(alarm_store)
    .with_terminal(Rc::new(WindowsTerminal::new(doctor::claude_bin())))
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
