//! UI state and key handling. All work goes through application use cases.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use std::rc::Rc;

use swarmux_application::console::{ConsoleService, ConsoleSnapshot, Extra, Gathered};
use swarmux_application::ports::Clock;
use swarmux_application::{
    AppError, AppResult, Begun, CheckpointService, Engine, EvidenceService, FocusTracker,
    GateService, GuardService, HistoryService, HookBus, JobService, MissionService, NotifyService,
    PaletteSource, PendingApproval, PromptService, QueueEntry, QueueStats, RoutingService,
    SessionService, SessionTerminal, TelemetryService, TelemetrySettings, UiSettings,
    WorkspaceService,
};
use swarmux_domain::alarm::{Breakpoint, LoadMeter};
use swarmux_domain::evidence::{brief, heat_strip};
use swarmux_domain::keys::{
    Action, Catalog, Context, GLOBAL, KeyConfig, KeySpec, Keymap, Outcome, Resolver, actions,
};
use swarmux_domain::mission::Contract;
use swarmux_domain::{
    AttentionKind, JobId, JobProgress, Millis, Priority, PromptId, Session, SessionId,
};

use crate::dashboard::DashboardState;
use crate::focus::{Checkpoints, FocusState};
use crate::help::HelpState;
use crate::inbox::{Confirm, Decision};
use crate::keys::{
    BOARD_ANSWER, DECIDE, DECIDE_ALWAYS, DECIDE_CANCEL_PLAN, DECIDE_NO, DECIDE_YES, HELP,
    HELP_PAGE_DOWN, HELP_PAGE_UP, HELP_TOGGLE, INBOX_FOCUS_MODE, LOOK_NEXT_LAYOUT, LOOK_NEXT_THEME,
    PROMPT_ROUTE, PROMPT_SEND, SHOWCASE_CONSTELLATION, SHOWCASE_SPECTRUM,
};
use crate::ledger::{HEAT_DAYS, LedgerScreen};
use crate::mission::{Cadence, MissionState, Pane, Tile, wall_tiles};
use crate::missions::{Composer, ComposerAction, MissionsScreen};
use crate::nav::BackStack;
use crate::prompts::{PendingDispatch, PromptsScreen};
use crate::queue::QueueScreen;
use crate::routing::RoutePayload;
use crate::screens::{self, Changes, Handled, Planned};
use crate::showcase::{AgentInput, Alert, Mode as ShowMode, Refresh, Showcase};
use crate::telemetry::Telemetry;
use crate::theme::{ColorDepth, Theme};
use crate::ui::anim::Anim;
use crate::ui::grid::PanelId;
use crate::ui::input::{Input, InputAction};
use crate::ui::look::{IconHints, LOW_POWER_FPS, Look};
use crate::ui::motion::AutoMotion;
use crate::ui::splash::SplashInfo;

pub use crate::screens::Screen;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPurpose {
    NewSession,
    Prompt,
    /// A prompt swarmux sends to the session it picks.
    RoutedPrompt,
    /// A value for the template variable `prompts.pending` is asking for.
    Variable,
    /// The note that sends a proposed plan back for a new one.
    PlanNote,
    /// The note that sends a job's work back to its agent, or goes with
    /// the denial of a held call.
    SendBackNote,
    /// A message to a running agent (Work).
    WorkMessage,
    /// The answer to a question an agent put on the board.
    BoardAnswer,
    /// The ledger search; applied as it is typed.
    Search,
    /// The workspace a new session works on.
    WorkspaceSession,
    /// A new knowledge item, `kind: text`.
    KnowledgeAdd,
    /// The new text of a knowledge item.
    KnowledgeEdit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Normal,
    Input(InputPurpose, Input),
    /// An irreversible call waits for its target to be named.
    Confirm(Confirm),
    /// `s`: how long to shelve the selected item.
    Shelve,
}

pub struct App {
    pub screen: Screen,
    /// The screens the user came from; `Esc` goes back through them.
    nav: BackStack<Screen>,
    /// What the keys do: defaults, and the user's changes.
    pub(crate) keymap: Keymap,
    /// Every action the keymap can bind.
    catalog: Catalog,
    /// A half-typed chord, and what a key press means.
    pub(crate) resolver: Resolver,
    /// The key list `?` opens, while it is open.
    pub(crate) help: Option<HelpState>,
    /// How many entries of the user's `[keys]` could not be used.
    key_problems: usize,
    /// What to tell the user about them, and the frame it disappears at.
    key_warning: Option<(String, u64)>,
    /// Milliseconds the terminal session has been on screen: real time,
    /// unlike the clock the work runs by (which the demo moves by hand).
    /// What a half-typed chord waits in.
    pub(crate) ui_ms: Millis,
    /// When `ui_ms` was last advanced.
    ui_tick: Option<std::time::Instant>,
    /// Frames drawn so far; drives the spinners.
    pub frame: u64,
    pub quit: bool,
    pub mode: Mode,
    pub sessions: Vec<Session>,
    /// The workspaces sessions can work on, by name.
    pub workspaces: Vec<swarmux_domain::Workspace>,
    /// Where workspaces are read from; none, none are shown.
    workspace_service: Option<WorkspaceService>,
    /// Index of the selected session in `sessions`; always re-resolved from
    /// `selected_id` when the list changes.
    pub selected: usize,
    /// The session prompts go to. The list re-sorts as sessions are used,
    /// so the selection is kept by id, not by position.
    selected_id: Option<SessionId>,
    /// Last error, shown in the status bar until the next key press.
    pub error: Option<String>,
    /// Last confirmation, shown like `error` but not as a problem.
    pub notice: Option<String>,
    pub prompts: PromptsScreen,
    pub queue: QueueScreen,
    pub ledger: LedgerScreen,
    pub dashboard: DashboardState,
    /// Scroll, human events and the "you were here" spot of Focus.
    pub focus_state: FocusState,
    /// Mission control's inbox, change log and feeds.
    pub mission: MissionState,
    /// The Missions screen's selection and composer.
    pub missions_screen: MissionsScreen,
    /// The Board screen's selection.
    pub board_screen: crate::board::BoardScreen,
    /// The Knowledge screen's selection.
    pub knowledge_screen: crate::knowledge::KnowledgeScreen,
    /// The Follow-ups screen: cards, approved intents and the brief preview.
    pub followup_screen: crate::followups::FollowScreen,
    /// The Work screen: its selection, tab and scroll.
    pub work_screen: crate::work::WorkScreen,
    /// The Deck screen: its selection, focus and scroll.
    pub deck_screen: crate::deck::DeckScreen,
    /// The Return brief: its window, digest and view.
    pub brief: crate::brief::BriefScreen,
    /// What `[deck]` of the config asks of the Deck, checked.
    pub(crate) deck_layout: swarmux_domain::console::DeckLayout,
    /// Jobs, the Guard, hooks, the gate, the watchdog, missions and the
    /// board: the services that keep the work going.
    pub(crate) engine: Engine,
    /// The console read model: what the Work and Deck screens read.
    pub(crate) console: ConsoleService,
    /// The verification figures of the last hours, read from the ledger.
    metrics: Option<swarmux_application::metrics::MetricsService>,
    /// The notes the send-back line offers to start from (`[console]
    /// send_back_notes`).
    send_back_notes: Vec<String>,
    /// Minutes the local time is ahead of UTC, for the times the screens
    /// write in the user's own clock.
    pub(crate) utc_offset: fn() -> i32,
    /// Bundles, their verification and outcome labels.
    evidence: Option<Rc<EvidenceService>>,
    /// When the user was last here: what happened since is the digest.
    pub(crate) away_since: Option<Millis>,
    clock: Option<Rc<dyn Clock>>,
    pub(crate) session_service: SessionService,
    pub(crate) prompt_service: PromptService,
    history_service: HistoryService,
    pub routing: RoutingService,
    /// Prompts being routed, by ticket.
    pub(crate) routes: HashMap<u64, RoutePayload>,
    cwd: PathBuf,
    /// Theme, icons, layouts and frame rate.
    pub look: Look,
    /// Tweens and pulses; touched while rendering, driven by `frame`.
    pub anim: RefCell<Anim>,
    /// Data for the telemetry panels.
    pub telemetry: Telemetry,
    /// The dashboard panel `Tab` focused.
    pub focus: Option<PanelId>,
    /// A short message and the frame it disappears at.
    toast: Option<(String, u64)>,
    /// What the splash screen shows.
    pub splash: SplashInfo,
    /// The file the theme comes from (pywal, base16, matugen), watched for changes.
    palette: Option<PaletteWatch>,
    /// Opens a session in a terminal tab of its own.
    terminal: Option<Rc<dyn SessionTerminal>>,
    /// Where Focus reads a job's worktree and checkpoints.
    checkpoints: Option<Rc<CheckpointService>>,
    /// What was last read for the job Focus shows, and when.
    focus_checkpoints: Option<(JobId, Millis, Checkpoints)>,
    /// How much of their time the user spends in the inbox.
    pub(crate) load: LoadMeter,
    /// When a key was last pressed.
    pub(crate) last_input: Millis,
    /// The idle breakpoint has been noted since the last key.
    pub(crate) idle_noted: bool,
    /// Tells the user about new P1/P2 items while the window is away.
    pub(crate) notifier: Option<NotifyService>,
    /// Whether the terminal window has the user's attention.
    window: FocusTracker,
    /// The Constellation and Spectrum screens.
    pub showcase: Showcase,
    /// When the showcase scene was last rebuilt.
    showcase_at: Option<Millis>,
    /// Set in demo mode: the tag shown in the status bar.
    demo: Option<String>,
    /// Run by the background service: `q` lets the window go.
    service: bool,
    /// The splash is on in the settings and not turned off at start.
    splash_on: bool,
    /// `icons` and `font` as configured, for windows that bring their own hints.
    glyph_setting: (String, String),
    /// Runs at the start of every update (the scripted demo's director).
    driver: Option<Driver>,
}

/// The keys that answer whatever asks the user for a decision.
fn is_decision(action: Action) -> bool {
    matches!(
        action,
        DECIDE_YES | DECIDE_NO | DECIDE_ALWAYS | DECIDE_CANCEL_PLAN
    )
}

/// What still works while the constellation or the spectrum shows: answering
/// a decision, leaving, going to a screen, the theme, and the showcase's own
/// keys.
fn passes_showcase(action: Action) -> bool {
    is_decision(action)
        || action == actions::QUIT
        || action == actions::BACK
        || action == LOOK_NEXT_THEME
        || action == SHOWCASE_CONSTELLATION
        || action == SHOWCASE_SPECTRUM
        || screens::nav_target(action).is_some()
        || screens::PLANNED.iter().any(|p| p.action == action)
}

/// A script the app plays: runs at the start of every update.
type Driver = Box<dyn FnMut(&mut App)>;

/// The user counts as away after this long without opening swarmux.
const AWAY_AFTER_MS: Millis = 30 * 60_000;
/// The morning brief covers this long.
const BRIEF_MS: Millis = 24 * 3_600_000;

/// What the digest covers: the time since the user was last here when
/// that was a while ago, else the last day. And whether it is the former.
fn digest_window(away_since: Option<Millis>, now: Millis) -> (Millis, bool) {
    match away_since {
        Some(at) if now - at >= AWAY_AFTER_MS => (at, true),
        _ => (now - BRIEF_MS, false),
    }
}

/// A palette file and what it looked like when last read.
struct PaletteWatch {
    source: Box<dyn PaletteSource>,
    stamp: Option<u64>,
    /// The frame it was last looked at.
    checked: u64,
}

/// How often the palette file is looked at, in milliseconds.
const PALETTE_CHECK_MS: u64 = 1_000;

/// What the composition root knows at start-up.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// The Claude Code executable, as configured.
    pub claude: String,
    /// `--no-splash`.
    pub no_splash: bool,
    /// Now, in milliseconds since the epoch (for "jobs today").
    pub now_ms: i64,
}

