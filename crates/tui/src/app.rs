//! UI state and key handling. All work goes through application use cases.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use std::rc::Rc;

use swarmux_application::ports::Clock;
use swarmux_application::{
    AppError, AppResult, CheckpointService, EvidenceService, FocusTracker, GateService,
    GuardService, HistoryService, HookBus, JobService, MissionService, NotifyService,
    PaletteSource, PendingApproval, PromptService, QueueEntry, QueueStats, RoutingService,
    SessionService, SessionTerminal, TelemetryService, TelemetrySettings, UiSettings,
};
use swarmux_domain::alarm::{Breakpoint, LoadMeter};
use swarmux_domain::evidence::{brief, heat_strip};
use swarmux_domain::mission::Contract;
use swarmux_domain::{AttentionKind, JobId, JobProgress, Millis, PromptId, Session, SessionId};

use crate::dashboard::DashboardState;
use crate::focus::{Checkpoints, FocusState};
use crate::inbox::Confirm;
use crate::ledger::{HEAT_DAYS, LedgerAction, LedgerScreen};
use crate::mission::{Cadence, MissionState, Pane, Tile, wall_tiles};
use crate::missions::{Composer, ComposerAction, MissionsScreen};
use crate::prompts::{FormAction, PendingDispatch, PromptForm, PromptsScreen};
use crate::queue::{QueueAction, QueueScreen};
use crate::routing::RoutePayload;
use crate::telemetry::Telemetry;
use crate::theme::{ColorDepth, Theme};
use crate::ui::anim::Anim;
use crate::ui::grid::PanelId;
use crate::ui::input::{Input, InputAction};
use crate::ui::look::{IconHints, Look};
use crate::ui::splash::SplashInfo;

/// The top-level screens, in tab order: `1` to `6`, then Focus on `0`
/// (and `⏎` from any card or row).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    /// Mission control: the home screen.
    #[default]
    Mission,
    /// Outcomes in flight: mission cards and the composer.
    Missions,
    Prompts,
    Queue,
    /// What happened, what it cost and whether it can be proven.
    Ledger,
    Sessions,
    /// The followed job in detail (the dashboard's panels).
    Dashboard,
}

