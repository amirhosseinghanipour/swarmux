//! UI state and key handling. All work goes through application use cases.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use swarmux_application::{AppError, AppResult, JobService, SessionService};
use swarmux_domain::{JobProgress, Session};

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
    session_service: SessionService,
    jobs: JobService,
    cwd: PathBuf,
}

impl App {
    pub fn new(session_service: SessionService, jobs: JobService, cwd: PathBuf) -> App {
        let mut app = App {
            screen: Screen::default(),
            quit: false,
            mode: Mode::Normal,
            sessions: Vec::new(),
            selected: 0,
            error: None,
            session_service,
            jobs,
            cwd,
        };
        app.refresh_sessions(None);
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

    /// Applies pending job updates.
    pub fn update(&mut self) {
        if let Some(e) = self.jobs.pump().into_iter().last() {
            self.error = Some(e.to_string());
        }
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    pub fn current_job(&self) -> Option<&JobProgress> {
        self.jobs.latest()
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
                InputAction::None => {}
                InputAction::Cancel => self.mode = Mode::Normal,
                InputAction::Submit(text) => {
                    self.mode = Mode::Normal;
                    let result = match purpose {
                        InputPurpose::NewSession => self.create_session(&text),
                        InputPurpose::Prompt => self.dispatch(&text),
                    };
                    if let Err(e) = result {
                        self.error = Some(e.to_string());
                    }
                }
            }
            return;
        }

        self.error = None;
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(c @ '1'..='5') => {
                self.screen = Screen::ALL[(c as u8 - b'1') as usize];
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down if self.selected + 1 < self.sessions.len() => self.selected += 1,
            KeyCode::Char('n') if self.screen == Screen::Sessions => {
                self.mode = Mode::Input(InputPurpose::NewSession, Input::default());
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
        self.jobs.dispatch(session.id, prompt)?;
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

    use swarmux_application::testing::{FakeRunner, FixedClock, InMemoryStore};
    use swarmux_application::{JobService, SessionService};

    use super::App;

    pub(crate) struct TestApp {
        pub app: App,
        pub runner: Rc<FakeRunner>,
        pub clock: Rc<FixedClock>,
    }

    /// An app wired to in-memory fakes.
    pub(crate) fn test_app() -> TestApp {
        let store = Rc::new(InMemoryStore::default());
        let runner = Rc::new(FakeRunner::default());
        let clock = Rc::new(FixedClock::new(1_000));
        let sessions = SessionService::new(store.clone(), clock.clone());
        let jobs = JobService::new(store.clone(), store, runner.clone(), clock.clone());
        TestApp {
            app: App::new(sessions, jobs, "/work".into()),
            runner,
            clock,
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
    fn busy_session_error_is_shown() {
        let mut t = test_app();
        with_session(&mut t.app);
        for _ in 0..2 {
            press(&mut t.app, KeyCode::Char('p'));
            type_str(&mut t.app, "x");
            press(&mut t.app, KeyCode::Enter);
        }
        assert!(t.app.error.as_deref().unwrap().contains("still running"));
    }
}