/// How long the splash stays up, in milliseconds.
const SPLASH_MS: u64 = 1500;
/// Lines of diff Focus keeps.
const DIFF_LINES: usize = 2000;
/// Frames per second of the Constellation at full motion, at least. The
/// Spectrum's bars only move by the minute, so it needs fewer.
const CONSTELLATION_FPS: u16 = 12;
const SPECTRUM_FPS: u16 = 8;
/// The longest step the showcase's animation clock takes at once.
const SHOWCASE_MAX_STEP_MS: u64 = 250;
/// How long a toast stays up, in milliseconds.
const TOAST_MS: u64 = 2500;
/// How long the warning about the user's `[keys]` stays up.
const KEY_WARNING_MS: u64 = 12_000;
/// How often Focus re-reads the job's checkpoints.
const CHECKPOINTS_EVERY_MS: Millis = 1_000;

impl App {
    pub fn new(
        session_service: SessionService,
        jobs: JobService,
        prompt_service: PromptService,
        history_service: HistoryService,
        guard: GuardService,
        routing: RoutingService,
        cwd: PathBuf,
    ) -> App {
        let (keymap, catalog) =
            crate::keys::defaults().expect("the default keymap has no conflicts");
        let mut app = App {
            screen: Screen::default(),
            nav: BackStack::default(),
            keymap,
            catalog,
            resolver: Resolver::new(),
            help: None,
            key_problems: 0,
            key_warning: None,
            ui_ms: 0,
            ui_tick: None,
            frame: 0,
            quit: false,
            mode: Mode::Normal,
            sessions: Vec::new(),
            workspaces: Vec::new(),
            workspace_service: None,
            selected: 0,
            selected_id: None,
            error: None,
            notice: None,
            prompts: PromptsScreen::default(),
            queue: QueueScreen::default(),
            ledger: LedgerScreen::default(),
            dashboard: DashboardState::default(),
            focus_state: FocusState::default(),
            mission: MissionState::default(),
            missions_screen: MissionsScreen::default(),
            board_screen: Default::default(),
            knowledge_screen: Default::default(),
            followup_screen: Default::default(),
            work_screen: Default::default(),
            deck_screen: Default::default(),
            brief: Default::default(),
            deck_layout: Default::default(),
            engine: Engine::new(jobs, guard),
            console: ConsoleService::new(),
            metrics: None,
            send_back_notes: Vec::new(),
            utc_offset: || 0,
            evidence: None,
            away_since: None,
            clock: None,
            session_service,
            prompt_service,
            history_service,
            routing,
            routes: HashMap::new(),
            cwd,
            look: Look::default(),
            anim: RefCell::new(Anim::default()),
            telemetry: Telemetry::default(),
            terminal: None,
            checkpoints: None,
            focus_checkpoints: None,
            load: LoadMeter::default(),
            last_input: 0,
            idle_noted: false,
            notifier: None,
            window: FocusTracker::default(),
            showcase: Showcase::default(),
            showcase_at: None,
            demo: None,
            service: false,
            splash_on: false,
            glyph_setting: (String::new(), String::new()),
            driver: None,
            focus: None,
            toast: None,
            splash: SplashInfo::default(),
            palette: None,
        };
        app.refresh_sessions(None);
        app.refresh_prompts(None);
        app
    }

    /// Applies the look-and-feel settings and shows the splash unless it is
    /// turned off. Settings it can't use are reported in the status bar.
    pub fn with_look(mut self, ui: &UiSettings, launch: Launch) -> App {
        let palette = self.palette.as_mut().map(|watch| {
            watch.stamp = watch.source.stamp();
            watch.source.load()
        });
        let (look, problems) = Look::from_settings_with(
            ui,
            ColorDepth::from_env(),
            palette,
            &IconHints::from_env(&ui.font),
        );
        self.look = look;
        self.look.auto_motion = AutoMotion::from_env();
        if let Some(p) = problems.into_iter().next() {
            self.error = Some(p);
        }
        let day_start = launch.now_ms - launch.now_ms.rem_euclid(86_400_000);
        let jobs_today = self.history_service.list("").map_or(0, |rows| {
            rows.iter().filter(|r| r.created_at >= day_start).count()
        });
        self.splash = SplashInfo {
            claude: launch.claude,
            sessions: self.sessions.len(),
            jobs_today,
        };
        self.splash_on = ui.splash && !launch.no_splash;
        self.glyph_setting = (ui.icons.clone(), ui.font.clone());
        if ui.splash && !launch.no_splash {
            self.look.splash_until = Some(self.frame + self.look.frames(SPLASH_MS));
        }
        self
    }

    /// Gives the theme a file to come from (`theme = "wal"`, `"base16:<file>"`,
    /// `"matugen:<file>"`). Call it before [`App::with_look`]; the theme is
    /// then re-read whenever the file changes.
    pub fn with_palette_source(mut self, source: Box<dyn PaletteSource>) -> App {
        self.palette = Some(PaletteWatch {
            source,
            stamp: None,
            checked: 0,
        });
        self
    }

    /// Uses `keymap` in place of the default keys.
    pub fn with_keymap(mut self, keymap: Keymap) -> App {
        self.keymap = keymap;
        self.resolver.clear();
        self
    }

    /// Every action the keymap can bind.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The keys in use.
    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// Uses the defaults with the user's `[keys]` applied. What can't be
    /// used keeps its default keys, and the status bar says so for a while
    /// (the key list says so too), pointing to `swarmux keys lint`.
    pub fn with_key_config(mut self, config: &KeyConfig) -> App {
        let Ok(found) = crate::keys::effective(config) else {
            // The defaults themselves are wrong: a bug a test catches. Keep
            // the ones the app started with.
            return self;
        };
        self.keymap = found.keymap;
        self.catalog = found.catalog;
        self.resolver.clear();
        self.key_problems = found.problems.len();
        if self.key_problems > 0 {
            let n = self.key_problems;
            self.key_warning = Some((
                format!(
                    "run `swarmux keys lint`: {n} key setting{} in config.toml can't be used, \
                     the default keys apply there",
                    if n == 1 { "" } else { "s" }
                ),
                self.frame + self.look.frames(KEY_WARNING_MS),
            ));
        }
        self
    }

    /// How many entries of `[keys]` were left out.
    pub(crate) fn key_problems(&self) -> usize {
        self.key_problems
    }

    /// The warning about `[keys]`, while it shows.
    pub(crate) fn key_warning(&self) -> Option<&str> {
        self.key_warning
            .as_ref()
            .filter(|(_, until)| self.frame < *until)
            .map(|(text, _)| text.as_str())
    }

    /// How the first key of `action` is written, looking in the contexts
    /// active now first and then everywhere. None when it has no key.
    pub(crate) fn key_of(&self, action: Action) -> Option<String> {
        self.contexts()
            .into_iter()
            .chain(self.keymap.contexts())
            .find_map(|c| self.keymap.chords_of(c, action).next())
            .map(ToString::to_string)
    }

    /// The key that goes to `screen` and the key of `action` there, for a
    /// message like "press 6 then n".
    pub(crate) fn goto_keys(&self, screen: Screen, action: Action) -> (String, String) {
        (
            crate::keys::nav_label(&self.keymap, screen),
            crate::hints::key_or_name(self, action),
        )
    }

    /// What to say when a session is needed and there is none.
    pub(crate) fn need_session(&self) -> String {
        let (go, new) = self.goto_keys(Screen::Sessions, screens::sessions::NEW);
        format!("create a session first: press {go} then {new}")
    }

    /// Re-reads the palette file when it has changed (looked at about once a
    /// second), while the theme in use still comes from it.
    fn pump_palette(&mut self) {
        let every = self.look.frames(PALETTE_CHECK_MS);
        let Some(watch) = &mut self.palette else {
            return;
        };
        if !matches!(self.look.theme.name, "wal" | "base16" | "matugen")
            || self.frame < watch.checked + every
        {
            return;
        }
        watch.checked = self.frame;
        let stamp = watch.source.stamp();
        if stamp == watch.stamp {
            return;
        }
        watch.stamp = stamp;
        match watch.source.load() {
            Ok(palette) => {
                self.look.theme =
                    Theme::from_base16(&palette).with_transparent(self.look.transparent);
                let name = self.look.theme.name;
                self.show_toast(format!("theme reloaded · {name}"));
            }
            Err(e) => self.error = Some(format!("theme not reloaded: {e}")),
        }
    }

    /// Records the agents' lifecycle events and shows what they say.
    /// Shows workspaces (sessions grouped by them, their repositories on
    /// the wall) and lets the Sessions screen open a session on one.
    pub fn with_workspaces(mut self, workspaces: WorkspaceService) -> App {
        self.set_workspaces(workspaces);
        self
    }

    pub(crate) fn set_workspaces(&mut self, workspaces: WorkspaceService) {
        self.workspace_service = Some(workspaces);
        self.refresh_sessions(None);
    }

    pub fn with_hook_bus(mut self, bus: HookBus) -> App {
        self.engine.hooks = Some(bus);
        self
    }

    /// Watches running jobs for loops, stalls, full contexts and runaway
    /// spending, and climbs the recovery ladder for stuck ones. Its steps
    /// show in Focus and the change log; what it can't fix goes to the inbox.
    pub fn with_watchdog(mut self, watchdog: swarmux_application::Watchdog) -> App {
        self.set_watchdog(watchdog);
        self
    }

    pub(crate) fn set_watchdog(&mut self, watchdog: swarmux_application::Watchdog) {
        self.engine.set_watchdog(watchdog);
    }

    /// Records the done-gate's verdicts and shows them on the wall.
    pub fn with_gate(mut self, gate: GateService) -> App {
        self.set_gate(gate);
        self
    }

    pub(crate) fn set_gate(&mut self, gate: GateService) {
        self.engine.gate = Some(gate);
    }

    /// Lets `t` on Focus open the session in a new terminal tab.
    pub fn with_terminal(mut self, terminal: Rc<dyn SessionTerminal>) -> App {
        self.terminal = Some(terminal);
        self
    }

    /// Lets the Missions screen write, lint, run and follow missions.
    pub fn with_missions(mut self, mut missions: MissionService) -> App {
        if let Some(evidence) = &self.evidence {
            missions.set_evidence(evidence.clone());
        }
        self.console = ConsoleService::new().with_landing(missions.landing_policy());
        self.engine.missions = Some(missions);
        self
    }

    /// Lets the Work screen record the operator's notes in the ledger and
    /// show the verification figures worked out from it (over the window
    /// `[console] metrics_hours` sets), and offers `[console]
    /// send_back_notes` as starting points for a note.
    pub fn with_ledger(
        mut self,
        ledger: Rc<swarmux_application::ledger::LedgerService>,
        console: &swarmux_application::ConsoleSettings,
    ) -> App {
        self.metrics = Some(swarmux_application::metrics::MetricsService::new(
            ledger.clone(),
            console.metrics_hours,
        ));
        self.send_back_notes = console.send_back_notes.clone();
        self.engine.interventions.configure(console.intervene);
        self.work_screen.mode = console.intervene.mode;
        self.engine.ledger = Some(ledger);
        self
    }

    /// Writes the times the screens show (a mission's due time) in the
    /// user's clock: `offset` gives the minutes local time is ahead of UTC.
    pub fn with_utc_offset(mut self, offset: fn() -> i32) -> App {
        self.utc_offset = offset;
        self
    }

    /// The notes the send-back line offers.
    pub(crate) fn send_back_notes(&self) -> Vec<String> {
        self.send_back_notes.clone()
    }

    /// Shows evidence on the Ledger screen. `away_since` is when the
    /// user was last here (the ledger's newest entry before this run).
    pub fn with_evidence(
        mut self,
        evidence: Rc<EvidenceService>,
        away_since: Option<Millis>,
    ) -> App {
        self.attach_evidence(evidence, away_since);
        self
    }

    /// When the user was last here, if the app does not know from the
    /// ledger (the demo plays its night first and then says so).
    pub fn with_away_since(mut self, at: Millis) -> App {
        self.away_since = Some(at);
        self
    }

    /// Also lets the missions seal a bundle when they are delivered.
    pub(crate) fn attach_evidence(
        &mut self,
        evidence: Rc<EvidenceService>,
        away_since: Option<Millis>,
    ) {
        if let Some(missions) = &mut self.engine.missions {
            missions.set_evidence(evidence.clone());
        }
        self.evidence = Some(evidence);
        self.away_since = away_since;
    }