impl Screen {
    pub const ALL: [Screen; 7] = [
        Screen::Mission,
        Screen::Missions,
        Screen::Prompts,
        Screen::Queue,
        Screen::Ledger,
        Screen::Sessions,
        Screen::Dashboard,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Mission => "mission",
            Screen::Missions => "missions",
            Screen::Dashboard => "focus",
            Screen::Prompts => "prompts",
            Screen::Queue => "queue",
            Screen::Ledger => "ledger",
            Screen::Sessions => "sessions",
        }
    }

    /// The number key that shows it.
    pub fn key(self) -> char {
        match self {
            Screen::Mission => '1',
            Screen::Missions => '2',
            Screen::Prompts => '3',
            Screen::Queue => '4',
            Screen::Ledger => '5',
            Screen::Sessions => '6',
            Screen::Dashboard => '0',
        }
    }

    pub fn by_key(c: char) -> Option<Screen> {
        Screen::ALL.into_iter().find(|s| s.key() == c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPurpose {
    NewSession,
    Prompt,
    /// A prompt swarmux sends to the session it picks.
    RoutedPrompt,
    /// A value for the template variable `prompts.pending` is asking for.
    Variable,
    /// The ledger search; applied as it is typed.
    Search,
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
    /// Frames drawn so far; drives the spinners.
    pub frame: u64,
    pub quit: bool,
    pub mode: Mode,
    pub sessions: Vec<Session>,
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
    /// Writes, lints and follows missions.
    missions: Option<MissionService>,
    /// Bundles, their verification and outcome labels.
    evidence: Option<Rc<EvidenceService>>,
    /// When the user was last here: what happened since is the digest.
    away_since: Option<Millis>,
    clock: Option<Rc<dyn Clock>>,
    pub(crate) session_service: SessionService,
    pub(crate) jobs: JobService,
    pub(crate) prompt_service: PromptService,
    history_service: HistoryService,
    pub guard: GuardService,
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
    /// Lifecycle events reported by the agents' hooks.
    hooks: Option<HookBus>,
    /// The done-gate's verdicts.
    gate: Option<GateService>,
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
    notifier: Option<NotifyService>,
    /// Whether the terminal window has the user's attention.
    window: FocusTracker,
}

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
/// How long a toast stays up, in milliseconds.
const TOAST_MS: u64 = 2500;
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
        let mut app = App {
            screen: Screen::default(),
            frame: 0,
            quit: false,
            mode: Mode::Normal,
            sessions: Vec::new(),
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
            missions: None,
            evidence: None,
            away_since: None,
            clock: None,
            session_service,
            jobs,
            prompt_service,
            history_service,
            guard,
            routing,
            routes: HashMap::new(),
            cwd,
            look: Look::default(),
            anim: RefCell::new(Anim::default()),
            telemetry: Telemetry::default(),
            hooks: None,
            gate: None,
            terminal: None,
            checkpoints: None,
            focus_checkpoints: None,
            load: LoadMeter::default(),
            last_input: 0,
            idle_noted: false,
            notifier: None,
            window: FocusTracker::default(),
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
    pub fn with_hook_bus(mut self, bus: HookBus) -> App {
        self.hooks = Some(bus);
        self
    }

    /// Records the done-gate's verdicts and shows them on the wall.
    pub fn with_gate(mut self, gate: GateService) -> App {
        self.set_gate(gate);
        self
    }

    pub(crate) fn set_gate(&mut self, gate: GateService) {
        self.gate = Some(gate);
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
        self.missions = Some(missions);
        self
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

    /// Also lets the missions seal a bundle when they are delivered.
    pub(crate) fn attach_evidence(
        &mut self,
        evidence: Rc<EvidenceService>,
        away_since: Option<Millis>,
    ) {
        if let Some(missions) = &mut self.missions {
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
        self.missions.as_ref().map_or(&[], MissionService::missions)
    }

    /// The mission service, for tests that set missions up directly.
    #[cfg(test)]
    pub(crate) fn mission_service(&mut self) -> &mut MissionService {
        self.missions.as_mut().expect("the test app has missions")
    }

    /// A lint is out for the mission.
    pub fn mission_linting(&self, slug: &str) -> bool {
        self.missions.as_ref().is_some_and(|m| m.linting(slug))
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
        let approvals = self.guard.approvals();
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

    pub(crate) fn refresh_mission(&mut self) {
        let now = self.now();
        let cwd = self.selected_session().map(|s| s.cwd.clone());
        let hook_changes = self
            .hooks
            .as_ref()
            .map(HookBus::changes)
            .unwrap_or_default();
        self.mission.refresh(
            (&self.guard, &self.jobs, self.gate.as_ref()),
            &hook_changes,
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

    fn show_toast(&mut self, text: String) {
        self.toast = Some((text, self.frame + self.look.frames(TOAST_MS)));
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quit {
            self.update();
            terminal.draw(|frame| crate::ui::draw(frame, &self))?;
            self.frame = self.frame.wrapping_add(1);
            if event::poll(self.look.frame_time())? {
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

    /// Applies pending job updates, starts queued jobs and handles guard requests.
    pub fn update(&mut self) {
        let report = self.jobs.pump();
        let mut errors = report.errors;
        for (job, trip) in &report.loop_trips {
            errors.extend(self.guard.record_loop(*job, trip).err());
        }
        let guard = self.guard.pump();
        errors.extend(guard.errors);
        for job in guard.decisions.iter().filter_map(|d| d.job_id) {
            self.jobs.observe_decision(job);
        }
        if let Some(hooks) = &mut self.hooks {
            errors.extend(hooks.pump());
        }
        if let Some(gate) = &mut self.gate {
            let (_, gate_errors) = gate.pump();
            errors.extend(gate_errors);
        }
        errors.extend(self.pump_routes());
        if let Some(e) = errors.into_iter().last() {
            self.error = Some(e.to_string());
        }
        if report.changed || !report.new_sessions.is_empty() {
            // Jobs touch their sessions, which re-sorts the list.
            self.refresh_sessions(None);
        }
        let evidence_changed = self.pump_evidence();
        if (report.changed || evidence_changed) && self.screen == Screen::Ledger {
            self.refresh_ledger();
        }
        self.pump_palette();
        self.refresh_mission();
        self.tick_inbox();
        self.pump_notifications();
        if let Some(e) = self.pump_missions().into_iter().last() {
            self.error = Some(e.to_string());
        }
        self.refresh_checkpoints();
        let on_focus = (self.screen == Screen::Dashboard && !self.dashboard.classic)
            .then(|| self.current_job().map(|j| (j.job_id, j.log.len())))
            .flatten();
        self.focus_state.observe(on_focus, self.frame);
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    /// The job the dashboard follows, else the most recent one.
    pub fn current_job(&self) -> Option<&JobProgress> {
        if let Some(id) = self.dashboard.followed
            && let Some(p) = self.jobs.progress(id).or(self.dashboard.replayed())
        {
            return Some(p);
        }
        self.jobs.latest()
    }

    /// The review run of the current job, if there is one.
    pub fn review_run(&self) -> Option<&JobProgress> {
        let job = self.current_job()?;
        if let Some(r) = self.dashboard.replayed_review()
            && self.dashboard.replayed().map(|p| p.job_id) == Some(job.job_id)
        {
            return Some(r);
        }
        self.jobs.review_progress(job.job_id)
    }

    /// Where the current job's timeline ends (now, while anything runs).
    pub fn timeline_end(&self) -> u32 {
        let Some(job) = self.current_job() else {
            return 0;
        };
        match self.jobs.progress(job.job_id) {
            Some(_) => self.jobs.timeline_end(job.job_id),
            None => self
                .review_run()
                .map_or(0, |r| r.log_offset + r.last_tick)
                .max(job.last_tick),
        }
    }

    /// Whether the next prompts get a review.
    pub fn review_next(&self) -> bool {
        self.jobs.review_next()
    }

    /// The reviewer's model.
    pub fn review_model(&self) -> &str {
        self.jobs.review_model()
    }

    /// Running, queued and recently finished jobs.
    pub fn queue_rows(&self) -> Vec<QueueEntry> {
        self.jobs.queue()
    }

    pub fn stats(&self) -> QueueStats {
        self.jobs.stats()
    }

    /// The session a job belongs to, if the job is from this run.
    pub fn session_of(&self, job: JobId) -> Option<&str> {
        self.jobs.progress(job).map(|p| p.session_name.as_str())
    }

    /// The timeline's "now" for the current job.
    pub fn now_tick(&self) -> u32 {
        self.current_job()
            .map_or(0, |j| self.jobs.tick_now(j.job_id).max(j.last_tick))
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
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
                }
                InputAction::Submit(text) => {
                    self.mode = Mode::Normal;
                    let result = match purpose {
                        InputPurpose::NewSession => self.create_session(&text),
                        InputPurpose::Prompt => self.dispatch(&text),
                        InputPurpose::RoutedPrompt => self.route_prompt(&text),
                        InputPurpose::Variable => self.answer_variable(text),
                        InputPurpose::Search => Ok(()),
                    };
                    self.show(result);
                }
            }
            return;
        }
        if self.screen == Screen::Missions
            && let Some(composer) = &mut self.missions_screen.composer
        {
            self.error = None;
            self.notice = None;
            let action = composer.handle(key);
            let result = self.composer_action(action);
            self.show(result);
            return;
        }
        if let Some(form) = &mut self.prompts.form {
            self.error = None;
            match form.handle(key) {
                FormAction::None => {}
                FormAction::Cancel => self.prompts.form = None,
                FormAction::Save => {
                    let result = self.save_form();
                    self.show(result);
                }
            }
            return;
        }

        self.error = None;
        self.notice = None;
        if let KeyCode::Char(c @ ('y' | 'n')) = key.code
            && self.guard.jev().is_some_and(|j| j.wants_consent())
        {
            let result = self.answer_consent(c == 'y');
            self.show(result);
            return;
        }
        if let KeyCode::Char(c @ ('y' | 'n' | 'a')) = key.code
            && self.guard.pending_count() > 0
        {
            let result = self.answer_approval(c);
            self.show(result);
            return;
        }
        let confirming_delete = std::mem::take(&mut self.prompts.confirm_delete);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(c) if Screen::by_key(c).is_some() => {
                let left_focus = self.on_focus();
                self.screen = Screen::by_key(c).unwrap_or_default();
                if left_focus && !self.on_focus() {
                    self.note_breakpoint(Breakpoint::FocusClosed);
                }
                if self.screen == Screen::Ledger {
                    self.refresh_ledger();
                }
            }
            KeyCode::Char('t') if self.on_focus() => {
                let result = self.open_terminal();
                self.show(result);
            }
            KeyCode::Char('t') => {
                let name = self.look.next_theme().name;
                self.show_toast(format!("theme · {name}"));
            }
            KeyCode::Char('L') => {
                let name = self.look.next_layout().name.clone();
                self.show_toast(format!("layout · {name}"));
            }
            KeyCode::Tab | KeyCode::BackTab if self.screen == Screen::Mission => {
                self.mission.pane = match self.mission.pane {
                    Pane::Inbox => Pane::Wall,
                    Pane::Wall => Pane::Inbox,
                };
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k' | 'h')
                if self.screen == Screen::Mission && self.mission.pane == Pane::Wall =>
            {
                self.move_wall(-1);
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j' | 'l')
                if self.screen == Screen::Mission && self.mission.pane == Pane::Wall =>
            {
                self.move_wall(1);
            }
            KeyCode::Up | KeyCode::Char('k') if self.screen == Screen::Mission => {
                self.mission.move_selection(-1);
                let now = self.now();
                self.mission.attend_selected(now);
            }
            KeyCode::Down | KeyCode::Char('j') if self.screen == Screen::Mission => {
                self.mission.move_selection(1);
                let now = self.now();
                self.mission.attend_selected(now);
            }
            KeyCode::Char('x') if self.screen == Screen::Mission => {
                let now = self.now();
                self.mission.attend_selected(now);
                if !self.mission.acknowledge_selected() {
                    self.notice = Some("held tool calls stay until you answer them".into());
                }
            }
            KeyCode::Char('A') if self.screen == Screen::Mission => {
                let result = self.answer_batch();
                self.show(result);
            }
            KeyCode::Char('s') if self.screen == Screen::Mission => self.begin_shelve(),
            KeyCode::Char('u') if self.screen == Screen::Mission => self.unshelve(),
            KeyCode::Char(c @ ('r' | 'R'))
                if self.screen == Screen::Mission
                    && self
                        .mission
                        .selected_item()
                        .is_some_and(|i| i.suggest.is_some()) =>
            {
                let result = self.settle_rule(c == 'r');
                self.show(result);
            }
            KeyCode::Char('F') => self.toggle_focus_mode(),
            KeyCode::Enter if self.screen == Screen::Mission => {
                let now = self.now();
                self.mission.attend_selected(now);
                let result = self.open_selected();
                self.show(result);
            }
            KeyCode::Up | KeyCode::Char('k') if self.on_focus() => self.focus_state.scroll_by(1),
            KeyCode::Down | KeyCode::Char('j') if self.on_focus() => {
                self.focus_state.scroll_by(-1);
            }
            KeyCode::Char('J') if self.on_focus() => self.focus_state.scroll_answer(3),
            KeyCode::Char('K') if self.on_focus() => self.focus_state.scroll_answer(-3),
            KeyCode::PageUp if self.on_focus() => self.focus_state.scroll_by(10),
            KeyCode::PageDown if self.on_focus() => self.focus_state.scroll_by(-10),
            KeyCode::Home if self.on_focus() => self.focus_state.scroll_by(isize::MAX),
            KeyCode::End | KeyCode::Char('G') if self.on_focus() => self.focus_state.scroll = 0,
            KeyCode::Char('d') if self.on_focus() => {
                let result = self.toggle_diff();
                self.show(result);
            }
            KeyCode::Char(c @ ('[' | ']')) if self.on_focus() => self.scrub_checkpoint(c == ']'),
            KeyCode::Char('D') if self.screen == Screen::Dashboard => {
                self.dashboard.classic = !self.dashboard.classic;
                self.focus = None;
            }
            KeyCode::Tab | KeyCode::BackTab
                if self.screen == Screen::Dashboard && self.dashboard.classic =>
            {
                self.cycle_focus(key.code == KeyCode::Tab);
            }
            KeyCode::Esc if self.focus.is_some() => self.focus = None,
            code if self.screen == Screen::Missions => {
                let result = self.missions_key(code);
                self.show(result);
            }
            code if self.screen == Screen::Queue => {
                let result = self.queue_key(code);
                self.show(result);
            }
            code if self.screen == Screen::Ledger => {
                let result = self.ledger_key(code);
                self.show(result);
            }
            KeyCode::Up if self.screen == Screen::Prompts => self.prompts.up(),
            KeyCode::Down if self.screen == Screen::Prompts => self.prompts.down(),
            KeyCode::Up => self.select_index(self.selected.saturating_sub(1)),
            KeyCode::Down if self.selected + 1 < self.sessions.len() => {
                self.select_index(self.selected + 1);
            }
            KeyCode::Char('n') if self.screen == Screen::Sessions => {
                self.mode = Mode::Input(InputPurpose::NewSession, Input::default());
            }
            KeyCode::Char('l') if self.screen == Screen::Sessions => {
                let result = self.toggle_lean();
                self.show(result);
            }
            KeyCode::Char(c) if self.screen == Screen::Prompts => {
                let result = self.prompts_key(c, confirming_delete);
                self.show(result);
            }
            KeyCode::Char('v') if self.screen == Screen::Dashboard => {
                let on = !self.jobs.review_next();
                self.notice = Some(if self.jobs.set_review_next(on) {
                    format!(
                        "review on: the next jobs are audited by {} when they finish",
                        self.jobs.review_model()
                    )
                } else if on {
                    "review is not available: no git workspace configured".into()
                } else {
                    "review off for the next jobs".into()
                });
            }
            KeyCode::Char(c @ ('p' | 'P'))
                if matches!(self.screen, Screen::Dashboard | Screen::Mission) =>
            {
                if self.screen == Screen::Mission {
                    self.select_inbox_session();
                }
                if self.sessions.is_empty() {
                    self.error = Some("create a session first: press 6 then n".into());
                } else {
                    let purpose = if c == 'P' {
                        InputPurpose::RoutedPrompt
                    } else {
                        InputPurpose::Prompt
                    };
                    self.mode = Mode::Input(purpose, Input::default());
                }
            }
            _ => {}
        }
    }

    /// Takes the linter's replies, follows the missions' jobs and stops
    /// the jobs of missions out of budget.
    fn pump_missions(&mut self) -> Vec<AppError> {
        let Some(service) = &mut self.missions else {
            return Vec::new();
        };
        let report = service.pump(&self.mission.facts, &self.mission.gates);
        if let Some(composer) = &mut self.missions_screen.composer
            && let Some(m) = composer.slug.as_deref().and_then(|s| service.get(s))
        {
            composer.sync(m);
        }
        let mut errors = report.errors;
        for job in report.cancel {
            errors.extend(self.jobs.cancel(job).err());
        }
        errors
    }

    /// The mission of the selected card.
    fn selected_mission(&self) -> Option<&swarmux_domain::mission::Mission> {
        self.missions().get(self.missions_screen.selected)
    }

    fn missions_key(&mut self, code: KeyCode) -> AppResult<()> {
        let count = self.missions().len();
        let slug = self.selected_mission().map(|m| m.slug.clone());
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.missions_screen.move_by(-1, count),
            KeyCode::Down | KeyCode::Char('j') => self.missions_screen.move_by(1, count),
            KeyCode::Char('n') => {
                if self.missions.is_none() {
                    self.notice = Some("missions are not set up in this run".into());
                } else if self.sessions.is_empty() {
                    self.error = Some("create a session first: press 6 then n".into());
                } else {
                    self.missions_screen.composer = Some(Composer::blank());
                }
            }
            KeyCode::Char('e') => {
                if let Some(m) = self.selected_mission() {
                    if m.state.in_flight() {
                        self.notice =
                            Some(format!("{} is {}: it can't change now", m.slug, m.state));
                    } else {
                        self.missions_screen.composer = Some(Composer::edit(m));
                    }
                }
            }
            KeyCode::Char('l') => {
                if let (Some(service), Some(slug)) = (&mut self.missions, slug) {
                    service.lint(&slug)?;
                }
            }
            KeyCode::Char('f') => {
                if let Some(slug) = slug {
                    self.fix_mission(&slug)?;
                }
            }
            KeyCode::Char('r') => {
                if let Some(slug) = slug {
                    self.launch_mission(&slug)?;
                }
            }
            KeyCode::Enter => match self.selected_mission().and_then(|m| m.job) {
                Some(job) => self.view(job)?,
                None => self.notice = Some("that mission has not run yet: r runs it".into()),
            },
            _ => {}
        }
        Ok(())
    }

    fn composer_action(&mut self, action: ComposerAction) -> AppResult<()> {
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
                    (&self.missions, &mut self.missions_screen.composer)
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
        let (Some(service), Some(composer)) =
            (&mut self.missions, &mut self.missions_screen.composer)
        else {
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
                let slug = service.create(contract, &root, Some(session.id))?;
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

    fn fix_mission(&mut self, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.missions else {
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

    /// Compiles the mission into a job and follows it.
    fn launch_mission(&mut self, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.missions else {
            return Ok(());
        };
        let job = service.launch(slug, &mut self.jobs)?;
        self.focus_state.note_human(job, self.now());
        self.dashboard.follow(job, None);
        self.refresh_sessions(None);
        self.notice = Some(format!(
            "{slug} is out as job #{job}; the gate decides when it is done"
        ));
        Ok(())
    }

    /// Makes the session of the selected inbox item the one prompts go to.
    fn select_inbox_session(&mut self) {
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
    fn open_selected(&mut self) -> AppResult<()> {
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
    fn on_focus(&self) -> bool {
        self.screen == Screen::Dashboard && !self.dashboard.classic
    }

    /// The wall's tiles in their fixed order.
    pub(crate) fn wall(&self) -> Vec<Tile> {
        let held: Vec<_> = self
            .guard
            .approvals()
            .iter()
            .filter_map(|a| a.request.job_id)
            .collect();
        wall_tiles(
            &self.sessions,
            &self.mission.facts,
            &self.mission.gates,
            &held,
            self.mission.usage(),
            self.now(),
        )
    }

    fn move_wall(&mut self, delta: isize) {
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
    fn open_terminal(&mut self) -> AppResult<()> {
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
    fn toggle_diff(&mut self) -> AppResult<()> {
        if self.focus_state.diff.take().is_some() {
            self.note_breakpoint(Breakpoint::DiffReviewed);
            return Ok(());
        }
        let Some(cwd) = self.focus_session().map(|s| s.cwd.clone()) else {
            return Ok(());
        };
        match self.jobs.working_diff(&cwd)? {
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
    fn scrub_checkpoint(&mut self, forward: bool) {
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
    fn cycle_focus(&mut self, forward: bool) {
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

    fn show(&mut self, result: AppResult<()>) {
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
    }

    /// `y` approves, `n` denies, `a` always allows the oldest held tool call.
    fn answer_approval(&mut self, key: char) -> AppResult<()> {
        let Some(target) = self.approval_target() else {
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
            match key {
                'y' => self.begin_confirm(&target),
                'a' => {
                    self.notice =
                        Some("irreversible calls are never always-allowed: y to confirm".into());
                }
                _ => {
                    self.guard.deny(id)?;
                    self.mission.alarms_mut().answered_request(id, false);
                }
            }
            return Ok(());
        }
        if let Some(job) = target.request.job_id {
            self.focus_state.note_human(job, now);
        }
        self.mission.alarms_mut().answered_request(id, key != 'n');
        match key {
            'y' => self.guard.approve(id),
            'n' => self.guard.deny(id),
            _ => {
                let done = self.guard.always_allow(id)?;
                self.notice = Some(match done.not_saved {
                    None => format!("always allowing {} (saved to config.toml)", done.rule),
                    Some(why) => format!(
                        "always allowing {} for this run; not saved: {why}",
                        done.rule
                    ),
                });
                Ok(())
            }
        }
    }

    fn queue_key(&mut self, code: KeyCode) -> AppResult<()> {
        let rows = self.jobs.queue();
        match self.queue.handle(code, &rows) {
            QueueAction::None => {}
            QueueAction::Cancel(id) => self.jobs.cancel(id)?,
            QueueAction::Retry(id) => {
                let new = self.jobs.retry(id)?;
                self.queue.selected = Some(new);
            }
            QueueAction::Priority(id, delta) => {
                self.jobs.change_priority(id, delta)?;
            }
            QueueAction::View(id) => self.view(id)?,
        }
        Ok(())
    }

    fn ledger_key(&mut self, code: KeyCode) -> AppResult<()> {
        match self.ledger.handle(code) {
            LedgerAction::None => {}
            LedgerAction::Moved => self.load_detail(),
            LedgerAction::Filtered => self.refresh_ledger(),
            LedgerAction::Search => {
                let input = Input::with_text(&self.ledger.query);
                self.mode = Mode::Input(InputPurpose::Search, input);
            }
            LedgerAction::Rerun(id) => {
                self.jobs.retry(id)?;
                self.refresh_ledger();
            }
            LedgerAction::Fork(id) => {
                self.jobs.fork(id)?;
                self.refresh_ledger();
            }
            LedgerAction::View(id) => self.view(id)?,
            LedgerAction::Verify(slug) => {
                if let Some(evidence) = &self.evidence {
                    evidence.start_verify(&slug)?;
                    self.show_toast(format!("verifying {slug}"));
                }
            }
            LedgerAction::Labels => match &self.evidence {
                Some(evidence) => {
                    let asked = evidence.request_labels()?;
                    self.show_toast(format!("asked GitHub about {asked} branch(es)"));
                }
                None => self.notice = Some("evidence is not set up in this run".into()),
            },
        }
        Ok(())
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
    fn view(&mut self, job: JobId) -> AppResult<()> {
        // A review job is shown as part of the job it audits.
        let mut job = self.jobs.subject_of(job);
        let mut told = None;
        let replay = match self.jobs.progress(job) {
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
        self.screen = Screen::Dashboard;
        Ok(())
    }

    /// Rebuilds the run cards: every job joined with its mission and
    /// bundle, then the search and filter applied.
    fn refresh_ledger(&mut self) {
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

    fn load_detail(&mut self) {
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

    fn prompts_key(&mut self, c: char, confirming_delete: bool) -> AppResult<()> {
        match c {
            'n' => self.prompts.form = Some(PromptForm::blank()),
            'e' => {
                if let Some(p) = self.prompts.selected_prompt() {
                    self.prompts.form = Some(PromptForm::edit(p));
                }
            }
            'x' => {
                if let Some(id) = self.prompts.selected_prompt().map(|p| p.id) {
                    if confirming_delete {
                        self.prompt_service.delete(id)?;
                        self.refresh_prompts(None);
                    } else {
                        self.prompts.confirm_delete = true;
                    }
                }
            }
            'd' => return self.start_template_dispatch(false),
            'D' => return self.start_template_dispatch(true),
            _ => {}
        }
        Ok(())
    }

    fn save_form(&mut self) -> AppResult<()> {
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
    fn start_template_dispatch(&mut self, route: bool) -> AppResult<()> {
        let Some(prompt) = self.prompts.selected_prompt() else {
            self.error = Some("no prompt selected: press n to write one".into());
            return Ok(());
        };
        let Some(session) = self.selected_session() else {
            self.error = Some("create a session first: press 6 then n".into());
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
            self.screen = Screen::Dashboard;
            return self.route_template(pending);
        }
        let rendered =
            self.prompt_service
                .render(pending.prompt_id, pending.session_id, &pending.values)?;
        let job = self.jobs.dispatch_template(pending.session_id, &rendered)?;
        self.focus_state.note_human(job, self.now());
        self.dashboard.follow(job, None);
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == pending.session_id)
            .cloned();
        self.refresh_sessions(session.as_ref());
        self.screen = Screen::Dashboard;
        Ok(())
    }

    pub(crate) fn refresh_prompts(&mut self, select: Option<PromptId>) {
        match self.prompt_service.list() {
            Ok(list) => self.prompts.set_list(list, select),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Lean on or off for the selected session's next jobs.
    fn toggle_lean(&mut self) -> AppResult<()> {
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

    fn dispatch(&mut self, prompt: &str) -> AppResult<()> {
        let session = self
            .selected_session()
            .cloned()
            .ok_or(AppError::SessionNotFound)?;
        let job = self.jobs.dispatch(session.id, prompt)?;
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
        if let Some(target) = select {
            self.selected_id = Some(target.id);
        }
        let found = self
            .selected_id
            .and_then(|id| self.sessions.iter().position(|s| s.id == id));
        let i = found.unwrap_or(self.selected);
        self.select_index(i.min(self.sessions.len().saturating_sub(1)));
    }

    fn select_index(&mut self, i: usize) {
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
        FakeAdvisor, FakeAgentCatalog, FakeConsents, FakeDecisionRepo, FakeHookInbox, FakeJobGates,
        FakeLinter, FakeMonitor, FakeRouter, FakeRuleWriter, FakeRunner, FakeTelemetry,
        FakeTerminal, FakeUsageProbe, FakeWorkspace, FixedClock, InMemoryMissions, InMemoryStore,
    };
    use swarmux_application::{
        Governor, GovernorSettings, GuardService, HistoryService, Jev, JobService, MissionService,
        PromptService, ReviewSettings, ReviewWhen, RoutingService, SessionService, SharedPolicy,
        TelemetryService, TelemetrySettings,
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
        pub rules: Rc<FakeRuleWriter>,
        pub router: Rc<FakeRouter>,
        pub catalog: Rc<FakeAgentCatalog>,
        /// What Mission control's telemetry strip reads.
        pub feed: Rc<FakeTelemetry>,
        /// Records what `t` on Focus opens.
        pub terminal: Rc<FakeTerminal>,
        /// Set by [`test_app_with_jev`].
        pub advisor: Option<Rc<FakeAdvisor>>,
        /// Where the Missions screen keeps its missions.
        pub missions: Rc<InMemoryMissions>,
        /// Answers mission lints when told to.
        pub linter: Rc<FakeLinter>,
        /// The gates mission jobs were given.
        pub job_gates: Rc<FakeJobGates>,
        /// The usage windows the governor reads.
        pub usage: Rc<FakeUsageProbe>,
    }

    /// An app wired to in-memory fakes.
    pub(crate) fn test_app() -> TestApp {
        test_app_with_store(Rc::new(InMemoryStore::default()))
    }

    /// An app with JEV configured; `consented` says whether the user already agreed.
    pub(crate) fn test_app_with_jev(consented: bool) -> TestApp {
        build(Rc::new(InMemoryStore::default()), Some(consented))
    }

    /// An app over an existing store, like a restart.
    pub(crate) fn test_app_with_store(store: Rc<InMemoryStore>) -> TestApp {
        build(store, None)
    }

    fn build(store: Rc<InMemoryStore>, jev: Option<bool>) -> TestApp {
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
        let rules = Rc::new(FakeRuleWriter::default());
        let mut guard = GuardService::new(
            SharedPolicy::new(GuardPolicy::default()),
            inbox.clone(),
            decisions.clone(),
            clock.clone(),
        )
        .with_rule_writer(rules.clone());
        let router = Rc::new(FakeRouter::default());
        let mut routing = RoutingService::new(
            store.clone(),
            store.clone(),
            decisions.clone(),
            clock.clone(),
        )
        .with_router(router.clone());
        let feed = Rc::new(FakeTelemetry::default());
        let terminal = Rc::new(FakeTerminal::default());
        let service = TelemetryService::new(feed.clone(), store.clone(), clock.clone());
        let mut advisor = None;
        if let Some(consented) = jev {
            let fake = Rc::new(FakeAdvisor::default());
            let consents = Rc::new(FakeConsents::default());
            let jev = Rc::new(Jev::new(fake.clone(), consents, clock.clone(), 0.85).unwrap());
            if consented {
                jev.grant().unwrap();
            }
            guard = guard.with_jev(jev.clone());
            routing = routing.with_jev(jev);
            advisor = Some(fake);
        }
        let missions = Rc::new(InMemoryMissions::default());
        let linter = Rc::new(FakeLinter::default());
        let job_gates = Rc::new(FakeJobGates::default());
        let mission_service = MissionService::new(missions.clone(), clock.clone())
            .unwrap()
            .with_linter(linter.clone())
            .with_gates(job_gates.clone());
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
            advisor,
            missions,
            linter,
            job_gates,
            usage,
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
        press(&mut t.app, KeyCode::Char('7'));
        assert_eq!(t.app.screen, Screen::Dashboard, "no such screen");
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
        let id = t.app.jobs.dispatch(a, "elsewhere").unwrap();
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
        assert!(t.app.jobs.progress(id).is_some());
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
