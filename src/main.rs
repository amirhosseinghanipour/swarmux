//! Composition root: the only place that wires concrete adapters to use cases.

mod audit;
mod contracts;
mod daemon;
mod deck;
mod demo;
mod dist;
mod doctor;
mod evidence;
mod fetch;
mod followup;
mod gate;
mod guardcmd;
mod holdout;
mod keys;
mod knowledge;
mod stats;
mod telemetry;
mod theme;
mod trust;
mod workspace;
mod worktree;

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use swarmux_application::ledger::{
    LedgerService, LedgeredDecisions, LedgeredJobs, LedgeredRules, ingest_fallbacks,
};
use swarmux_application::ports::Clock;
use swarmux_application::{
    AlwaysSettings, Board, BoardDigest, BoardService, CombinedStatusline, GateKeeper, GateService,
    Governor, GuardService, HandoffBoard, HistoryService, HookBus, HookMessenger, JobService,
    MissionService, NotifyService, NudgeBoard, PromptService, RoutingService, ScopeClaims,
    SessionService, SharedPolicy, StatuslineFeed, TrustGuard, TruthBoard, UnattendedJobs, Watchdog,
};
use swarmux_infrastructure::hook::{self, HookServer, Responders};
use swarmux_infrastructure::trust::LedgerTrustReader;
use swarmux_infrastructure::{
    BoardMcp, ClaudeCli, ClaudePlanner, FilePalette, FsAgentCatalog, FsGateSource, GitCli,
    GitMergeQueue, GitRepoRoots, GitVcs, GitWorktrees, GuardHook, HaikuDiagnoser, HaikuExtractor,
    HaikuLinter, HaikuRouter, MissionFiles, Paths, SqliteLedger, SqliteStore, SysinfoMonitor,
    SystemClock, TomlRuleStore, WindowsTerminal, build_notifier_with, load_settings, rules_file,
    utc_offset_minutes,
};
use swarmux_tui::{App, Launch};

/// Claude Code may run the hook this much longer than an approval can wait.
const HOOK_GRACE_SECS: u64 = 10;