    /// Where bundles are verified and labelled, when evidence is set up.
    pub(crate) fn evidence(&self) -> Option<&Rc<EvidenceService>> {
        self.evidence.as_ref()
    }

    /// Every mission, oldest first.
    pub fn missions(&self) -> &[swarmux_domain::mission::Mission] {
        self.engine
            .missions
            .as_ref()
            .map_or(&[], MissionService::missions)
    }

    /// The mission service, for tests that set missions up directly.
    #[cfg(test)]
    pub(crate) fn mission_service(&mut self) -> &mut MissionService {
        self.engine
            .missions
            .as_mut()
            .expect("the test app has missions")
    }

    /// A lint is out for the mission.
    pub fn mission_linting(&self, slug: &str) -> bool {
        self.engine
            .missions
            .as_ref()
            .is_some_and(|m| m.linting(slug))
    }

    /// Lets Focus show the job's branch, worktree and checkpoints.
    pub fn with_checkpoints(mut self, checkpoints: Rc<CheckpointService>) -> App {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// The worktree and checkpoints last read for `job`.
    pub(crate) fn checkpoints_of(&self, job: JobId) -> Option<&Checkpoints> {
        self.focus_checkpoints
            .as_ref()
            .filter(|(j, _, _)| *j == job)
            .map(|(_, _, c)| c)
    }

    /// Re-reads the checkpoints of the job Focus shows: when it changes,
    /// and then once a second.
    fn refresh_checkpoints(&mut self) {
        let Some(service) = self.checkpoints.clone() else {
            return;
        };
        let Some(job) = self
            .on_focus()
            .then(|| self.current_job().map(|j| j.job_id))
            .flatten()
        else {
            return;
        };
        let now = self.now();
        let fresh = self
            .focus_checkpoints
            .as_ref()
            .is_some_and(|(j, at, _)| *j == job && now - at < CHECKPOINTS_EVERY_MS);
        if fresh {
            return;
        }
        match Checkpoints::load(&service, job) {
            Ok(c) => self.focus_checkpoints = Some((job, now, c)),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Sends notifications for new P1 and P2 items while the terminal is
    /// in the background.
    pub fn with_notifier(mut self, notifier: NotifyService) -> App {
        self.notifier = Some(notifier);
        self
    }

    /// Time for ages and countdowns; without it the screens show no ages.
    pub fn with_clock(mut self, clock: Rc<dyn Clock>) -> App {
        self.clock = Some(clock);
        self
    }

    /// Feeds Mission control with the machine-wide telemetry.
    pub fn with_telemetry_feed(
        mut self,
        feed: TelemetryService,
        settings: &TelemetrySettings,
    ) -> App {
        let mission = std::mem::take(&mut self.mission);
        self.mission = mission.with_feed(feed, Cadence::from_settings(settings));
        self.refresh_mission();
        self
    }

    /// Keeps the inbox's memory (shelved items, how asks were answered, the
    /// alarm log) in `store`.
    pub fn with_alarms(mut self, store: Rc<dyn swarmux_application::AlarmStore>) -> App {
        let now = self.now();
        let mission = std::mem::take(&mut self.mission);
        self.mission = mission.with_alarms(store, now);
        self.refresh_mission();
        self
    }

    /// Now, in milliseconds since the epoch.
    pub fn now(&self) -> Millis {
        self.clock.as_ref().map_or(0, |c| c.now())
    }

    /// The held tool call `y`/`n`/`a` answer: the selected one on Mission
    /// control, else the oldest.
    pub fn approval_target(&self) -> Option<PendingApproval> {
        let approvals = self.engine.guard.approvals();
        if self.screen == Screen::Work {
            // Work answers the call of the outcome it shows, never another.
            let request = self
                .work_selected()
                .and_then(|o| crate::work::held_request(&o))?;
            return approvals
                .into_iter()
                .find(|a| a.request.request_id == request);
        }
        if self.screen == Screen::Mission
            && let Some(item) = self.mission.selected_item()
            && item.kind == AttentionKind::Approval
            && let Some(found) = approvals
                .iter()
                .find(|a| Some(a.request.request_id) == item.request)
        {
            return Some(found.clone());
        }
        approvals.into_iter().next()
    }

    /// The snapshot built last (by [`App::update`]), for drawing, which
    /// cannot build one. `None` before the first update.
    pub(crate) fn console_current(&self) -> Option<&ConsoleSnapshot> {
        self.console.current()
    }

    /// The console read model as it stands: ranked outcome groups, the
    /// Guard's holds, the Deck's graph and the header figures, as plain
    /// data (no terminal types), the same data a daemon API would serve.
    ///
    /// Call it when the engine reported a change, not per frame: the
    /// snapshot is rebuilt only when what it reads changed, and is
    /// served as it was otherwise.
    pub fn console_snapshot(&mut self) -> &ConsoleSnapshot {
        let now = self.now();
        let gathered = Gathered::of(&self.engine);
        let src = gathered.sources(
            &self.engine,
            Extra {
                now,
                attention: self.mission.alarms().open(),
                evidence: self.evidence.as_deref(),
                telemetry: Some(&self.mission.snapshot),
                stream: self.history_service.repository(),
                metrics: self.metrics.as_mut().and_then(|m| m.current(now)),
            },
        );
        self.console.snapshot(&src)
    }

    pub(crate) fn refresh_mission(&mut self) {
        let now = self.now();
        let cwd = self.selected_session().map(|s| s.cwd.clone());
        let mut hook_changes = self
            .engine
            .hooks
            .as_ref()
            .map(HookBus::changes)
            .unwrap_or_default();
        let mut escalations = Vec::new();
        if let Some(dog) = &self.engine.watchdog {
            hook_changes.extend(dog.changes());
            escalations = dog.escalations();
        }
        if let Some(missions) = &self.engine.missions {
            escalations.extend(missions.plan_items(now));
        }
        escalations.extend(self.board_items());
        escalations.extend(self.knowledge_items());
        escalations.extend(self.followup_items());
        self.mission.refresh(
            (
                &self.engine.guard,
                &self.engine.jobs,
                self.engine.gate.as_ref(),
            ),
            (&hook_changes, escalations),
            now,
            cwd.as_deref(),
        );
    }

    /// New data for the telemetry panels.
    pub fn set_telemetry(&mut self, telemetry: Telemetry) {
        self.telemetry = telemetry;
    }

    /// The toast showing now, if any.
    pub fn toast(&self) -> Option<&str> {
        self.toast
            .as_ref()
            .filter(|(_, until)| self.frame < *until)
            .map(|(s, _)| s.as_str())
    }

    pub(crate) fn show_toast(&mut self, text: String) {
        self.toast = Some((text, self.frame + self.look.frames(TOAST_MS)));
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quit {
            self.update();
            let mut drew = std::time::Duration::ZERO;
            terminal.draw(|frame| {
                let started = std::time::Instant::now();
                crate::ui::draw(frame, &self);
                drew = started.elapsed();
            })?;
            self.look.auto_motion.observe_frame(drew);
            self.frame = self.frame.wrapping_add(1);
            if event::poll(self.frame_time())? {
                self.on_event(event::read()?);
            }
        }
        Ok(())
    }

    /// Handles one terminal event: a key, or the window gaining or losing focus.
    pub(crate) fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                self.window.on_input(self.now());
                self.on_key(key);
            }
            Event::FocusGained => self.window.on_focus(true),
            Event::FocusLost => self.window.on_focus(false),
            _ => {}
        }
    }

    /// Notifies from `mission.items`, which is the inbox after focus mode,
    /// deferral and shelving: only items that are due can notify.
    fn pump_notifications(&mut self) {
        let now = self.now();
        let unfocused = self.window.unfocused(now);
        if let Some(notifier) = &mut self.notifier
            && let Some(e) = notifier
                .observe(&self.mission.items, unfocused, now)
                .into_iter()
                .last()
        {
            self.error = Some(e.to_string());
        }
    }

    /// Marks the app as a scripted demo: `label` stays in the status bar so
    /// nothing on screen can be taken for real data.
    pub fn with_demo(mut self, label: &str) -> App {
        self.demo = Some(label.to_string());
        self
    }

    /// Marks the app as run by the background service: windows attach to
    /// it and `q` lets the window go while the work goes on.
    pub fn set_service(&mut self, on: bool) {
        self.service = on;
    }

    /// Run by the background service.
    pub fn in_service(&self) -> bool {
        self.service
    }

    /// Whether `q` (or Ctrl+C) was pressed since the last call.
    pub fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    /// Shows the splash again (a window attached), when it is on.
    pub fn splash_again(&mut self) {
        if self.splash_on {
            self.look.splash_until = Some(self.frame + self.look.frames(SPLASH_MS));
        }
    }

    /// The folder new sessions start in.
    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    /// Draws glyphs as a window's terminal wants them, for `icons = "auto"`.
    pub fn set_glyphs(&mut self, hints: &swarmux_application::daemon::GlyphHints) {
        let (icons, font) = &self.glyph_setting;
        let hints = crate::ui::look::IconHints {
            icons: hints.icons.clone(),
            nerd_font: hints.nerd_font.clone(),
            font: if font.trim().is_empty() {
                hints.font.clone().unwrap_or_default()
            } else {
                font.clone()
            },
        };
        self.look.icons = crate::ui::look::IconSet::resolve(icons, &hints);
    }

    /// The services that keep the work going.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// The demo tag, when this is a scripted demo.
    pub fn demo_label(&self) -> Option<&str> {
        self.demo.as_deref()
    }

    /// Gives the app a script to play: `driver` runs at the start of every
    /// update and may dispatch prompts and launch missions.
    pub fn with_driver(mut self, driver: impl FnMut(&mut App) + 'static) -> App {
        self.driver = Some(Box::new(driver));
        self
    }

