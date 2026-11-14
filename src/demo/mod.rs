//! `swarmux --demo`: the real interface over a scripted "overnight org".
//!
//! Everything runs in a throwaway folder. `claude`, git and the checks are
//! replaced by the adapters in `swarmux_infrastructure::demo`, and a
//! director plays a fixed script through the same inbox, ledger, gate and
//! evidence code the app uses for real work. Nothing here is a recording
//! of a real run, and the status bar says so.

mod director;
mod release;
mod script;
pub mod snapshot;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use swarmux_application::ledger::{LedgerService, LedgeredDecisions, LedgeredJobs};
use swarmux_application::ports::{Clock, SessionRepository};
use swarmux_application::{
    GateKeeper, GateService, GuardService, GuardSettings, HistoryService, JobService,
    MissionService, PromptService, RoutingService, SessionService, Settings, SharedPolicy,
    UiSettings,
};
use swarmux_domain::mission::{Acceptance, Budget, Constraints, Contract};
use swarmux_domain::{Session, SessionId};
use swarmux_infrastructure::demo::{
    DemoChecker, DemoClock, DemoFeed, DemoGateSource, DemoMissions, DemoMonitor, DemoPlanner,
    DemoPlans, DemoVcs, DemoWorkspace, ScriptedInbox, ScriptedRunner,
};
use swarmux_infrastructure::{EvidenceFiles, Paths, SqliteLedger, SqliteStore};
use swarmux_tui::{App, Launch};

use director::Director;

/// The tag shown in the status bar for as long as the demo runs.
pub const LABEL: &str = "DEMO · scripted";

/// The projects of the scripted org, as sessions.
pub const SESSIONS: [&str; 5] = [
    "api-gateway",
    "web-checkout",
    "docs-site",
    "data-pipeline",
    "mobile-app",
];

/// What the demo is built with. The defaults are what `--demo` uses.
#[derive(Clone, Debug)]
pub struct Options {
    /// A built-in theme name.
    pub theme: Option<String>,
    /// Seed of the made-up token counts.
    pub seed: u64,
    /// Start as if the user had been away this many minutes: the script
    /// plays through first, then the app attaches and shows the Return
    /// brief over what the script left in the ledger.
    pub away_minutes: Option<u32>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            theme: None,
            seed: 7,
            away_minutes: None,
        }
    }
}

/// Runs the demo in the terminal.
pub fn run(options: &Options) -> anyhow::Result<()> {
    Ok(swarmux_tui::run(build(options)?)?)
}