/// Autonomy you can verify. The control plane for autonomous AI agents.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Start without the splash screen.
    #[arg(long, global = true)]
    no_splash: bool,
    /// Play a scripted demo in a throwaway folder: no Claude Code, no
    /// account, and none of your data is used or changed.
    #[arg(long)]
    demo: bool,
    /// A built-in theme for the demo (see `swarmux theme list`).
    #[arg(long, requires = "demo", value_name = "NAME")]
    theme: Option<String>,
    /// Run the engine in this window instead of the background service:
    /// quitting stops its jobs.
    #[arg(long, conflicts_with = "demo")]
    no_daemon: bool,
    /// Write the demo's key moments as JSON frames into DIR instead of
    /// playing it (for the website and README images).
    #[arg(long, requires = "demo", value_name = "DIR")]
    snapshot: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// The background service: jobs, approvals and missions go on with no
    /// window open. `swarmux` attaches to it, starting it when needed.
    Daemon {
        #[command(subcommand)]
        action: daemon::Action,
    },
    /// Check that Claude Code is installed and swarmux's data is healthy.
    Doctor,
    /// The Guard's rules: list or remove the allow rules added with "always
    /// allow", and check the Guard's settings.
    Guard {
        #[command(subcommand)]
        action: guardcmd::Action,
    },
    /// A neofetch-style card: the logo, your sessions, spend, limits and
    /// ledger head, in the colours of your theme. Honours NO_COLOR.
    Fetch {
        /// A built-in theme, one from `[themes]`, `wal`, `base16:<file>` or
        /// `matugen:<file>` (default: the one in your config).
        #[arg(long)]
        theme: Option<String>,
        /// auto (colour on a terminal), always or never.
        #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
        color: String,
    },
    /// Themes: list them, or export one for another tool.
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
    /// Hotkeys: list them, check the `[keys]` of your config, or export the
    /// keys in use.
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },
    /// The Deck: check the `[deck]` of your config, or show its layout.
    Deck {
        #[command(subcommand)]
        action: DeckAction,
    },
    /// The tamper-evident ledger of everything swarmux observed or decided.
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
    /// Repository trust: which repositories may run the commands they
    /// ship (`.swarmux/gates.toml`, guessed checks). None do until you
    /// grant it.
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// The done-gate: the checks an agent must pass before it may stop.
    Gate {
        #[command(subcommand)]
        action: GateAction,
    },
    /// Hidden checks: run by the verifier in clean copies after the visible
    /// checks, never shown to agents (only how many failed).
    Holdout {
        #[command(subcommand)]
        action: HoldoutAction,
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
    /// Workspaces: a product folder of several git repositories, worked on
    /// as one (a session from its root sees every repository).
    Workspace {
        #[command(subcommand)]
        action: workspace::Action,
    },
    /// Rules, facts and decisions kept for a workspace's (or a
    /// repository's) agents: enforced by the Guard and the gate, or given
    /// to them as quoted text when they start and when they touch a file.
    Knowledge {
        #[command(subcommand)]
        action: Box<knowledge::Action>,
    },
    /// What agents left behind when they finished: the follow-ups waiting for
    /// you, approve/decline/defer them, the brief and the standing orders.
    Followup {
        #[command(subcommand)]
        action: followup::FollowAction,
    },
    /// Contracts between a workspace's repositories: what each provides
    /// (tables, columns, functions, endpoints, env keys) and who consumes it.
    Contracts(contracts::ContractsArgs),
    /// The rule registry: how many rules, enforced, failing, advisory-only.
    Rules {
        #[command(flatten)]
        home: knowledge::HomeArgs,
    },
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
    /// Put swarmux on this machine: copy it to a per-user folder, add the
    /// folder to your PATH and create ~/.swarmux. Does not touch Claude
    /// Code's settings.
    Install {
        /// Where to put the executable (default: a per-user folder).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Do not change your PATH.
        #[arg(long)]
        no_path: bool,
        /// Also print the `statusLine` setting for Claude Code's settings.json.
        #[arg(long)]
        statusline: bool,
    },
    /// Remove what `install` set up and say what was removed. Your data in
    /// ~/.swarmux stays unless you ask for --purge.
    Uninstall {
        /// Also delete ~/.swarmux (config, database, ledger).
        #[arg(long)]
        purge: bool,
        /// Leave your PATH alone.
        #[arg(long)]
        no_path: bool,
    },
    /// Update swarmux to the newest release, after checking its minisign
    /// signature against the key built into this binary.
    SelfUpdate {
        /// Only look for a newer release.
        #[arg(long)]
        check: bool,
        /// Take the release from a folder laid out like a release
        /// (latest.txt plus the archives and their .minisig files).
        #[arg(long, value_name = "DIR")]
        from: Option<PathBuf>,
    },
    /// The MCP server a mission job talks to the board through (not for
    /// direct use).
    #[command(hide = true)]
    BoardMcp {
        /// The swarmux job the agent runs as.
        #[arg(long)]
        job: Option<i64>,
    },
    /// Hooks Claude Code runs inside swarmux jobs (not for direct use).
    #[command(hide = true)]
    Hook {
        /// The hook event: pre-tool, stop, session-start, … (or Claude Code's own name).
        event: String,
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
    /// How much of what the agents read came from the prompt cache.
    Cache {
        /// How many days back to look.
        #[arg(long, default_value_t = 7)]
        days: i64,
    },
    /// How verification has gone: first-run gate passes, claim to
    /// verified, verified an hour, checks run, bundles sealed.
    Verification {
        /// How many hours back to look (default: `[console] metrics_hours`).
        #[arg(long)]
        hours: Option<u32>,
    },
}

#[derive(Subcommand)]
enum GateAction {
    /// Run the repository's checks now, in a clean copy with the hidden
    /// checks after them (as when an agent stops); exits with 1 if one fails.
    Run {
        /// A folder inside the repository (default: the current one).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Run the visible checks in the folder itself, without a clean copy
        /// or the hidden checks.
        #[arg(long)]
        in_place: bool,
        /// How far to go: fast, full (the default) or deep. A check runs when
        /// its level is at or below this one.
        #[arg(long)]
        level: Option<String>,
    },
    /// Make the changes an agent that wanted its work to count would make
    /// (a planted harness file, a deleted assertion) in scratch copies of
    /// the repository, and check that the gate refuses each. Exits with 1
    /// when it lets one through; the result goes into the ledger.
    Selftest {
        /// A folder inside the repository (default: the current one).
        dir: Option<PathBuf>,
    },
    /// Check `.swarmux/gates.toml`: what is wrong with it, and what was read
    /// with a fallback to a default. Exits with 1 when the file cannot be used.
    Lint {
        /// A folder inside the repository (default: the current one).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
    },
    /// Show the gate that applies: checks, protected paths, limits.
    Show {
        /// A folder inside the repository (default: the current one).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum HoldoutAction {
    /// Add a hidden check to a repository, or replace one of the same name.
    /// Its files are copied into the store and pinned by hash.
    Add {
        /// The repository: a folder inside it (or a worktree of it), or its
        /// name in the store.
        repo: String,
        /// The check's name.
        check: String,
        /// A file the check brings into the clean copy: `DEST=SOURCE`, where
        /// DEST is relative to the repository root, or one relative path
        /// for both. Repeat for more.
        #[arg(long = "file", value_name = "DEST=SOURCE")]
        files: Vec<String>,
        /// Seconds the check may run.
        #[arg(long)]
        timeout: Option<u64>,
        /// The command, after `--`; run directly, never through a shell.
        #[arg(last = true, required = true)]
        run: Vec<String>,
    },
    /// Show the hidden checks of one repository, or of all.
    List {
        /// A folder inside the repository, or its name in the store.
        repo: Option<String>,
    },
    /// Remove a hidden check.
    Rm { repo: String, check: String },
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
enum ThemeAction {
    /// The built-in themes and your own.
    List,
    /// Print a theme (default: the one in your config) as TOML for the
    /// config, as a base16 scheme, or as a kitty, alacritty or wezterm palette.
    Export {
        /// A built-in theme, one from `[themes]`, `wal`, `base16:<file>` or
        /// `matugen:<file>`.
        name: Option<String>,
        /// toml, base16, kitty, alacritty or wezterm.
        #[arg(long, short, default_value = "toml")]
        format: String,
    },
}

#[derive(Subcommand)]
enum KeysAction {
    /// Every action with its context, its keys now and by default; changed
    /// ones are marked.
    List,
    /// Check the `[keys]` of your config. Prints every problem and exits
    /// with 1 if there is one; the app still starts, with the default keys
    /// for what is wrong.
    Lint,
    /// Print the keys in use, defaults included, as TOML for the config.
    Export,
}

#[derive(Subcommand)]
enum TrustAction {
    /// Every repository you granted or revoked, and whether its files
    /// still match what you granted.
    List,
    /// What a repository wants to run, the hash of each file a grant would
    /// cover, and where it stands. Defaults to the current folder.
    Show { repo: Option<std::path::PathBuf> },
    /// Allow the commands a repository ships, for exactly the files `show`
    /// lists. A changed file asks again. Written to the ledger.
    Grant { repo: std::path::PathBuf },
    /// Take a grant back: a folder, or an id from `list`.
    Revoke { repo: String },
}

#[derive(Subcommand)]
enum DeckAction {
    /// Check the `[deck]` of your config. Prints every problem and exits
    /// with 1 if there is one; the app still starts, with the default for
    /// what is wrong.
    Lint,
    /// The panels of the Deck in the order they are drawn, by zone.
    Show,
}

#[derive(Subcommand)]
enum AuditAction {
    /// Check the whole hash chain and every signed checkpoint of it; exits
    /// with 1 if the chain is broken or the ledger was cut short or
    /// rewritten since a checkpoint.
    Verify {
        /// Also check against checkpoints kept elsewhere (the output of
        /// `export-checkpoints`, one per line).
        #[arg(long)]
        checkpoints: Option<PathBuf>,
    },
    /// Sign the ledger's head now and keep the checkpoint. Also done when
    /// a mission is delivered.
    Checkpoint,
    /// Print every checkpoint, one per line, to keep somewhere an agent
    /// cannot write (a gist, an email to yourself).
    ExportCheckpoints,
    /// Show the newest entries.
    Tail {
        /// How many entries.
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
}

fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    if cli.demo && cli.command.is_some() {
        anyhow::bail!("--demo plays on its own: run `swarmux --demo` without a command");
    }
    match cli.command {
        Some(Command::Audit {
            action: AuditAction::Checkpoint,
        }) => audit::checkpoint(),
        Some(Command::Audit {
            action: AuditAction::ExportCheckpoints,
        }) => audit::export_checkpoints(),
        Some(Command::Audit {
            action: AuditAction::Verify { checkpoints },
        }) => {
            return Ok(if audit::verify(checkpoints.as_deref())? {
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
        Some(Command::Stats {
            what: StatsAction::Cache { days },
        }) => stats::cache(days.max(1)),
        Some(Command::Stats {
            what: StatsAction::Verification { hours },
        }) => stats::verification(hours),
        Some(Command::Gate {
            action:
                GateAction::Run {
                    dir,
                    in_place,
                    level,
                },
        }) => {
            return Ok(if gate::run(dir, in_place, level.as_deref())? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Gate {
            action: GateAction::Selftest { dir },
        }) => {
            return Ok(if gate::selftest(dir)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Gate {
            action: GateAction::Lint { dir, strict },
        }) => {
            return Ok(if gate::lint(dir, strict)? {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::Gate {
            action: GateAction::Show { dir },
        }) => gate::show(dir),
        Some(Command::Holdout { action }) => match action {
            HoldoutAction::Add {
                repo,
                check,
                files,
                timeout,
                run,
            } => holdout::add(&repo, &check, &files, timeout, &run),
            HoldoutAction::List { repo } => holdout::list(repo.as_deref()),
            HoldoutAction::Rm { repo, check } => holdout::rm(&repo, &check),
        },
        Some(Command::Workspace { action }) => workspace::run(action),
        Some(Command::Knowledge { action }) => knowledge::run(*action),
        Some(Command::Followup { action }) => followup::run(action),
        Some(Command::Contracts(args)) => contracts::run(args),
        Some(Command::Rules { home }) => knowledge::rules(home),
        Some(Command::Worktree { action }) => match action {
            WorktreeAction::List => worktree::list(),
            WorktreeAction::Gc { dry_run } => worktree::gc(dry_run),
            WorktreeAction::Checkpoints { job, n } => worktree::checkpoints(job, n),
            WorktreeAction::Mode { session, mode } => worktree::mode(&session, mode),
        },
        None if cli.demo => {
            let options = demo::Options {
                theme: cli.theme,
                ..demo::Options::default()
            };
            match cli.snapshot {
                Some(dir) => {
                    for name in demo::snapshot::write(&options, &dir)? {
                        println!("{}", dir.join(format!("{name}.json")).display());
                    }
                    Ok(())
                }
                None => demo::run(&options),
            }
        }
        None if cli.no_daemon => run_tui(cli.no_splash),
        None => return daemon::attach(cli.no_splash),
        Some(Command::Daemon { action }) => return daemon::run(action),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Guard { action }) => {
            if !guardcmd::run(action)? {
                return Ok(ExitCode::FAILURE);
            }
            Ok(())
        }
        Some(Command::Fetch { theme, color }) => fetch::run(theme.as_deref(), &color),
        Some(Command::Keys { action }) => {
            return keys::run(match action {
                KeysAction::List => keys::Action::List,
                KeysAction::Lint => keys::Action::Lint,
                KeysAction::Export => keys::Action::Export,
            });
        }
        Some(Command::Trust { action }) => match action {
            TrustAction::List => trust::list(),
            TrustAction::Show { repo } => trust::show(repo),
            TrustAction::Grant { repo } => trust::grant(repo),
            TrustAction::Revoke { repo } => trust::revoke(repo),
        },
        Some(Command::Deck { action }) => {
            return deck::run(match action {
                DeckAction::Lint => deck::Action::Lint,
                DeckAction::Show => deck::Action::Show,
            });
        }
        Some(Command::Theme { action }) => match action {
            ThemeAction::List => theme::list(),
            ThemeAction::Export { name, format } => theme::export_cmd(name.as_deref(), &format),
        },
        Some(Command::Statusline { print_settings }) => statusline(print_settings),
        Some(Command::Install {
            dir,
            no_path,
            statusline,
        }) => dist::install(dir, no_path, statusline),
        Some(Command::Uninstall { purge, no_path }) => dist::uninstall(purge, no_path),
        Some(Command::SelfUpdate { check, from }) => dist::self_update(check, from),
        Some(Command::BoardMcp { job }) => {
            swarmux_infrastructure::board::board_mcp_main(job).context("board-mcp failed")?;
            Ok(())
        }
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

/// The engine and the interface in this window, without the background
/// service: quitting stops the jobs.
fn run_tui(no_splash: bool) -> anyhow::Result<()> {
    // Everything that can fail with a readable message happens before the
    // terminal switches into full-screen mode.
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    // One engine per data folder: not beside a background service.
    let _lock = daemon::engine_lock(&paths)?;
    let rt = runtime()?;
    let app = build_app(&rt, &paths, no_splash, None)?;
    Ok(swarmux_tui::run(app)?)
}

/// The runtime the engine's adapters run their I/O on.
fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

/// Builds the app: the engine with every adapter wired, and the interface
/// over it. `windows` is the background service's endpoint, when the app
/// runs there: terminal notifications then go to the windows attached.
fn build_app(
    rt: &tokio::runtime::Runtime,
    paths: &Paths,
    no_splash: bool,
    windows: Option<Arc<swarmux_infrastructure::daemon::WindowServer>>,
) -> anyhow::Result<App> {
    let paths = paths.clone();
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

    // The done-gate judges every stop and fences protected paths. Its
    // checks run in clean copies, followed by the hidden ones.
    let checker = holdout::checker();
    let (keeper, gate_records) = GateKeeper::new(Arc::new(FsGateSource::new()), checker.clone());
    // A repository the owner has not trusted runs none of the commands it
    // ships: the grants are read back from the ledger.
    let unattended = UnattendedJobs::new();
    let trust = Arc::new(TrustGuard::new(
        Arc::new(LedgerTrustReader::new(&paths.db)),
        Arc::new(|| SystemClock.now()),
    ));
    // Rules, facts and decisions: guard rules ask the policy first, gate
    // and lint rules add checks, and sessions get what is in scope.
    let knowledge = Rc::new(knowledge::service(store.clone(), ledger.clone())?);
    let hub = knowledge.hub();
    // Contracts between repositories: who consumes what a job touches, and
    // the cross-repo check of a mission's gate.
    let contracts = Rc::new(contracts::service(store.clone()).with_ledger(ledger.clone()));
    let contract_hub = contracts.hub();
    let keeper = Arc::new(
        keeper
            .with_verifier(holdout::verifier(&paths, checker))
            .with_rules(hub.clone())
            .with_contracts(contract_hub.clone())
            .with_trust(trust.clone())
            .with_unattended(unattended.clone()),
    );
    let guard_settings = &settings.guard;
    // Watchdog nudges reach an agent as the reason its next call is not run.
    let nudges = NudgeBoard::new();
    // Tasks of a mission's graph write only inside the files they claim.
    let claims = ScopeClaims::new();
    // What the Guard keeps from the agents' tools: swarmux's own commands and
    // files, and the files that steer agents. The jobs nobody watches fail
    // closed and may not write those.
    let roots = Arc::new(GitRepoRoots::new("git".into()).with_workspaces({
        let hub = hub.clone();
        Box::new(move |job| {
            hub.job(job)
                .filter(|i| !i.folders.is_empty())
                .map(|i| i.root)
        })
    }));
    let rule_store = Rc::new(LedgeredRules::new(
        Rc::new(TomlRuleStore::new(rules_file(&paths.data_dir))),
        ledger.clone(),
    ));
    let policy = SharedPolicy::new(guard_settings.policy())
        .with_fence(keeper.clone())
        .with_fence(claims.clone())
        .with_nudges(nudges.clone())
        .with_rules(hub.clone())
        .with_roots(roots)
        .with_unattended(unattended.clone())
        .with_protection(guardcmd::protection(&paths, &settings)?);
    let pipe = hook::endpoint_for(std::process::id());
    // Handoffs reach the sessions that start after a checkpoint.
    let board = HandoffBoard::new();
    // The Board: what the agents of a mission tell each other. Sessions that
    // start are given its digest after their handoff.
    let board_hub = settings.board.enabled.then(Board::new);
    let session_start: Arc<dyn swarmux_application::SessionStartContext> = match &board_hub {
        Some(hub) => Arc::new(BoardDigest::new(hub.clone(), Some(board.clone()))),
        None => board.clone(),
    };
    // The knowledge goes first: it stays the same, so the prompt caches.
    hub.set_inner(session_start);
    let responders = Responders {
        stop: keeper.clone(),
        session_start: hub.clone(),
        tool_context: Arc::new(swarmux_application::PullContext {
            knowledge: hub,
            contracts: contract_hub,
        }),
    };
    let server = Rc::new(
        HookServer::start_with(rt.handle(), &pipe, policy.clone(), responders)
            .with_context(|| format!("cannot listen on {pipe}"))?,
    );
    let board_mcp = BoardMcp {
        exe: std::env::current_exe().context("cannot find the swarmux executable")?,
        pipe: pipe.clone(),
    };
    let guard_hook = GuardHook {
        exe: std::env::current_exe().context("cannot find the swarmux executable")?,
        pipe,
        timeout_secs: guard_settings.approval_timeout_secs + HOOK_GRACE_SECS,
    };
    let mut cli = ClaudeCli::new(doctor::claude_bin(), rt.handle().clone())
        .with_guard(guard_hook)
        .with_guard_spool(hook::spool_file(&paths.data_dir))
        .with_on_failure_block(guard_settings.hook_on_failure_block);
    if board_hub.is_some() {
        cli = cli.with_board(board_mcp);
    }
    let runner = Rc::new(cli);
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
        .with_checkpoints(checkpoints.clone())
        .with_finisher(Rc::new(keeper.clone()));
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
        Some(Rc::new(audit::service(&paths)?)),
    );
    // Plan-first missions: a plan-mode run proposes, the user approves, and
    // the finished build is compared with the plan.
    let planner = ClaudePlanner::new(doctor::claude_bin(), rt.handle().clone())
        .with_bare(std::env::var_os("ANTHROPIC_API_KEY").is_some());
    let plan_dirs = {
        let checkpoints = checkpoints.clone();
        Rc::new(move |job, root: &std::path::Path| {
            checkpoints
                .worktree(job)
                .ok()
                .flatten()
                .filter(|w| w.removed_at.is_none())
                .map_or_else(|| root.to_path_buf(), |w| w.path)
        })
    };
    // A workspace's missions are kept in swarmux's folder: its root is often
    // not a repository.
    let mission_files =
        MissionFiles::new(store.clone()).with_workspace_dir(paths.data_dir.join("workspaces"));
    let missions = MissionService::new(Rc::new(mission_files), clock.clone())?
        .with_workspaces(store.clone())
        .with_linter(Rc::new(linter))
        .with_gates(Rc::new(keeper.clone()))
        .with_ledger(ledger.clone())
        .with_evidence(evidence.clone())
        .with_plan_settings(settings.missions.clone())
        .with_planner(Rc::new(planner), store.clone())?
        .with_changes(Rc::new(GitVcs::default()), plan_dirs)
        // Plans whose tasks can run side by side run as a task graph, merged
        // into a branch of swarmux's own.
        .with_graph(
            Rc::new(GitMergeQueue::under(
                settings
                    .worktree
                    .root
                    .clone()
                    .unwrap_or_else(|| paths.data_dir.join("worktrees")),
            )),
            worktrees.clone(),
            claims.clone(),
        );
    let missions = missions.with_watch(Rc::new(swarmux_application::WatchFan(vec![
        knowledge.clone(),
        contracts.clone(),
    ])));
    let mut missions = match &board_hub {
        Some(hub) => missions.with_board(hub.clone()),
        None => missions,
    };
    let board_service = board_hub
        .as_ref()
        .map(|hub| {
            BoardService::new(store.clone(), hub.clone(), store.clone(), clock.clone())
                .map(|s| Rc::new(s.with_claims(claims.clone())))
        })
        .transpose()?;
    // Follow-through: what finished agents leave behind, the owner's
    // standing orders and the brief. A small model only reads each finished
    // job's last message (`extract = false` turns even that off).
    let followups = settings.followups.enabled.then(|| {
        let extractor = settings.followups.extract.then(|| {
            HaikuExtractor::new(doctor::claude_bin(), rt.handle().clone())
                .with_model(&settings.followups.extract_model)
                .with_bare(std::env::var_os("ANTHROPIC_API_KEY").is_some())
        });
        Rc::new(followup::build(
            &settings,
            store.clone(),
            ledger.clone(),
            clock.clone(),
            extractor,
        ))
    });
    // Stuck agents: detectors on every job, and a ladder of recoveries,
    // cheapest first, before the inbox.
    let watchdog = Watchdog::new(
        settings.watchdog.clone(),
        Rc::new(HookMessenger(nudges)),
        clock.clone(),
    )
    .with_ledger(ledger.clone())
    .with_checkpoints(checkpoints.clone())
    .with_restore(Rc::new(GitWorktrees::default()))
    .with_diagnoser(Rc::new(
        HaikuDiagnoser::new(doctor::claude_bin(), rt.handle().clone())
            .with_bare(std::env::var_os("ANTHROPIC_API_KEY").is_some()),
    ));
    let hook_bus = HookBus::new(server.clone(), ledger.clone(), store.clone(), clock.clone())
        .with_checkpoints(checkpoints.clone())
        .with_truth(truth.clone());
    let hook_bus = match &board_service {
        Some(service) => hook_bus.with_board(service.clone()),
        None => hook_bus,
    };
    let board_inbox: Rc<dyn swarmux_application::BoardInbox> = server.clone();
    let mut guard = GuardService::new(policy, server, decisions.clone(), clock.clone())
        .with_timeout_secs(guard_settings.approval_timeout_secs)
        .with_rule_store(rule_store)
        .with_always(AlwaysSettings {
            scope: guard_settings.always_scope,
            expiry_days: guard_settings.always_expiry_days,
        });
    // Rules added in earlier runs, and the hook calls that fell back while
    // swarmux was not running.
    let mut startup_notes: Vec<String> = settings.warnings.clone();
    // Said at every start: what the Guard does when swarmux can't be reached.
    {
        use swarmux_application::FailMode;
        let (unattended, watched) = (
            guard_settings.on_unreachable.mode(true),
            guard_settings.on_unreachable.mode(false),
        );
        if unattended != FailMode::Closed || watched != FailMode::Closed {
            startup_notes.push(format!(
                "guard: if swarmux can't be reached, unattended jobs are {} and other jobs are {} (guard.on_unreachable)",
                if unattended == FailMode::Closed { "denied" } else { "left to Claude Code" },
                if watched == FailMode::Closed { "denied" } else { "left to Claude Code" },
            ));
        }
    }
    match guard.load_rules() {
        Ok(skipped) => startup_notes.extend(skipped.iter().map(|r| {
            format!(
                "guard rule {} ({}) can't be read and is not applied",
                r.id, r.text
            )
        })),
        Err(e) => startup_notes.push(format!("guard rules: {e}")),
    }
    match ingest_fallbacks(
        &hook::FileSpool::new(hook::spool_file(&paths.data_dir)),
        &ledger,
    ) {
        Ok(0) => {}
        Ok(n) => startup_notes.push(format!(
            "{n} tool call(s) fell back while swarmux was not reachable: see `swarmux audit tail` (policy entries, guard_fallback)"
        )),
        Err(e) => startup_notes.push(format!("guard fallbacks: {e}")),
    }
    let router = HaikuRouter::new(doctor::claude_bin(), rt.handle().clone());
    let routing = RoutingService::new(store.clone(), jobs_log.clone(), decisions, clock.clone())
        .with_router(Rc::new(router));

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
    let workspaces = workspace::service(store.clone());
    let mut jobs = JobService::new(store.clone(), jobs_log, runner, clock)
        .with_unattended(unattended, guard_settings.on_unreachable)
        .with_workspaces(store.clone())
        .with_telemetry_store(store)
        .with_limits(settings.limits())
        .with_loop_repeats(
            settings
                .watchdog
                .breaker_repeats(guard_settings.loop_repeats),
        )
        .with_monitor(Rc::new(SysinfoMonitor::new()))
        .with_review(workspace, &settings.review)
        .with_agent_catalog(Rc::new(FsAgentCatalog::new(Some(paths.home.clone()))))
        .with_execution_log(Rc::new(keeper))
        .with_worktrees(worktrees)
        .with_governor(governor)
        .with_starts(Rc::new(swarmux_application::StartFan(vec![
            knowledge.clone(),
            contracts.clone(),
        ])));
    if let Some(hub) = &board_hub {
        jobs = jobs.with_board(hub.clone());
    }
    jobs.recover()?;
    // Missions in flight when swarmux last stopped carry on where they were.
    let recovered = missions.recover(&mut jobs);
    let mut app = App::new(
        sessions,
        jobs,
        prompts,
        history,
        guard,
        routing,
        std::env::current_dir()?,
    )
    .with_clock(Rc::new(SystemClock))
    .with_workspaces(workspaces)
    .with_board(board_service.zip(Some(board_inbox)))
    .with_followups(followups)
    .with_knowledge(Some(knowledge))
    .with_contracts(Some(contracts))
    .with_trust(Some(trust))
    .with_hook_bus(hook_bus)
    .with_gate(gate)
    .with_watchdog(watchdog)
    .with_missions(missions)
    .with_evidence(evidence, away_since)
    .with_ledger(ledger, &settings.console)
    .with_utc_offset(utc_offset_minutes)
    .with_checkpoints(checkpoints)
    .with_alarms(alarm_store)
    .with_terminal(Rc::new(WindowsTerminal::new(doctor::claude_bin())))
    .with_notifier(NotifyService::new(
        build_notifier_with(
            &settings.notify,
            rt.handle(),
            windows.map(|w| {
                Box::new(swarmux_infrastructure::daemon::WindowNotifier::new(
                    w,
                    settings.notify.terminal,
                )) as Box<dyn swarmux_application::Notifier>
            }),
        ),
        settings.notify.clone(),
        utc_offset_minutes,
    ))
    .with_telemetry_feed(telemetry, &settings.telemetry);
    if let Some(e) = recovered.errors.into_iter().last() {
        app.error = Some(format!("picking missions up again: {e}"));
    }
    if !startup_notes.is_empty() {
        app.notice = Some(startup_notes.join("; "));
    }
    // `theme = "wal"` and friends: a file the theme is read from and watched.
    let app = match FilePalette::from_spec(&settings.ui.theme, &paths.home) {
        Some(palette) => app.with_palette_source(Box::new(palette)),
        None => app,
    };
    let app = app.with_look(
        &settings.ui,
        Launch {
            claude: doctor::claude_bin().display().to_string(),
            no_splash,
            now_ms: SystemClock.now(),
        },
    );
    // After the look: how long the warning about the keys shows depends on it.
    Ok(app
        .with_key_config(&settings.keys)
        .with_deck_config(&settings.deck))
}