    /// Sends `prompt` to the session called `name`, as if typed for it.
    pub fn dispatch_to(&mut self, name: &str, prompt: &str) -> AppResult<JobId> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.name == name)
            .cloned()
            .ok_or(AppError::SessionNotFound)?;
        let job = self.engine.jobs.dispatch(session.id, prompt)?;
        self.focus_state.note_human(job, self.now());
        self.refresh_sessions(None);
        Ok(job)
    }

    /// Runs the mission called `slug` (it must be linted).
    pub fn run_mission(&mut self, slug: &str) -> AppResult<JobId> {
        let Some(service) = &mut self.engine.missions else {
            return Err(AppError::Mission("missions are not set up".into()));
        };
        let job = service.launch(slug, &mut self.engine.jobs)?;
        self.focus_state.note_human(job, self.now());
        self.refresh_sessions(None);
        Ok(job)
    }

    /// Presses a character key, as the user would.
    pub fn press_char(&mut self, c: char) {
        self.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    /// Presses a named key, as the user would: `enter`, `esc`, `tab` or
    /// `backspace`, `down` or `up`. Any other name is ignored.
    pub fn press_named(&mut self, name: &str) {
        let code = match name {
            "enter" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "down" => KeyCode::Down,
            "up" => KeyCode::Up,
            _ => return,
        };
        self.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The screen as text (one line per row) at the given size, for
    /// headless runs and tests.
    pub fn render_text(&self, width: u16, height: u16) -> String {
        crate::ui::render_to_string(self, width, height)
    }

    /// The screen as cells with their colours, for comparing two runs.
    pub fn render_buffer(&self, width: u16, height: u16) -> ratatui::buffer::Buffer {
        crate::ui::render_buffer(self, width, height)
    }

    /// Applies pending job updates, starts queued jobs and handles guard requests.
    pub fn update(&mut self) {
        let tick = std::time::Instant::now();
        if let Some(last) = self.ui_tick.replace(tick) {
            self.ui_ms += Millis::try_from(tick.duration_since(last).as_millis()).unwrap_or(0);
        }
        if let Some(mut driver) = self.driver.take() {
            driver(self);
            self.driver = Some(driver);
        }
        let report = self.engine.pump();
        let mut errors = report.errors;
        errors.extend(self.pump_routes());
        if let Some(e) = errors.into_iter().last() {
            self.error = Some(e.to_string());
        }
        if report.sessions_changed {
            // Jobs touch their sessions, which re-sorts the list.
            self.refresh_sessions(None);
        }
        let changes = Changes {
            jobs: report.jobs_changed,
            evidence: self.pump_evidence(),
        };
        self.pump_palette();
        self.resolver.expire(self.ui_ms);
        self.refresh_mission();
        self.tick_inbox();
        self.pump_notifications();
        self.pump_followups();
        self.screen.def().tick(self, changes);
        if let Some(e) = self.pump_missions().into_iter().last() {
            self.error = Some(e.to_string());
        }
        self.refresh_checkpoints();
        // After the missions moved: the Work screen and the tab counts read
        // the snapshot, which is rebuilt only when what it reads changed.
        self.work_refresh();
        self.sense_motion();
        self.refresh_showcase();
        let on_focus = (self.screen == Screen::Dashboard && !self.dashboard.classic)
            .then(|| self.current_job().map(|j| (j.job_id, j.log.len())))
            .flatten();
        self.focus_state.observe(on_focus, self.frame);
    }

    /// Time between frames: slow while a showcase screen stands still,
    /// smoother while one moves.
    pub fn frame_time(&self) -> std::time::Duration {
        use std::time::Duration;
        if !self.showcase.active() {
            return self.look.frame_time();
        }
        if self.showcase.frozen() {
            return Duration::from_millis(250);
        }
        let fps = match (self.look.animated(), self.showcase.mode) {
            (true, Some(ShowMode::Constellation)) => self.look.fps().max(CONSTELLATION_FPS),
            (true, _) => self.look.fps().clamp(LOW_POWER_FPS, SPECTRUM_FPS),
            (false, _) => self.look.fps(),
        };
        Duration::from_millis(1000 / u64::from(fps.max(1)))
    }

    /// Tells `motion = "auto"` whether the machine runs on battery.
    fn sense_motion(&mut self) {
        let on_battery = self
            .mission
            .snapshot
            .system
            .data
            .as_ref()
            .and_then(|s| s.battery.as_ref())
            .map(|b| !b.plugged_in && !b.charging);
        self.look.auto_motion.set_battery(on_battery);
    }

    /// The first P1 or P2 item in the inbox: it holds the showcase still.
    fn showcase_alert(&self) -> Option<Alert> {
        self.mission
            .items
            .iter()
            .find(|i| matches!(i.priority, Priority::P1 | Priority::P2))
            .map(|i| Alert {
                priority: i.priority,
                text: i.title.clone(),
            })
    }

    /// Rebuilds the showcase scene from the wall while a showcase screen is up.
    pub(crate) fn refresh_showcase(&mut self) {
        if !self.showcase.active() {
            self.showcase_at = None;
            return;
        }
        let now = self.now();
        let dt_ms = match (self.showcase_at, &self.clock) {
            (Some(at), Some(_)) => u64::try_from(now - at)
                .unwrap_or(0)
                .min(SHOWCASE_MAX_STEP_MS),
            (Some(_), None) => self.look.frame_time().as_millis() as u64,
            (None, _) => 0,
        };
        self.showcase_at = Some(now);
        let alert = self.showcase_alert();
        let agents = {
            self.wall()
                .into_iter()
                .map(|tile| {
                    let activity = tile.job.and_then(|j| self.engine.jobs.job_activity(j));
                    let tool_ages_ms = activity.map_or_else(Vec::new, |a| {
                        a.tools
                            .ticker(8)
                            .iter()
                            .map(|c| u64::try_from(now - c.started_at).unwrap_or(0))
                            .collect()
                    });
                    let io = activity.map_or_else(Vec::new, |a| {
                        a.flow
                            .series(now)
                            .iter()
                            .map(|b| (b.input + b.cache_write, b.output))
                            .collect()
                    });
                    AgentInput {
                        tile,
                        tool_ages_ms,
                        io,
                    }
                })
                .collect()
        };
        let spend_today = self.mission.usage().map(|u| u.today.cost);
        self.showcase.refresh(Refresh {
            agents,
            alert,
            tier: self.look.tier(),
            dt_ms,
            spend_today,
        });
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    /// The job the dashboard follows, else the most recent one.
    pub fn current_job(&self) -> Option<&JobProgress> {
        if let Some(id) = self.dashboard.followed
            && let Some(p) = self.engine.jobs.progress(id).or(self.dashboard.replayed())
        {
            return Some(p);
        }
        self.engine.jobs.latest()
    }

    /// The review run of the current job, if there is one.
    pub fn review_run(&self) -> Option<&JobProgress> {
        let job = self.current_job()?;
        if let Some(r) = self.dashboard.replayed_review()
            && self.dashboard.replayed().map(|p| p.job_id) == Some(job.job_id)
        {
            return Some(r);
        }
        self.engine.jobs.review_progress(job.job_id)
    }

    /// Where the current job's timeline ends (now, while anything runs).
    pub fn timeline_end(&self) -> u32 {
        let Some(job) = self.current_job() else {
            return 0;
        };
        match self.engine.jobs.progress(job.job_id) {
            Some(_) => self.engine.jobs.timeline_end(job.job_id),
            None => self
                .review_run()
                .map_or(0, |r| r.log_offset + r.last_tick)
                .max(job.last_tick),
        }
    }

    /// Whether the next prompts get a review.
    pub fn review_next(&self) -> bool {
        self.engine.jobs.review_next()
    }

    /// The reviewer's model.
    pub fn review_model(&self) -> &str {
        self.engine.jobs.review_model()
    }

    /// Running, queued and recently finished jobs.
    pub fn queue_rows(&self) -> Vec<QueueEntry> {
        self.engine.jobs.queue()
    }

    pub fn stats(&self) -> QueueStats {
        self.engine.jobs.stats()
    }

    /// The session a job belongs to, if the job is from this run.
    pub fn session_of(&self, job: JobId) -> Option<&str> {
        self.engine
            .jobs
            .progress(job)
            .map(|p| p.session_name.as_str())
    }

    /// The timeline's "now" for the current job.
    pub fn now_tick(&self) -> u32 {
        self.current_job()
            .map_or(0, |j| self.engine.jobs.tick_now(j.job_id).max(j.last_tick))
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Ctrl+C is reserved: it quits wherever the user is, typing included.
        if crate::keys::spec_of(key) == Some(KeySpec::ctrl('c')) {
            self.quit = true;
            return;
        }
        if self.look.splash_at(self.frame) {
            // Any key just dismisses the splash.
            self.look.splash_until = None;
            return;
        }
        self.last_input = self.now();
        self.idle_noted = false;
        if matches!(self.mode, Mode::Confirm(_)) {
            let result = self.confirm_key(key);
            self.show(result);
            return;
        }
        if self.mode == Mode::Shelve {
            let result = self.shelve_key(key);
            self.show(result);
            return;
        }
        // The note line also takes keys that pick a suggested note.
        if matches!(self.mode, Mode::Input(InputPurpose::SendBackNote, _))
            && self.work_note_key(key)
        {
            return;
        }
        // The message line takes Tab to switch between queue and steer.
        if matches!(self.mode, Mode::Input(InputPurpose::WorkMessage, _))
            && self.work_message_key(key)
        {
            return;
        }
        if let Mode::Input(purpose, input) = &mut self.mode {
            let purpose = *purpose;
            match input.handle(key) {
                InputAction::None => {
                    if purpose == InputPurpose::Search {
                        self.ledger.query = input.text.clone();
                        self.refresh_ledger();
                    }
                }
                InputAction::Cancel => {
                    self.mode = Mode::Normal;
                    self.prompts.pending = None;
                    if purpose == InputPurpose::Search {
                        self.ledger.query.clear();
                        self.refresh_ledger();
                    }
                    if purpose == InputPurpose::SendBackNote {
                        self.work_note_cancelled();
                    }
                    if purpose == InputPurpose::WorkMessage {
                        self.work_message_cancelled();
                    }
                }
                InputAction::Submit(text) => {
                    self.mode = Mode::Normal;
                    let result = match purpose {
                        InputPurpose::NewSession => self.create_session(&text),
                        InputPurpose::Prompt => self.dispatch(&text),
                        InputPurpose::RoutedPrompt => self.route_prompt(&text),
                        InputPurpose::Variable => self.answer_variable(text),
                        InputPurpose::PlanNote => self.submit_plan_note(&text),
                        InputPurpose::SendBackNote => self.submit_send_back(&text),
                        InputPurpose::WorkMessage => self.submit_message(&text),
                        InputPurpose::BoardAnswer => self.submit_board_answer(&text),
                        InputPurpose::Search => Ok(()),
                        InputPurpose::WorkspaceSession => self.create_workspace_session(&text),
                        InputPurpose::KnowledgeAdd => self.submit_knowledge_add(&text),
                        InputPurpose::KnowledgeEdit => self.submit_knowledge_edit(&text),
                    };
                    self.show(result);
                }
            }
            return;
        }
        if self.screen.def().edit_key(self, key) {
            return;
        }

        self.error = None;
        self.notice = None;
        self.dispatch_key(key);
    }

    /// Normal mode: the keymap says what the key means, and the first
    /// action that applies runs.
    fn dispatch_key(&mut self, key: KeyEvent) {
        let outcome = match crate::keys::spec_of(key) {
            Some(spec) => {
                // The key list is modal: only its own keys and the global ones.
                let stack = if self.help.is_some() {
                    vec![HELP, GLOBAL]
                } else {
                    self.contexts()
                };
                self.resolver.feed(&self.keymap, &stack, spec, self.ui_ms)
            }
            None => Outcome::Unbound,
        };
        let mut delete_checked = false;
        match outcome {
            Outcome::Actions(found) => {
                for action in found {
                    // The decision overlays answer before anything else
                    // sees the key, so they leave a pending delete alone.
                    if !is_decision(action) && !delete_checked {
                        self.check_delete_armed();
                        delete_checked = true;
                    }
                    if self.perform(action) == Handled::Yes {
                        return;
                    }
                }
            }
            Outcome::Cancelled(keys) => {
                // Esc just drops the chord; any other key says why.
                if keys
                    .last()
                    .is_some_and(|k| !swarmux_domain::keys::is_reserved(*k))
                {
                    let typed: Vec<String> = keys.iter().map(ToString::to_string).collect();
                    self.notice = Some(format!("{} is not bound", typed.join(" ")));
                }
            }
            Outcome::Pending | Outcome::Unbound => {}
        }
        if !delete_checked {
            self.check_delete_armed();
        }
    }

    /// A key that is not an answer: a pending delete is confirmed only by
    /// the very next key, if that is `x` again.
    fn check_delete_armed(&mut self) {
        self.prompts.delete_armed = std::mem::take(&mut self.prompts.confirm_delete);
    }

    /// The contexts whose keys apply now, innermost first: the decision
    /// overlays, the screen's own, the global ones. While the constellation
    /// or the spectrum shows only the overlays and the global keys do.
    pub(crate) fn contexts(&self) -> Vec<Context> {
        let mut stack = vec![DECIDE];
        if self.showcase.mode.is_none() {
            stack.extend(self.screen.def().contexts(self));
        }
        stack.push(GLOBAL);
        stack
    }

    /// Runs `action`: the screen first, then what every screen shares.
    fn perform(&mut self, action: Action) -> Handled {
        if self.help.is_some() {
            return self.help_action(action);
        }
        if self.showcase.mode.is_some() && !passes_showcase(action) {
            // The constellation and the spectrum take every other key.
            return Handled::Yes;
        }
        if self.screen.def().handle(self, action) == Handled::Yes {
            return Handled::Yes;
        }
        self.handle_shared(action)
    }

    /// What a key does while the key list shows: scroll it, close it, leave.
    /// Nothing else gets through.
    fn help_action(&mut self, action: Action) -> Handled {
        let scroll = |app: &App, by: isize| {
            if let Some(help) = &app.help {
                help.scroll.set(help.scroll.get().saturating_add_signed(by));
            }
        };
        match action {
            HELP_TOGGLE | actions::BACK => self.help = None,
            actions::QUIT => self.quit = true,
            actions::LIST_PREV => scroll(self, -1),
            actions::LIST_NEXT => scroll(self, 1),
            HELP_PAGE_UP => scroll(self, -10),
            HELP_PAGE_DOWN => scroll(self, 10),
            _ => {}
        }
        Handled::Yes
    }

    fn handle_shared(&mut self, action: Action) -> Handled {
        match action {
            actions::QUIT => self.quit = true,
            HELP_TOGGLE => self.help = Some(HelpState::default()),
            actions::BACK => self.go_back(),
            DECIDE_YES | DECIDE_NO | DECIDE_ALWAYS | DECIDE_CANCEL_PLAN => {
                return self.decide(action);
            }
            LOOK_NEXT_THEME => {
                let name = self.look.next_theme().name;
                self.show_toast(format!("theme · {name}"));
            }
            LOOK_NEXT_LAYOUT => {
                let name = self.look.next_layout().name.clone();
                self.show_toast(format!("layout · {name}"));
            }
            INBOX_FOCUS_MODE => self.toggle_focus_mode(),
            SHOWCASE_CONSTELLATION | SHOWCASE_SPECTRUM => return self.showcase_action(action),
            PROMPT_SEND | PROMPT_ROUTE => self.start_prompt(action == PROMPT_ROUTE),
            BOARD_ANSWER => return self.board_answer(),
            other => {
                if let Some(to) = screens::nav_target(other) {
                    self.showcase.mode = None;
                    self.navigate(to);
                } else if let Some(planned) = screens::PLANNED.iter().find(|p| p.action == other) {
                    self.showcase.mode = None;
                    self.go_planned(planned);
                } else {
                    return Handled::No;
                }
            }
        }
        Handled::Yes
    }

    /// The answers to whatever is asking: a proposed plan first, then the
    /// oldest held tool call. Does nothing, and lets the key through, when
    /// nothing is asking.
    fn decide(&mut self, action: Action) -> Handled {
        let answer = match action {
            DECIDE_YES => Decision::Yes,
            DECIDE_NO => Decision::No,
            DECIDE_ALWAYS => Decision::Always,
            _ => Decision::Cancel,
        };
        if answer != Decision::Always
            && let Some(slug) = self.plan_target()
        {
            let result = self.answer_plan(answer, &slug);
            self.show(result);
            return Handled::Yes;
        }
        if answer != Decision::Cancel && self.engine.guard.pending_count() > 0 {
            let result = self.answer_approval(answer);
            self.show(result);
            return Handled::Yes;
        }
        Handled::No
    }

    /// Opens the constellation or the spectrum from a screen that allows
    /// it, switches between them, or closes the one that shows.
    fn showcase_action(&mut self, action: Action) -> Handled {
        let target = if action == SHOWCASE_CONSTELLATION {
            ShowMode::Constellation
        } else {
            ShowMode::Spectrum
        };
        match self.showcase.mode {
            None if !self.screen.def().allows_showcase() => return Handled::No,
            Some(current) if current == target => self.showcase.mode = None,
            _ => {
                self.showcase.mode = Some(target);
                self.showcase_at = None;
                self.refresh_showcase();
            }
        }
        Handled::Yes
    }

    /// Esc: leaves the constellation or the spectrum, else drops the
    /// highlighted panel, else returns to the screen the user came from.
    fn go_back(&mut self) {
        if self.showcase.mode.take().is_some() || self.focus.take().is_some() {
            return;
        }
        if let Some(previous) = self.nav.back() {
            self.switch_to(previous);
        }
    }

    /// Shows `to`, remembering the screen being left for `Esc`.
    pub(crate) fn navigate(&mut self, to: Screen) {
        self.nav.visit(self.screen, to);
        self.switch_to(to);
    }

    /// Shows `to` without remembering the screen being left: `Esc` there
    /// does not come back.
    pub(crate) fn skip_to(&mut self, to: Screen) {
        self.switch_to(to);
    }

    fn switch_to(&mut self, to: Screen) {
        let left_focus = self.on_focus();
        self.screen = to;
        if left_focus && !self.on_focus() {
            self.note_breakpoint(Breakpoint::FocusClosed);
        }
        to.def().enter(self);
    }

    /// A screen that is planned but not built: go to the nearest one that
    /// exists, and say so.
    fn go_planned(&mut self, planned: &Planned) {
        match planned.stand_in {
            Some(to) => {
                self.navigate(to);
                self.notice = Some(format!(
                    "{} is not built yet: showing {}",
                    planned.name,
                    to.title()
                ));
            }
            None => self.notice = Some(format!("{} is not built yet", planned.name)),
        }
    }

    /// Asks for a prompt to send: to the selected session, or to the one
    /// swarmux picks.
    fn start_prompt(&mut self, routed: bool) {
        if self.screen == Screen::Mission {
            self.select_inbox_session();
        }
        if self.sessions.is_empty() {
            self.error = Some(self.need_session());
        } else {
            let purpose = if routed {
                InputPurpose::RoutedPrompt
            } else {
                InputPurpose::Prompt
            };
            self.mode = Mode::Input(purpose, Input::default());
        }
    }

    /// Selects the previous or next session.
    pub(crate) fn move_session(&mut self, delta: isize) {
        if delta < 0 {
            self.select_index(self.selected.saturating_sub(1));
        } else if self.selected + 1 < self.sessions.len() {
            self.select_index(self.selected + 1);
        }
    }

    /// Takes the linter's replies, follows the missions' jobs and stops
    /// the jobs of missions out of budget.
    fn pump_missions(&mut self) -> Vec<AppError> {
        let pumped = self
            .engine
            .pump_missions(&self.mission.facts, &self.mission.gates);
        if let (Some(composer), Some(service)) =
            (&mut self.missions_screen.composer, &self.engine.missions)
            && let Some(m) = composer.slug.as_deref().and_then(|s| service.get(s))
        {
            composer.sync(m);
        }
        // A message delivered, or one that could not be, is said once, even
        // when its job is no longer on the screen.
        if let Some(line) = self.engine.interventions.news().pop() {
            self.notice = Some(line);
        }
        let errors = pumped.errors;
        if pumped.graphs_changed {
            // Task graphs start jobs in sessions of their own.
            self.refresh_sessions(None);
        }
        errors
    }

    /// The mission of the selected card.
    pub(crate) fn selected_mission(&self) -> Option<&swarmux_domain::mission::Mission> {
        self.missions().get(self.missions_screen.selected)
    }

    pub(crate) fn composer_action(&mut self, action: ComposerAction) -> AppResult<()> {
        match action {
            ComposerAction::None => Ok(()),
            ComposerAction::Cancel => {
                self.missions_screen.composer = None;
                Ok(())
            }
            ComposerAction::Save => self.save_composer().map(drop),
            ComposerAction::Fix => {
                let slug = self.save_composer()?;
                self.fix_mission(&slug)?;
                if let (Some(service), Some(composer)) =
                    (&self.engine.missions, &mut self.missions_screen.composer)
                    && let Some(m) = service.get(&slug)
                {
                    *composer = Composer::edit(m);
                }
                Ok(())
            }
            ComposerAction::Run => {
                let slug = self.save_composer()?;
                self.launch_mission(&slug)?;
                self.missions_screen.composer = None;
                Ok(())
            }
        }
    }

    /// Saves what the composer holds: a new mission, a changed one (linted
    /// again), or only new answers. Returns its slug.
    fn save_composer(&mut self) -> AppResult<String> {
        let session = self.selected_session().cloned();
        let (Some(service), Some(composer)) = (
            &mut self.engine.missions,
            &mut self.missions_screen.composer,
        ) else {
            return Err(AppError::Mission("nothing to save".into()));
        };
        let current = composer
            .slug
            .as_deref()
            .and_then(|s| service.get(s))
            .cloned();
        let asked = current
            .as_ref()
            .map(|m| m.contract.clarifications.clone())
            .unwrap_or_default();
        let contract = composer.contract(&asked).map_err(AppError::Mission)?;
        let slug = match current {
            None => {
                let session = session.ok_or(AppError::SessionNotFound)?;
                let root = session.cwd.display().to_string();
                let slug =
                    service.create_in(contract, &root, Some(session.id), session.workspace)?;
                composer.slug = Some(slug.clone());
                slug
            }
            Some(m) => {
                let unchanged = Contract {
                    clarifications: Vec::new(),
                    ..contract.clone()
                } == Contract {
                    clarifications: Vec::new(),
                    ..m.contract.clone()
                };
                if unchanged {
                    for (i, q) in contract.clarifications.iter().enumerate() {
                        service.answer(&m.slug, i, &q.answer)?;
                    }
                } else {
                    service.edit(&m.slug, contract)?;
                }
                m.slug
            }
        };
        if let Some(m) = service.get(&slug) {
            composer.sync(m);
        }
        if let Some(at) = service.missions().iter().position(|m| m.slug == slug) {
            self.missions_screen.selected = at;
        }
        self.notice = Some(format!("{slug} saved and linted"));
        Ok(slug)
    }

    pub(crate) fn fix_mission(&mut self, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.engine.missions else {
            return Ok(());
        };
        let n = service.fix(slug)?;
        self.notice = Some(match n {
            0 => "nothing to protect".to_string(),
            1 => "protected 1 oracle".to_string(),
            n => format!("protected {n} oracles"),
        });
        Ok(())
    }

    /// Compiles the mission into a job and follows it; a mission that
    /// plans first is planned before anything is built.
    pub(crate) fn launch_mission(&mut self, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.engine.missions else {
            return Ok(());
        };
        match service.begin(slug, &mut self.engine.jobs)? {
            Begun::Job(job) => self.followed_mission(slug, job),
            Begun::Planning => {
                self.notice = Some(format!(
                    "{slug} is being planned: the plan comes back for your approval"
                ));
                Ok(())
            }
        }
    }

    /// The mission's build is out as `job`: follow it.
    pub(crate) fn followed_mission(&mut self, slug: &str, job: JobId) -> AppResult<()> {
        self.focus_state.note_human(job, self.now());
        self.dashboard.follow(job, None);
        self.refresh_sessions(None);
        self.notice = Some(format!(
            "{slug} is out as job #{job}; the gate decides when it is done"
        ));
        Ok(())
    }

    /// Makes the session of the selected inbox item the one prompts go to.
    pub(crate) fn select_inbox_session(&mut self) {
        if self.mission.pane == Pane::Wall {
            if let Some(tile) = self.wall().into_iter().nth(self.mission.wall_at)
                && let Some(at) = self.sessions.iter().position(|s| s.id == tile.session)
            {
                self.select_index(at);
            }
            return;
        }
        let Some(session) = self.mission.selected_item().and_then(|i| i.session) else {
            return;
        };
        if let Some(at) = self.sessions.iter().position(|s| s.id == session) {
            self.select_index(at);
        }
    }

    /// Opens the job of the selected inbox item in Focus and marks what it
    /// left in the inbox as read.
    pub(crate) fn open_selected(&mut self) -> AppResult<()> {
        if self.open_plan_item() {
            return Ok(());
        }
        let job = if self.mission.pane == Pane::Wall {
            let tile = self.wall().into_iter().nth(self.mission.wall_at);
            if tile.as_ref().is_some_and(|t| t.job.is_none()) {
                self.notice = Some("that session has not run anything yet".into());
            }
            tile.and_then(|t| t.job)
        } else {
            self.mission.selected_item().and_then(|i| i.job)
        };
        let Some(job) = job else {
            return Ok(());
        };
        self.view(job)?;
        self.mission.acknowledge_job(job);
        Ok(())
    }

    /// Focus is showing (not the classic panel grid).
    pub(crate) fn on_focus(&self) -> bool {
        self.screen == Screen::Dashboard && !self.dashboard.classic
    }

    /// The wall's tiles in their fixed order.
    pub(crate) fn wall(&self) -> Vec<Tile> {
        let held: Vec<_> = self
            .engine
            .guard
            .approvals()
            .iter()
            .filter_map(|a| a.request.job_id)
            .collect();
        let mut tiles = wall_tiles(
            &self.sessions,
            &self.mission.facts,
            &self.mission.gates,
            &held,
            self.mission.usage(),
            self.now(),
        );
        for tile in &mut tiles {
            let ws = self
                .sessions
                .iter()
                .find(|s| s.id == tile.session)
                .and_then(|s| crate::workspaces::find(&self.workspaces, s.workspace));
            tile.workspace = ws.map(crate::workspaces::label);
        }
        crate::workspaces::group_tiles(&mut tiles);
        tiles
    }

    /// The workspace of the session, when it works on one swarmux knows.
    pub(crate) fn workspace_of(&self, session: &Session) -> Option<&swarmux_domain::Workspace> {
        crate::workspaces::find(&self.workspaces, session.workspace)
    }

    pub(crate) fn move_wall(&mut self, delta: isize) {
        let last = self.sessions.len().saturating_sub(1);
        self.mission.wall_at = self.mission.wall_at.saturating_add_signed(delta).min(last);
        self.select_inbox_session();
        self.note_breakpoint(Breakpoint::TileSwitched);
    }

    /// The session of the job Focus shows.
    fn focus_session(&self) -> Option<&Session> {
        let job = self.current_job()?;
        let id = self
            .mission
            .facts
            .iter()
            .find(|f| f.job_id == job.job_id)
            .map(|f| f.session_id);
        self.sessions
            .iter()
            .find(|s| Some(s.id) == id || (id.is_none() && s.name == job.session_name))
    }

    /// `t`: the session, resumed in a new terminal tab.
    pub(crate) fn open_terminal(&mut self) -> AppResult<()> {
        let Some(terminal) = self.terminal.clone() else {
            self.notice = Some("no terminal launcher is set up".into());
            return Ok(());
        };
        let Some(session) = self.focus_session().cloned() else {
            self.notice = Some("no session to open".into());
            return Ok(());
        };
        terminal.open_resume(&session.cwd, &session.id.to_string())?;
        self.notice = Some(format!("opened {} in a new terminal tab", session.name));
        Ok(())
    }

    /// `d`: the uncommitted changes in place of the answer, and back.
    pub(crate) fn toggle_diff(&mut self) -> AppResult<()> {
        if self.focus_state.diff.take().is_some() {
            self.note_breakpoint(Breakpoint::DiffReviewed);
            return Ok(());
        }
        let Some(cwd) = self.focus_session().map(|s| s.cwd.clone()) else {
            return Ok(());
        };
        match self.engine.jobs.working_diff(&cwd)? {
            None => self.notice = Some("diffs need a git workspace".into()),
            Some(d) if d.trim().is_empty() => self.notice = Some("no uncommitted changes".into()),
            Some(d) => {
                self.focus_state.diff =
                    Some(d.lines().take(DIFF_LINES).map(str::to_string).collect());
            }
        }
        Ok(())
    }

    /// `[` and `]`: step through the job's checkpoints.
    pub(crate) fn scrub_checkpoint(&mut self, forward: bool) {
        let Some(job) = self.current_job().map(|j| j.job_id) else {
            return;
        };
        let count = Checkpoints::of(self, job).dots.len();
        if count == 0 {
            self.notice = Some("no checkpoints on this job yet".into());
            return;
        }
        let at = self.focus_state.checkpoint;
        self.focus_state.checkpoint = Some(match (at, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(i), true) => (i + 1).min(count - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
    }

    /// Moves the focus highlight to the next (or previous) boxed panel of
    /// the layout, then off.
    pub(crate) fn cycle_focus(&mut self, forward: bool) {
        let panels = self.look.layout_spec().panel_ids();
        let boxed: Vec<PanelId> = panels.into_iter().filter(|p| p.panel().boxed()).collect();
        if boxed.is_empty() {
            return;
        }
        let at = self.focus.and_then(|f| boxed.iter().position(|p| *p == f));
        self.focus = match (at, forward) {
            (None, true) => boxed.first().copied(),
            (None, false) => boxed.last().copied(),
            (Some(i), true) => boxed.get(i + 1).copied(),
            (Some(0), false) => None,
            (Some(i), false) => boxed.get(i - 1).copied(),
        };
    }

    /// The folder swarmux was started in.
    pub(crate) fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    pub(crate) fn show(&mut self, result: AppResult<()>) {
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
    }

    /// Approves, denies or always allows the oldest held tool call.
    fn answer_approval(&mut self, answer: Decision) -> AppResult<()> {
        let Some(target) = self.approval_target() else {
            if self.screen == Screen::Work {
                self.notice = Some("no call is held on this outcome".into());
            }
            return Ok(());
        };
        let id = target.request.request_id;
        let now = self.now();
        if let Some(item) = self.mission.alarms().find_request(id).cloned() {
            self.mission.alarms_mut().attend(&item, now);
        }
        if App::is_irreversible(&target) {
            // Irreversible calls are confirmed by naming their target, and
            // are never allowed for good.
            match answer {
                Decision::Yes => self.begin_confirm(&target),
                Decision::Always => {
                    self.notice = Some(format!(
                        "irreversible calls are never always-allowed: {} to confirm",
                        crate::hints::key_or_name(self, DECIDE_YES)
                    ));
                }
                _ => {
                    self.engine.guard.deny(id)?;
                    self.mission.alarms_mut().answered_request(id, false);
                }
            }
            return Ok(());
        }
        if let Some(job) = target.request.job_id {
            self.focus_state.note_human(job, now);
        }
        self.mission
            .alarms_mut()
            .answered_request(id, answer != Decision::No);
        match answer {
            Decision::Yes => self.engine.guard.approve(id),
            Decision::No => self.engine.guard.deny(id),
            _ => {
                let done = match self.engine.guard.always_allow(id) {
                    Ok(done) => done,
                    // A call that can't be always-allowed stays held: it can be
                    // allowed once.
                    Err(AppError::Guard(why)) => {
                        self.notice = Some(why);
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                };
                self.notice = Some(crate::inbox::always_notice("always allowing", &done));
                Ok(())
            }
        }
    }

    /// Collects finished verifications and label answers; whether any
    /// arrived.
    fn pump_evidence(&mut self) -> bool {
        let Some(evidence) = &self.evidence else {
            return false;
        };
        let verified = evidence.pump_verify();
        let labelled = evidence.pump_labels();
        let toast = verified.last().map(|slug| {
            let verdict = evidence.report(slug).map_or("no report", |r| r.verdict());
            format!("{slug}: {verdict}")
        });
        let any = !verified.is_empty() || !labelled.is_empty();
        if let Some(text) = toast {
            self.show_toast(text);
        }
        any
    }

    /// Shows `job` on the dashboard, rebuilding it from history if it is
    /// not from this run.
    pub(crate) fn view(&mut self, job: JobId) -> AppResult<()> {
        // A review job is shown as part of the job it audits.
        let mut job = self.engine.jobs.subject_of(job);
        let mut told = None;
        let replay = match self.engine.jobs.progress(job) {
            Some(_) => None,
            None => {
                let mut detail = self.history_service.detail(job)?;
                if let Some(of) = detail.summary.review_of {
                    job = of;
                    detail = self.history_service.detail(job)?;
                }
                told = Some((detail.summary.prompt, detail.result.map(|r| r.text)));
                Some(self.history_service.replay_with_review(job)?)
            }
        };
        self.dashboard.follow(job, replay);
        if let Some((prompt, answer)) = told {
            self.dashboard.replay_prompt = Some(prompt);
            self.dashboard.replay_answer = answer;
        }
        self.navigate(Screen::Dashboard);
        Ok(())
    }

    /// Rebuilds the run cards: every job joined with its mission and
    /// bundle, then the search and filter applied.
    pub(crate) fn refresh_ledger(&mut self) {
        let rows = match self.history_service.list("") {
            Ok(rows) => rows,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let rows: Vec<_> = rows.into_iter().filter(|r| r.review_of.is_none()).collect();
        let stored = match &self.evidence {
            Some(e) => e.list().unwrap_or_else(|err| {
                self.error = Some(err.to_string());
                Vec::new()
            }),
            None => Vec::new(),
        };
        let all = swarmux_application::build_cards(&rows, self.missions(), &stored);
        let now = self.now();
        let (since, away) = digest_window(self.away_since, now);
        let l = &mut self.ledger;
        l.stats = swarmux_domain::evidence::LedgerStats::of(&all);
        l.heat = heat_strip(&all, now, HEAT_DAYS);
        l.brief = brief(&all, since, now);
        l.away = away;
        let (rows, cards): (Vec<_>, Vec<_>) = rows
            .into_iter()
            .zip(all)
            .filter(|(r, c)| l.filter.admits(c) && (r.matches(&l.query) || c.matches(&l.query)))
            .unzip();
        l.set_rows(rows, cards);
        self.load_detail();
    }

    pub(crate) fn load_detail(&mut self) {
        self.ledger.detail = None;
        self.ledger.bundle = None;
        let Some(id) = self.ledger.selected_row().map(|r| r.job_id) else {
            return;
        };
        match self.history_service.detail(id) {
            Ok(d) => self.ledger.detail = Some(d),
            Err(e) => self.error = Some(e.to_string()),
        }
        let slug = self.ledger.selected_card().and_then(|c| c.mission.clone());
        if let (Some(slug), Some(evidence)) = (slug, &self.evidence) {
            match evidence.get(&slug) {
                Ok(stored) => self.ledger.bundle = stored,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    pub(crate) fn save_form(&mut self) -> AppResult<()> {
        let Some(form) = &self.prompts.form else {
            return Ok(());
        };
        let (name, body, tags) = (form.name.text.clone(), form.body.text(), form.tag_list());
        let id = match form.id {
            Some(id) => {
                self.prompt_service.update(id, &name, &body, tags)?;
                id
            }
            None => self.prompt_service.create(&name, &body, tags)?,
        };
        self.prompts.form = None;
        self.refresh_prompts(Some(id));
        Ok(())
    }

    /// Asks for the selected template's variables, then dispatches it (or
    /// routes it, with `route`).
    pub(crate) fn start_template_dispatch(&mut self, route: bool) -> AppResult<()> {
        let Some(prompt) = self.prompts.selected_prompt() else {
            self.error = Some(format!(
                "no prompt selected: press {} to write one",
                crate::hints::key_or_name(self, screens::prompts::NEW)
            ));
            return Ok(());
        };
        let Some(session) = self.selected_session() else {
            self.error = Some(self.need_session());
            return Ok(());
        };
        self.prompts.pending = Some(PendingDispatch {
            prompt_id: prompt.id,
            session_id: session.id,
            remaining: PromptService::user_variables(&prompt.body),
            values: Default::default(),
            route,
        });
        self.ask_or_dispatch()
    }

    fn answer_variable(&mut self, value: String) -> AppResult<()> {
        if let Some(pending) = &mut self.prompts.pending {
            pending.answer(value);
        }
        self.ask_or_dispatch()
    }

    /// Asks for the next missing variable, or dispatches once all are known.
    fn ask_or_dispatch(&mut self) -> AppResult<()> {
        let Some(pending) = self.prompts.pending.take() else {
            return Ok(());
        };
        if pending.asking().is_some() {
            self.prompts.pending = Some(pending);
            self.mode = Mode::Input(InputPurpose::Variable, Input::default());
            return Ok(());
        }
        if pending.route {
            self.navigate(Screen::Dashboard);
            return self.route_template(pending);
        }
        let rendered =
            self.prompt_service
                .render(pending.prompt_id, pending.session_id, &pending.values)?;
        let job = self
            .engine
            .jobs
            .dispatch_template(pending.session_id, &rendered)?;
        self.focus_state.note_human(job, self.now());
        self.dashboard.follow(job, None);
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == pending.session_id)
            .cloned();
        self.refresh_sessions(session.as_ref());
        self.navigate(Screen::Dashboard);
        Ok(())
    }

    pub(crate) fn refresh_prompts(&mut self, select: Option<PromptId>) {
        match self.prompt_service.list() {
            Ok(list) => self.prompts.set_list(list, select),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Lean on or off for the selected session's next jobs.
    pub(crate) fn toggle_lean(&mut self) -> AppResult<()> {
        let Some(session) = self.selected_session().cloned() else {
            return Ok(());
        };
        let lean = self.session_service.toggle_lean(session.id)?;
        self.notice = Some(if lean {
            format!(
                "{} is lean: its jobs skip your user-level Claude Code settings",
                session.name
            )
        } else {
            format!("{} uses all your Claude Code settings again", session.name)
        });
        self.refresh_sessions(Some(&session));
        Ok(())
    }

    fn create_session(&mut self, name: &str) -> AppResult<()> {
        let session = self.session_service.create(name, self.cwd.clone())?;
        self.refresh_sessions(Some(&session));
        Ok(())
    }

    /// A new session on the workspace called `name`, named after it.
    fn create_workspace_session(&mut self, name: &str) -> AppResult<()> {
        let service = self
            .workspace_service
            .as_ref()
            .ok_or_else(|| AppError::WorkspaceNotFound(name.trim().to_string()))?;
        let workspace = service.find(name)?;
        let session = self
            .session_service
            .create_in_workspace(&workspace.name, &workspace)?;
        self.refresh_sessions(Some(&session));
        Ok(())
    }

    fn dispatch(&mut self, prompt: &str) -> AppResult<()> {
        let session = self
            .selected_session()
            .cloned()
            .ok_or(AppError::SessionNotFound)?;
        let job = self.engine.jobs.dispatch(session.id, prompt)?;
        self.focus_state.note_human(job, self.now());
        self.dashboard.follow(job, None);
        self.refresh_sessions(Some(&session));
        self.note_breakpoint(Breakpoint::PromptSent);
        Ok(())
    }

    /// Reloads the session list. `select` becomes the selected session;
    /// otherwise the selected session stays selected wherever it moved.
    pub(crate) fn refresh_sessions(&mut self, select: Option<&Session>) {
        match self.session_service.list() {
            Ok(list) => self.sessions = list,
            Err(e) => self.error = Some(e.to_string()),
        }
        if let Some(service) = &self.workspace_service {
            match service.list() {
                Ok(list) => self.workspaces = list,
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        crate::workspaces::group_sessions(&mut self.sessions);
        if let Some(target) = select {
            self.selected_id = Some(target.id);
        }
        let found = self
            .selected_id
            .and_then(|id| self.sessions.iter().position(|s| s.id == id));
        let i = found.unwrap_or(self.selected);
        self.select_index(i.min(self.sessions.len().saturating_sub(1)));
    }

    pub(crate) fn select_index(&mut self, i: usize) {
        self.selected = i;
        self.selected_id = self.sessions.get(i).map(|s| s.id);
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::rc::Rc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use swarmux_application::ports::PromptRepository;
    use swarmux_application::testing::{
        FakeAgentCatalog, FakeDecisionRepo, FakeGit, FakeHookInbox, FakeJobGates, FakeLinter,
        FakeMergeQueue, FakeMonitor, FakePlanner, FakeRouter, FakeRuleStore, FakeRunner,
        FakeTelemetry, FakeTerminal, FakeUsageProbe, FakeWorkspace, FixedClock, InMemoryMissions,
        InMemoryPlans, InMemoryStore, InMemoryWorktrees,
    };
    use swarmux_application::{
        Governor, GovernorSettings, GuardService, HistoryService, JobService, MissionService,
        PromptService, ReviewSettings, ReviewWhen, RoutingService, ScopeClaims, SessionService,
        SharedPolicy, TelemetryService, TelemetrySettings, WorktreeService,
    };
    use swarmux_domain::{GuardPolicy, PromptDraft};

    use super::App;

    pub(crate) struct TestApp {
        pub app: App,
        pub store: Rc<InMemoryStore>,
        pub runner: Rc<FakeRunner>,
        pub clock: Rc<FixedClock>,
        pub monitor: Rc<FakeMonitor>,
        pub inbox: Rc<FakeHookInbox>,
        pub decisions: Rc<FakeDecisionRepo>,
        pub rules: Rc<FakeRuleStore>,
        pub router: Rc<FakeRouter>,
        pub catalog: Rc<FakeAgentCatalog>,
        /// What Mission control's telemetry strip reads.
        pub feed: Rc<FakeTelemetry>,
        /// Records what `t` on Focus opens.
        pub terminal: Rc<FakeTerminal>,
        /// Where the Missions screen keeps its missions.
        pub missions: Rc<InMemoryMissions>,
        /// Answers mission lints when told to.
        pub linter: Rc<FakeLinter>,
        /// The gates mission jobs were given.
        pub job_gates: Rc<FakeJobGates>,
        /// Answers planning runs of plan-first missions when told to.
        pub planner: Rc<FakePlanner>,
        /// The usage windows the governor reads.
        pub usage: Rc<FakeUsageProbe>,
        /// Set by [`test_app_with_graph`]: the merge queue task graphs use.
        pub merge: Option<Rc<FakeMergeQueue>>,
    }

    /// An app wired to in-memory fakes.
    pub(crate) fn test_app() -> TestApp {
        test_app_with_store(Rc::new(InMemoryStore::default()))
    }

    /// An app whose `/work` folder is a git repository with worktrees, so
    /// approved plans can run as task graphs.
    pub(crate) fn test_app_with_graph() -> TestApp {
        build(Rc::new(InMemoryStore::default()), true)
    }

    /// An app over an existing store, like a restart.
    pub(crate) fn test_app_with_store(store: Rc<InMemoryStore>) -> TestApp {
        build(store, false)
    }

    fn build(store: Rc<InMemoryStore>, graph: bool) -> TestApp {
        let runner = Rc::new(FakeRunner::default());
        let clock = Rc::new(FixedClock::new(1_000));
        let monitor = Rc::new(FakeMonitor::default());
        let workspace = Rc::new(FakeWorkspace::with_diff("+diff"));
        let sessions = SessionService::new(store.clone(), clock.clone());
        let catalog = Rc::new(FakeAgentCatalog::default());
        let usage = Rc::new(FakeUsageProbe::default());
        let governor = Governor::new(&GovernorSettings::default()).with_probe(usage.clone());
        let jobs = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone())
            .with_governor(governor)
            .with_monitor(monitor.clone())
            .with_review(
                workspace.clone(),
                &ReviewSettings {
                    // The fixtures exercise the reviewer on small diffs.
                    when: ReviewWhen::Always,
                    ..ReviewSettings::default()
                },
            )
            .with_agent_catalog(catalog.clone());
        let prompts = PromptService::new(store.clone(), store.clone(), workspace, clock.clone());
        let history = HistoryService::new(store.clone());
        let inbox = Rc::new(FakeHookInbox::default());
        let decisions = Rc::new(FakeDecisionRepo::default());
        let rules = Rc::new(FakeRuleStore::default());
        let guard = GuardService::new(
            SharedPolicy::new(GuardPolicy::default()).with_now(clock.reader()),
            inbox.clone(),
            decisions.clone(),
            clock.clone(),
        )
        .with_rule_store(rules.clone());
        let router = Rc::new(FakeRouter::default());
        let routing = RoutingService::new(
            store.clone(),
            store.clone(),
            decisions.clone(),
            clock.clone(),
        )
        .with_router(router.clone());
        let feed = Rc::new(FakeTelemetry::default());
        let terminal = Rc::new(FakeTerminal::default());
        let service = TelemetryService::new(feed.clone(), store.clone(), clock.clone());
        let missions = Rc::new(InMemoryMissions::default());
        let linter = Rc::new(FakeLinter::default());
        let job_gates = Rc::new(FakeJobGates::default());
        let planner = Rc::new(FakePlanner::default());
        let mut mission_service = MissionService::new(missions.clone(), clock.clone())
            .unwrap()
            .with_linter(linter.clone())
            .with_gates(job_gates.clone())
            .with_planner(planner.clone(), Rc::new(InMemoryPlans::default()))
            .unwrap();
        let mut jobs = jobs;
        let mut merge = None;
        if graph {
            let worktrees = Rc::new(WorktreeService::new(
                Rc::new(FakeGit::with_repo("/work")),
                Rc::new(InMemoryWorktrees::default()),
                store.clone(),
                clock.clone(),
                "/wt".into(),
            ));
            let queue = Rc::new(FakeMergeQueue::default());
            jobs = jobs.with_worktrees(worktrees.clone());
            mission_service =
                mission_service.with_graph(queue.clone(), worktrees, ScopeClaims::new());
            merge = Some(queue);
        }
        let mut app = App::new(
            sessions,
            jobs,
            prompts,
            history,
            guard,
            routing,
            "/work".into(),
        )
        .with_missions(mission_service)
        .with_clock(clock.clone())
        .with_terminal(terminal.clone())
        .with_telemetry_feed(service, &TelemetrySettings::default());
        // The panel-grid tests predate Focus; they ask for the classic view.
        // Focus tests turn it off with `focus_view`.
        app.dashboard.classic = true;
        TestApp {
            app,
            store,
            runner,
            clock,
            monitor,
            inbox,
            decisions,
            rules,
            router,
            catalog,
            feed,
            terminal,
            missions,
            linter,
            job_gates,
            planner,
            usage,
            merge,
        }
    }

    impl TestApp {
        pub(crate) fn keys(&mut self, codes: impl IntoIterator<Item = KeyCode>) {
            for code in codes {
                self.app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
        }

        /// Creates a session through the sessions screen, then goes back to the dashboard.
        pub(crate) fn with_session(&mut self, name: &str) {
            self.keys([KeyCode::Char('6'), KeyCode::Char('n')]);
            self.keys(name.chars().map(KeyCode::Char));
            self.keys([KeyCode::Enter, KeyCode::Char('0')]);
        }

        /// Sends a prompt to the selected session from the dashboard.
        pub(crate) fn send(&mut self, prompt: &str) {
            self.keys([KeyCode::Char('0'), KeyCode::Char('p')]);
            self.keys(prompt.chars().map(KeyCode::Char));
            self.keys([KeyCode::Enter]);
        }

        /// Seals a bundle for every mission delivered from now on, on a
        /// ledger in memory the returned kit shares with the app.
        pub(crate) fn with_evidence(
            &mut self,
        ) -> swarmux_application::evidence::testing::EvidenceKit {
            let kit = swarmux_application::evidence::testing::EvidenceKit::new(
                self.store.clone(),
                self.clock.clone(),
            );
            self.app.attach_evidence(kit.service.clone(), None);
            kit
        }

        /// Saves a prompt straight to the store and reloads the list.
        pub(crate) fn add_prompt(&mut self, name: &str, body: &str, tags: &[&str]) {
            let tags = tags.iter().map(|t| t.to_string()).collect();
            let draft = PromptDraft::new(name, body, tags).unwrap();
            let id = PromptRepository::insert(self.store.as_ref(), &draft, 1).unwrap();
            self.app.refresh_prompts(Some(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::test_app;
    use super::*;
    use swarmux_application::ports::RunnerMsg;
    use swarmux_domain::{Actor, AgentEvent, JobStatus};

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn with_session(app: &mut App) {
        press(app, KeyCode::Char('6'));
        press(app, KeyCode::Char('n'));
        type_str(app, "demo");
        press(app, KeyCode::Enter);
        press(app, KeyCode::Char('0'));
    }

    #[test]
    fn number_keys_switch_screens() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('4'));
        assert_eq!(t.app.screen, Screen::Queue);
        press(&mut t.app, KeyCode::Char('2'));
        assert_eq!(t.app.screen, Screen::Missions);
        press(&mut t.app, KeyCode::Char('0'));
        assert_eq!(t.app.screen, Screen::Dashboard);
        press(&mut t.app, KeyCode::Char('='));
        assert_eq!(t.app.screen, Screen::Dashboard, "no such screen");
        press(&mut t.app, KeyCode::Char('8'));
        assert_eq!(t.app.screen, Screen::Knowledge);
        press(&mut t.app, KeyCode::Char('1'));
        assert_eq!(t.app.screen, Screen::Mission);
        assert_eq!(Screen::default(), Screen::Mission, "the home screen");
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('q'));
        assert!(t.app.quit);
        let mut t = test_app();
        t.app
            .on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(t.app.quit);
    }

    #[test]
    fn create_session_via_keys() {
        let mut t = test_app();
        with_session(&mut t.app);
        let names: Vec<_> = t.app.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["demo"]);
        assert_eq!(t.app.sessions[0].cwd, PathBuf::from("/work"));
        assert_eq!(t.app.mode, Mode::Normal);
    }

    #[test]
    fn l_toggles_lean_and_jobs_follow() {
        let mut t = test_app();
        with_session(&mut t.app);
        press(&mut t.app, KeyCode::Char('6'));
        press(&mut t.app, KeyCode::Char('l'));
        assert!(t.app.sessions[0].lean);
        assert!(t.app.notice.as_deref().unwrap().contains("is lean"));
        let out = crate::ui::render_to_string(&t.app, 100, 8);
        assert!(out.contains("lean"), "{out}");
        press(&mut t.app, KeyCode::Char('0'));
        press(&mut t.app, KeyCode::Char('p'));
        type_str(&mut t.app, "hi");
        press(&mut t.app, KeyCode::Enter);
        assert!(t.runner.request(0).lean);
        press(&mut t.app, KeyCode::Char('6'));
        press(&mut t.app, KeyCode::Char('l'));
        assert!(!t.app.sessions[0].lean);
    }

    fn session_named(t: &super::tests_support::TestApp, name: &str) -> swarmux_domain::SessionId {
        t.app.sessions.iter().find(|s| s.name == name).unwrap().id
    }

    #[test]
    fn new_lean_session_takes_the_next_prompt() {
        let mut t = test_app();
        t.with_session("test session");
        t.clock.advance(1_000);
        t.with_session("demo-shop");
        t.keys([KeyCode::Char('6'), KeyCode::Char('l'), KeyCode::Char('0')]);
        assert!(t.app.selected_session().unwrap().lean);
        t.send("hi");
        assert_eq!(
            t.runner.request(0).session_id,
            session_named(&t, "demo-shop")
        );
        assert!(t.runner.request(0).lean);
    }

    #[test]
    fn selection_follows_the_session_when_the_list_re_sorts() {
        let mut t = test_app();
        t.with_session("a");
        t.clock.advance(1_000);
        t.with_session("b");
        let b = session_named(&t, "b");
        t.clock.advance(1_000);
        t.send("one");
        // Another session's job finishing re-sorts the list under the cursor.
        let a = session_named(&t, "a");
        let id = t.app.engine.jobs.dispatch(a, "elsewhere").unwrap();
        assert_eq!(t.runner.request(1).session_id, a);
        t.clock.advance(1_000);
        for i in 0..2 {
            t.runner.emit(i, RunnerMsg::SpawnFailed("x".into()));
        }
        t.app.update();
        assert_eq!(t.app.sessions[0].id, a, "a was used last");
        assert_eq!(t.app.selected_session().unwrap().id, b);
        t.send("two");
        assert_eq!(t.runner.request(2).session_id, b);
        assert!(t.app.engine.jobs.progress(id).is_some());
    }

    #[test]
    fn q_inside_input_types_instead_of_quitting() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('6'));
        press(&mut t.app, KeyCode::Char('n'));
        press(&mut t.app, KeyCode::Char('q'));
        assert!(!t.app.quit);
        assert!(matches!(&t.app.mode, Mode::Input(_, i) if i.text == "q"));
    }

    #[test]
    fn prompt_without_session_shows_hint() {
        let mut t = test_app();
        t.app.screen = Screen::Dashboard;
        press(&mut t.app, KeyCode::Char('p'));
        assert_eq!(t.app.mode, Mode::Normal);
        assert!(t.app.error.as_deref().unwrap().contains("press 6 then n"));
    }

    #[test]
    fn prompt_dispatches_and_progress_updates() {
        let mut t = test_app();
        with_session(&mut t.app);
        press(&mut t.app, KeyCode::Char('p'));
        type_str(&mut t.app, "hello");
        press(&mut t.app, KeyCode::Enter);
        assert_eq!(t.runner.started(), 1);
        assert_eq!(t.runner.request(0).prompt, "hello");

        t.clock.advance(2_000);
        t.runner.emit(
            0,
            RunnerMsg::Event(AgentEvent::Text {
                text: "hi there".into(),
                actor: Actor::Lead,
            }),
        );
        t.runner
            .emit(0, RunnerMsg::SpawnFailed("not really".into()));
        t.app.update();
        let job = t.app.current_job().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.log.iter().any(|l| l.contains("hi there")));
        assert_eq!(t.app.now_tick(), 20);
    }

    #[test]
    fn second_prompt_to_busy_session_is_queued() {
        let mut t = test_app();
        with_session(&mut t.app);
        for p in ["one", "two"] {
            press(&mut t.app, KeyCode::Char('p'));
            type_str(&mut t.app, p);
            press(&mut t.app, KeyCode::Enter);
        }
        assert_eq!(t.app.error, None);
        assert_eq!(t.runner.started(), 1);
        let job = t.app.current_job().unwrap();
        assert_eq!(
            job.status,
            JobStatus::Queued,
            "dashboard follows the new job"
        );
        assert_eq!(t.app.stats().queued, 1);
    }

    fn ctrl_s(app: &mut App) {
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    }

    /// Types `body` into the editor, turning `\n` into Enter.
    fn type_body(app: &mut App, body: &str) {
        for (i, line) in body.split('\n').enumerate() {
            if i > 0 {
                press(app, KeyCode::Enter);
            }
            type_str(app, line);
        }
    }

    fn new_prompt(app: &mut App, name: &str, tags: &str, body: &str) {
        press(app, KeyCode::Char('3'));
        press(app, KeyCode::Char('n'));
        type_str(app, name);
        press(app, KeyCode::Tab);
        type_str(app, tags);
        press(app, KeyCode::Tab);
        type_body(app, body);
        ctrl_s(app);
    }

    #[test]
    fn create_prompt_via_form() {
        let mut t = test_app();
        new_prompt(
            &mut t.app,
            "q1 review",
            " rust, ai ,",
            "check {{file}}\nthen 5 more",
        );
        assert!(t.app.prompts.form.is_none());
        assert_eq!(t.app.error, None);
        assert!(!t.app.quit);
        assert_eq!(t.app.screen, Screen::Prompts);
        let p = t.app.prompts.selected_prompt().unwrap();
        assert_eq!(p.name, "q1 review");
        assert_eq!(p.tags, ["rust", "ai"]);
        assert_eq!(p.body, "check {{file}}\nthen 5 more");
    }

    #[test]
    fn edit_prompt_keeps_id_and_updates_body() {
        let mut t = test_app();
        new_prompt(&mut t.app, "a", "", "one");
        let id = t.app.prompts.selected_prompt().unwrap().id;
        press(&mut t.app, KeyCode::Char('e'));
        press(&mut t.app, KeyCode::End);
        type_str(&mut t.app, " two");
        ctrl_s(&mut t.app);
        let p = t.app.prompts.selected_prompt().unwrap();
        assert_eq!((p.id, p.body.as_str()), (id, "one two"));
    }

    #[test]
    fn invalid_form_stays_open_with_error() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('3'));
        press(&mut t.app, KeyCode::Char('n'));
        ctrl_s(&mut t.app);
        assert!(t.app.prompts.form.is_some());
        assert!(t.app.error.as_deref().unwrap().contains("name"));
        press(&mut t.app, KeyCode::Esc);
        assert!(t.app.prompts.form.is_none());
        assert!(t.app.prompts.list.is_empty());
    }

    #[test]
    fn delete_needs_x_twice() {
        let mut t = test_app();
        new_prompt(&mut t.app, "a", "", "body");
        press(&mut t.app, KeyCode::Char('x'));
        assert!(t.app.prompts.confirm_delete);
        press(&mut t.app, KeyCode::Down);
        assert!(!t.app.prompts.confirm_delete);
        assert_eq!(t.app.prompts.list.len(), 1);
        press(&mut t.app, KeyCode::Char('x'));
        press(&mut t.app, KeyCode::Char('x'));
        assert!(t.app.prompts.list.is_empty());
    }

    #[test]
    fn dispatch_asks_each_user_variable() {
        let mut t = test_app();
        with_session(&mut t.app);
        new_prompt(
            &mut t.app,
            "fix",
            "",
            "fix {{file}} on {{date}}: {{goal}} {{file}}",
        );
        press(&mut t.app, KeyCode::Char('d'));
        assert!(matches!(t.app.mode, Mode::Input(InputPurpose::Variable, _)));
        let status = crate::ui::render_to_string(&t.app, 100, 8);
        assert!(status.contains("var file:"), "{status}");

        type_str(&mut t.app, "a.rs");
        press(&mut t.app, KeyCode::Enter);
        assert_eq!(t.runner.started(), 0);
        type_str(&mut t.app, "speed 1");
        press(&mut t.app, KeyCode::Enter);

        assert_eq!(t.runner.started(), 1);
        assert_eq!(
            t.runner.request(0).prompt,
            "fix a.rs on 1970-01-01: speed 1 a.rs"
        );
        let job = t.app.current_job().unwrap().job_id;
        assert_eq!(
            t.store.prompt_of(job),
            t.app.prompts.selected_prompt().map(|p| p.id)
        );
        assert_eq!(t.app.screen, Screen::Dashboard);
        assert_eq!(t.app.mode, Mode::Normal);
    }

    #[test]
    fn dispatch_without_user_variables_is_immediate() {
        let mut t = test_app();
        with_session(&mut t.app);
        new_prompt(&mut t.app, "review", "", "review:\n{{git_diff}}");
        press(&mut t.app, KeyCode::Char('d'));
        assert_eq!(t.runner.request(0).prompt, "review:\n+diff");
        assert_eq!(t.app.screen, Screen::Dashboard);
    }

    #[test]
    fn esc_while_asking_cancels_dispatch() {
        let mut t = test_app();
        with_session(&mut t.app);
        new_prompt(&mut t.app, "p", "", "{{x}}");
        press(&mut t.app, KeyCode::Char('d'));
        press(&mut t.app, KeyCode::Esc);
        assert!(t.app.prompts.pending.is_none());
        assert_eq!(t.runner.started(), 0);
        assert_eq!(t.app.screen, Screen::Prompts);
    }

    #[test]
    fn dispatch_without_session_shows_hint() {
        let mut t = test_app();
        new_prompt(&mut t.app, "p", "", "body");
        press(&mut t.app, KeyCode::Char('d'));
        assert!(t.app.error.as_deref().unwrap().contains("press 6 then n"));
        assert_eq!(t.runner.started(), 0);
    }

    #[test]
    fn up_down_on_prompts_moves_prompt_selection() {
        let mut t = test_app();
        new_prompt(&mut t.app, "a", "", "x");
        t.clock.advance(1);
        new_prompt(&mut t.app, "b", "", "x");
        assert_eq!(t.app.prompts.selected_prompt().unwrap().name, "b");
        press(&mut t.app, KeyCode::Down);
        assert_eq!(t.app.prompts.selected_prompt().unwrap().name, "a");
    }
}

#[cfg(test)]
mod notify_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crossterm::event::Event;
    use swarmux_application::{AppResult, Notification, Notifier, NotifyService, NotifySettings};
    use swarmux_domain::{AttentionId, AttentionItem, AttentionKind, Likelihood, Priority};

    use super::tests_support::test_app;

    struct Log(Rc<RefCell<Vec<Notification>>>);

    impl Notifier for Log {
        fn notify(&self, n: &Notification) -> AppResult<()> {
            self.0.borrow_mut().push(n.clone());
            Ok(())
        }
    }

    fn item(key: &str, priority: Priority) -> AttentionItem {
        AttentionItem {
            id: AttentionId::new(key),
            kind: AttentionKind::JobFailed,
            priority,
            title: key.into(),
            detail: String::new(),
            session: None,
            job: None,
            request: None,
            since: 0,
            deadline: None,
            likelihood: Likelihood::Certain,
            risk: None,
            signature: None,
            batch: 1,
            suggest: None,
        }
    }

    #[test]
    fn a_background_window_notifies_p2_once_and_never_p3() {
        let mut t = test_app();
        let seen = Rc::new(RefCell::new(Vec::new()));
        t.app = t.app.with_notifier(NotifyService::new(
            Box::new(Log(seen.clone())),
            NotifySettings::default(),
            || 0,
        ));
        t.app.mission.items = vec![item("a", Priority::P2), item("b", Priority::P3)];
        // The user is here: nothing yet.
        t.app.on_event(Event::FocusGained);
        t.app.pump_notifications();
        assert!(seen.borrow().is_empty());
        // The window goes to the background.
        t.app.on_event(Event::FocusLost);
        t.app.pump_notifications();
        t.app.pump_notifications();
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(seen.borrow()[0].id.as_str(), "a");
    }
}
