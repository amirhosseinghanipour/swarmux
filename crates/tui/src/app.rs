//! UI state and key handling. All work goes through application use cases.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use swarmux_application::{
    AppError, AppResult, HistoryService, JobService, PromptService, QueueEntry, QueueStats,
    SessionService,
};
use swarmux_domain::{JobId, JobProgress, PromptId, Session};

use crate::dashboard::DashboardState;
use crate::history::{HistoryAction, HistoryScreen};
use crate::prompts::{FormAction, PendingDispatch, PromptForm, PromptsScreen};
use crate::queue::{QueueAction, QueueScreen};
use crate::ui::input::{Input, InputAction};

/// Redraw budget: about 10 frames per second.
const FRAME: Duration = Duration::from_millis(100);

/// The five top-level screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Dashboard,
    Prompts,
    Queue,
    History,
    Sessions,
}

impl Screen {
    pub const ALL: [Screen; 5] = [
        Screen::Dashboard,
        Screen::Prompts,
        Screen::Queue,
        Screen::History,
        Screen::Sessions,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Dashboard => "dashboard",
            Screen::Prompts => "prompts",
            Screen::Queue => "queue",
            Screen::History => "history",
            Screen::Sessions => "sessions",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPurpose {
    NewSession,
    Prompt,
    /// A value for the template variable `prompts.pending` is asking for.
    Variable,
    /// The history search; applied as it is typed.
    Search,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Normal,
    Input(InputPurpose, Input),
}

pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub mode: Mode,
    pub sessions: Vec<Session>,
    pub selected: usize,
    /// Last error, shown in the status bar until the next key press.
    pub error: Option<String>,
    pub prompts: PromptsScreen,
    pub queue: QueueScreen,
    pub history: HistoryScreen,
    pub dashboard: DashboardState,
    session_service: SessionService,
    jobs: JobService,
    prompt_service: PromptService,
    history_service: HistoryService,
    cwd: PathBuf,
}

impl App {
    pub fn new(
        session_service: SessionService,
        jobs: JobService,
        prompt_service: PromptService,
        history_service: HistoryService,
        cwd: PathBuf,
    ) -> App {
        let mut app = App {
            screen: Screen::default(),
            quit: false,
            mode: Mode::Normal,
            sessions: Vec::new(),
            selected: 0,
            error: None,
            prompts: PromptsScreen::default(),
            queue: QueueScreen::default(),
            history: HistoryScreen::default(),
            dashboard: DashboardState::default(),
            session_service,
            jobs,
            prompt_service,
            history_service,
            cwd,
        };
        app.refresh_sessions(None);
        app.refresh_prompts(None);
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.quit {
            self.update();
            terminal.draw(|frame| crate::ui::draw(frame, &self))?;
            if event::poll(FRAME)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key);
            }
        }
        Ok(())
    }

    /// Applies pending job updates and starts queued jobs.
    pub fn update(&mut self) {
        let report = self.jobs.pump();
        if let Some(e) = report.errors.into_iter().last() {
            self.error = Some(e.to_string());
        }
        if !report.new_sessions.is_empty() {
            let current = self.selected_session().cloned();
            self.refresh_sessions(current.as_ref());
        }
        if report.changed && self.screen == Screen::History {
            self.refresh_history();
        }
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

    /// Running, queued and recently finished jobs.
    pub fn queue_rows(&self) -> Vec<QueueEntry> {
        self.jobs.queue()
    }

    pub fn stats(&self) -> QueueStats {
        self.jobs.stats()
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
        if let Mode::Input(purpose, input) = &mut self.mode {
            let purpose = *purpose;
            match input.handle(key) {
                InputAction::None => {
                    if purpose == InputPurpose::Search {
                        self.history.query = input.text.clone();
                        self.refresh_history();
                    }
                }
                InputAction::Cancel => {
                    self.mode = Mode::Normal;
                    self.prompts.pending = None;
                    if purpose == InputPurpose::Search {
                        self.history.query.clear();
                        self.refresh_history();
                    }
                }
                InputAction::Submit(text) => {
                    self.mode = Mode::Normal;
                    let result = match purpose {
                        InputPurpose::NewSession => self.create_session(&text),
                        InputPurpose::Prompt => self.dispatch(&text),
                        InputPurpose::Variable => self.answer_variable(text),
                        InputPurpose::Search => Ok(()),
                    };
                    self.show(result);
                }
            }
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
        let confirming_delete = std::mem::take(&mut self.prompts.confirm_delete);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(c @ '1'..='5') => {
                self.screen = Screen::ALL[(c as u8 - b'1') as usize];
                if self.screen == Screen::History {
                    self.refresh_history();
                }
            }
            code if self.screen == Screen::Queue => {
                let result = self.queue_key(code);
                self.show(result);
            }
            code if self.screen == Screen::History => {
                let result = self.history_key(code);
                self.show(result);
            }
            KeyCode::Up if self.screen == Screen::Prompts => self.prompts.up(),
            KeyCode::Down if self.screen == Screen::Prompts => self.prompts.down(),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down if self.selected + 1 < self.sessions.len() => self.selected += 1,
            KeyCode::Char('n') if self.screen == Screen::Sessions => {
                self.mode = Mode::Input(InputPurpose::NewSession, Input::default());
            }
            KeyCode::Char(c) if self.screen == Screen::Prompts => {
                let result = self.prompts_key(c, confirming_delete);
                self.show(result);
            }
            KeyCode::Char('p') if self.screen == Screen::Dashboard => {
                if self.sessions.is_empty() {
                    self.error = Some("create a session first: press 5 then n".into());
                } else {
                    self.mode = Mode::Input(InputPurpose::Prompt, Input::default());
                }
            }
            _ => {}
        }
    }

    fn show(&mut self, result: AppResult<()>) {
        if let Err(e) = result {
            self.error = Some(e.to_string());
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

    fn history_key(&mut self, code: KeyCode) -> AppResult<()> {
        match self.history.handle(code) {
            HistoryAction::None => {}
            HistoryAction::Moved => self.load_detail(),
            HistoryAction::Search => {
                let input = Input::with_text(&self.history.query);
                self.mode = Mode::Input(InputPurpose::Search, input);
            }
            HistoryAction::Rerun(id) => {
                self.jobs.retry(id)?;
                self.refresh_history();
            }
            HistoryAction::Fork(id) => {
                self.jobs.fork(id)?;
                self.refresh_history();
            }
            HistoryAction::View(id) => self.view(id)?,
        }
        Ok(())
    }

    /// Shows `job` on the dashboard, rebuilding it from history if it is
    /// not from this run.
    fn view(&mut self, job: JobId) -> AppResult<()> {
        let replay = match self.jobs.progress(job) {
            Some(_) => None,
            None => Some(self.history_service.replay(job)?),
        };
        self.dashboard.follow(job, replay);
        self.screen = Screen::Dashboard;
        Ok(())
    }

    fn refresh_history(&mut self) {
        match self.history_service.list(&self.history.query) {
            Ok(rows) => self.history.set_rows(rows),
            Err(e) => self.error = Some(e.to_string()),
        }
        self.load_detail();
    }

    fn load_detail(&mut self) {
        self.history.detail = None;
        if let Some(id) = self.history.selected_row().map(|r| r.job_id) {
            match self.history_service.detail(id) {
                Ok(d) => self.history.detail = Some(d),
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
            'd' => return self.start_template_dispatch(),
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

    /// Asks for the selected template's variables, then dispatches it.
    fn start_template_dispatch(&mut self) -> AppResult<()> {
        let Some(prompt) = self.prompts.selected_prompt() else {
            self.error = Some("no prompt selected: press n to write one".into());
            return Ok(());
        };
        let Some(session) = self.selected_session() else {
            self.error = Some("create a session first: press 5 then n".into());
            return Ok(());
        };
        self.prompts.pending = Some(PendingDispatch {
            prompt_id: prompt.id,
            session_id: session.id,
            remaining: PromptService::user_variables(&prompt.body),
            values: Default::default(),
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
        let rendered =
            self.prompt_service
                .render(pending.prompt_id, pending.session_id, &pending.values)?;
        let job = self.jobs.dispatch_template(pending.session_id, &rendered)?;
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

    fn refresh_prompts(&mut self, select: Option<PromptId>) {
        match self.prompt_service.list() {
            Ok(list) => self.prompts.set_list(list, select),
            Err(e) => self.error = Some(e.to_string()),
        }
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
        self.dashboard.follow(job, None);
        self.refresh_sessions(Some(&session));
        Ok(())
    }

    fn refresh_sessions(&mut self, select: Option<&Session>) {
        match self.session_service.list() {
            Ok(list) => self.sessions = list,
            Err(e) => self.error = Some(e.to_string()),
        }
        if let Some(target) = select
            && let Some(i) = self.sessions.iter().position(|s| s.id == target.id)
        {
            self.selected = i;
        }
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::rc::Rc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use swarmux_application::ports::PromptRepository;
    use swarmux_application::testing::{
        FakeMonitor, FakeRunner, FakeWorkspace, FixedClock, InMemoryStore,
    };
    use swarmux_application::{HistoryService, JobService, PromptService, SessionService};
    use swarmux_domain::PromptDraft;

    use super::App;

    pub(crate) struct TestApp {
        pub app: App,
        pub store: Rc<InMemoryStore>,
        pub runner: Rc<FakeRunner>,
        pub clock: Rc<FixedClock>,
        pub monitor: Rc<FakeMonitor>,
    }

    /// An app wired to in-memory fakes.
    pub(crate) fn test_app() -> TestApp {
        test_app_with_store(Rc::new(InMemoryStore::default()))
    }

    /// An app over an existing store, like a restart.
    pub(crate) fn test_app_with_store(store: Rc<InMemoryStore>) -> TestApp {
        let runner = Rc::new(FakeRunner::default());
        let clock = Rc::new(FixedClock::new(1_000));
        let monitor = Rc::new(FakeMonitor::default());
        let workspace = Rc::new(FakeWorkspace::with_diff("+diff"));
        let sessions = SessionService::new(store.clone(), clock.clone());
        let jobs = JobService::new(store.clone(), store.clone(), runner.clone(), clock.clone())
            .with_monitor(monitor.clone());
        let prompts = PromptService::new(store.clone(), store.clone(), workspace, clock.clone());
        let history = HistoryService::new(store.clone());
        TestApp {
            app: App::new(sessions, jobs, prompts, history, "/work".into()),
            store,
            runner,
            clock,
            monitor,
        }
    }

    impl TestApp {
        /// Saves a prompt straight to the store and reloads the list.
        fn keys(&mut self, codes: impl IntoIterator<Item = KeyCode>) {
            for code in codes {
                self.app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
        }

        /// Creates a session through the sessions screen, then goes back to the dashboard.
        pub(crate) fn with_session(&mut self, name: &str) {
            self.keys([KeyCode::Char('5'), KeyCode::Char('n')]);
            self.keys(name.chars().map(KeyCode::Char));
            self.keys([KeyCode::Enter, KeyCode::Char('1')]);
        }

        /// Sends a prompt to the selected session from the dashboard.
        pub(crate) fn send(&mut self, prompt: &str) {
            self.keys([KeyCode::Char('1'), KeyCode::Char('p')]);
            self.keys(prompt.chars().map(KeyCode::Char));
            self.keys([KeyCode::Enter]);
        }

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
        press(app, KeyCode::Char('5'));
        press(app, KeyCode::Char('n'));
        type_str(app, "demo");
        press(app, KeyCode::Enter);
        press(app, KeyCode::Char('1'));
    }

    #[test]
    fn number_keys_switch_screens() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('3'));
        assert_eq!(t.app.screen, Screen::Queue);
        press(&mut t.app, KeyCode::Char('1'));
        assert_eq!(t.app.screen, Screen::Dashboard);
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
    fn q_inside_input_types_instead_of_quitting() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('5'));
        press(&mut t.app, KeyCode::Char('n'));
        press(&mut t.app, KeyCode::Char('q'));
        assert!(!t.app.quit);
        assert!(matches!(&t.app.mode, Mode::Input(_, i) if i.text == "q"));
    }

    #[test]
    fn prompt_without_session_shows_hint() {
        let mut t = test_app();
        press(&mut t.app, KeyCode::Char('p'));
        assert_eq!(t.app.mode, Mode::Normal);
        assert!(t.app.error.as_deref().unwrap().contains("press 5 then n"));
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
        press(app, KeyCode::Char('2'));
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
        press(&mut t.app, KeyCode::Char('2'));
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
        assert!(t.app.error.as_deref().unwrap().contains("press 5 then n"));
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