/// The demo app, ready to run, with its script loaded.
pub fn build(options: &Options) -> anyhow::Result<App> {
    let dir = tempfile::Builder::new()
        .prefix("swarmux-demo-")
        .tempdir()
        .context("cannot make the demo's temporary folder")?;
    let paths = Paths::from_home(dir.path());
    std::fs::create_dir_all(&paths.data_dir)?;

    let defaults = Settings::default();
    let settings = Settings {
        max_concurrent: SESSIONS.len(),
        guard: GuardSettings {
            // A held call waits for the whole scenario (the clock runs fast).
            approval_timeout_secs: 3600,
            ..defaults.guard.clone()
        },
        ui: UiSettings {
            splash: false,
            theme: options.theme.clone().unwrap_or(defaults.ui.theme.clone()),
            ..defaults.ui.clone()
        },
        ..defaults
    };

    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(DemoClock::new(script::START));
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    let jobs_log = Rc::new(LedgeredJobs::new(store.clone(), ledger.clone()));
    let decisions = Rc::new(LedgeredDecisions::new(store.clone(), ledger.clone()));

    // The real gate and guard, over a gate source and checker that follow the script.
    let checker = Arc::new(DemoChecker::default());
    let (keeper, gate_records) = GateKeeper::new(Arc::new(DemoGateSource), checker.clone());
    let keeper = Arc::new(keeper);
    let policy = SharedPolicy::new(settings.guard.policy()).with_fence(keeper.clone());
    let inbox = Rc::new(ScriptedInbox::default());
    let runner = Rc::new(ScriptedRunner::default());
    let gate = GateService::new(gate_records, ledger.clone(), store.clone(), clock.clone());
    let evidence = Rc::new(swarmux_application::EvidenceService::new(
        Rc::new(EvidenceFiles::new(store.clone())),
        ledger.clone(),
        store.clone(),
        Rc::new(DemoVcs),
        clock.clone(),
    ));

    let sessions = SessionService::new(store.clone(), clock.clone())
        .with_default_model(&settings.default_model);
    // The folders are not real: nothing in the demo reads or writes them,
    // and the same names show on every machine.
    let folder = |name: &str| PathBuf::from(format!("/work/{name}"));
    let mut cwds = Vec::new();
    let mut created = Vec::new();
    for (i, name) in SESSIONS.into_iter().enumerate() {
        // One second apart, so lists that sort by age always sort the same way.
        clock.set(script::START + 1_000 * i64::try_from(i)?);
        let cwd = folder(name);
        // Fixed ids, because the ledger records them and runs must not differ.
        let id = SessionId::parse(&format!("5eed0000-0000-4000-8000-{i:012}"))
            .context("a session id")?;
        let session = Session::new(id, name, cwd.clone(), &settings.default_model, clock.now())?;
        store.insert(&session)?;
        created.push(session);
        cwds.push((name, cwd));
    }

    // The mission whose gate blocks a premature "done" and then passes.
    // The second mission is a plan built as a task graph, kept as an
    // earlier run left it (see `release`); the Deck reads it like any other.
    let release_root = created[3].cwd.display().to_string();
    let mission_store =
        DemoMissions::default().with(release::mission(created[3].id, &release_root));
    let plan_store = DemoPlans::default().with(release::SLUG, release::plan());
    let mut missions = MissionService::new(Rc::new(mission_store), clock.clone())?
        .with_gates(Rc::new(keeper.clone()))
        .with_ledger(ledger.clone())
        .with_evidence(evidence.clone())
        .with_planner(Rc::new(DemoPlanner), Rc::new(plan_store))?;
    let api = &created[0];
    let slug = missions.create(
        Contract {
            title: "Fix invoice rounding".into(),
            goal: "Invoice totals round half up to the cent, as the finance spec says.".into(),
            acceptance: Acceptance {
                checks: vec!["cargo test".into()],
                human_signoff: false,
            },
            constraints: Constraints {
                paths_rw: vec!["src/**".into()],
                protected: vec!["tests/**".into()],
            },
            budget: Budget {
                usd: Some(5.0),
                wall_minutes: Some(90),
                ..Budget::default()
            },
            due: None,
            clarifications: Vec::new(),
            repos: Vec::new(),
        },
        &api.cwd.display().to_string(),
        Some(api.id),
    )?;

    let jobs = JobService::new(
        store.clone(),
        jobs_log.clone(),
        runner.clone(),
        clock.clone(),
    )
    .with_telemetry_store(store.clone())
    .with_limits(settings.limits())
    .with_loop_repeats(settings.guard.loop_repeats)
    .with_monitor(Rc::new(DemoMonitor))
    .with_execution_log(Rc::new(keeper.clone()));
    let guard = GuardService::new(
        policy.clone(),
        inbox.clone(),
        decisions.clone(),
        clock.clone(),
    )
    .with_timeout_secs(settings.guard.approval_timeout_secs);
    let routing = RoutingService::new(store.clone(), jobs_log.clone(), decisions, clock.clone());
    let prompts = PromptService::new(
        store.clone(),
        store.clone(),
        Rc::new(DemoWorkspace),
        clock.clone(),
    );
    let history = HistoryService::new(jobs_log);
    let telemetry =
        swarmux_application::TelemetryService::new(Rc::new(DemoFeed), store.clone(), clock.clone());

    let director = Director::new(director::Parts {
        _dir: dir,
        clock: clock.clone(),
        runner,
        inbox,
        keeper,
        policy,
        checker,
        cwds: cwds.into_iter().collect::<Vec<(&str, PathBuf)>>(),
        mission: slug,
        seed: options.seed,
    });
    let app = App::new(
        sessions,
        jobs,
        prompts,
        history,
        guard,
        routing,
        folder(SESSIONS[0]),
    )
    .with_clock(clock.clone())
    .with_gate(gate)
    .with_missions(missions)
    .with_evidence(evidence, None)
    .with_ledger(ledger.clone(), &settings.console)
    .with_alarms(store)
    .with_telemetry_feed(telemetry, &settings.telemetry)
    .with_demo(LABEL)
    .with_driver(director.into_driver());
    let app = app.with_look(
        &settings.ui,
        Launch {
            claude: "claude (scripted demo)".into(),
            no_splash: true,
            now_ms: script::START,
        },
    );
    // The demo is on its own: nothing of the user's config but their keys,
    // so `swarmux --demo` shows what their own keymap looks like.
    let keys = Paths::default_home()
        .ok()
        .and_then(|p| swarmux_infrastructure::load_settings(&p.config).ok())
        .map(|s| s.keys)
        .unwrap_or_default();
    let mut app = app.with_key_config(&keys);
    if let Some(minutes) = options.away_minutes {
        // What the scripted org did "while you were away": played in full,
        // through the same doors, before the user attaches.
        for _ in 0..AWAY_TICKS {
            app.update();
            app.frame += 1;
        }
        let at = app.now() - i64::from(minutes) * 60_000;
        app = app
            .with_away_since(at)
            .with_brief_config(&swarmux_domain::console::BriefConfig::default());
    }
    Ok(app)
}

/// Ticks the whole scenario takes, plus a little for the last job to settle.
const AWAY_TICKS: u32 = 340;

#[cfg(test)]
mod brief_tests;
#[cfg(test)]
mod deck_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod work_tests;
