//! App state and update logic. The UI only reads this; all I/O happens here or in tasks.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::claude::{Event, JobSpec, RunnerMsg, run_job};
use crate::store::{JobResult, JobStatus, Session, Store};
use crate::ui::input::{Input, InputAction};

/// The five top-level screens (spec section 7).
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

const LOG_CAP: usize = 500;
const LOG_TEXT_MAX: usize = 160;

/// One row of the timeline: everything a single actor did.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    /// `"lead"` or the parent `tool_use` id of a subagent.
    pub actor: String,
    pub label: String,
    /// Tenths of a second since the job started.
    pub marks: Vec<u32>,
}

/// Live view of one job.
#[derive(Clone, Debug)]
pub struct JobView {
    pub job_id: i64,
    pub session_name: String,
    pub status: JobStatus,
    pub model: String,
    pub log: VecDeque<String>,
    pub lanes: Vec<Lane>,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_used: u64,
    pub context_window: Option<u64>,
    pub started: Instant,
    /// Subagent `tool_use` id → label (e.g. "explorer").
    task_labels: HashMap<String, String>,
}

impl JobView {
    pub fn new(job_id: i64, session_name: &str, model: &str) -> JobView {
        JobView {
            job_id,
            session_name: session_name.to_string(),
            status: JobStatus::Running,
            model: model.to_string(),
            log: VecDeque::new(),
            lanes: vec![Lane {
                actor: "lead".into(),
                label: "lead".into(),
                marks: Vec::new(),
            }],
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
            context_used: 0,
            context_window: None,
            started: Instant::now(),
            task_labels: HashMap::new(),
        }
    }

    pub fn tick(&self) -> u32 {
        (self.started.elapsed().as_millis() / 100) as u32
    }

    fn label_for(&self, actor: &str) -> String {
        if actor == "lead" {
            return "lead".into();
        }
        self.task_labels
            .get(actor)
            .cloned()
            .unwrap_or_else(|| actor.to_string())
    }

    fn mark(&mut self, actor: &str) {
        let tick = self.tick();
        let label = self.label_for(actor);
        match self.lanes.iter_mut().find(|l| l.actor == actor) {
            Some(lane) => lane.marks.push(tick),
            None => self.lanes.push(Lane {
                actor: actor.to_string(),
                label,
                marks: vec![tick],
            }),
        }
    }

    fn log_line(&mut self, label: &str, kind: &str, text: &str) {
        let secs = self.started.elapsed().as_secs();
        let line = format!(
            "{:02}:{:02} {label:<10} {kind:<11} {text}",
            secs / 60,
            secs % 60
        );
        self.log.push_back(line);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }
}

pub struct App {
    pub screen: Screen,
    pub quit: bool,
    pub mode: Mode,
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub jobs: Vec<JobView>,
    /// Last error, shown in the status bar until the next key press.
    pub error: Option<String>,
    store: Store,
    claude_bin: PathBuf,
    rt: Handle,
    cwd: PathBuf,
    tx: UnboundedSender<(i64, RunnerMsg)>,
    rx: UnboundedReceiver<(i64, RunnerMsg)>,
}

impl App {
    pub fn new(store: Store, claude_bin: PathBuf, rt: Handle, cwd: PathBuf) -> App {
        let (tx, rx) = unbounded_channel();
        let mut app = App {
            screen: Screen::default(),
            quit: false,
            mode: Mode::Normal,
            sessions: Vec::new(),
            selected: 0,
            jobs: Vec::new(),
            error: None,
            store,
            claude_bin,
            rt,
            cwd,
            tx,
            rx,
        };
        app.reload_sessions(None);
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            self.drain();
            terminal.draw(|frame| crate::ui::draw(frame, &self))?;
            // ~10 fps: redraw at least every 100 ms, react to keys immediately.
            if event::poll(Duration::from_millis(100))?
                && let TermEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key);
            }
        }
        Ok(())
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    pub fn current_job(&self) -> Option<&JobView> {
        self.jobs.last()
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
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        let result = match purpose {
                            InputPurpose::NewSession => self.create_session(&text),
                            InputPurpose::Prompt => self.dispatch(text),
                        };
                        if let Err(e) = result {
                            self.error = Some(e.to_string());
                        }
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

    fn reload_sessions(&mut self, select: Option<uuid::Uuid>) {
        match self.store.list_sessions() {
            Ok(list) => self.sessions = list,
            Err(e) => self.error = Some(e.to_string()),
        }
        if let Some(id) = select
            && let Some(i) = self.sessions.iter().position(|s| s.id == id)
        {
            self.selected = i;
        }
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
    }

    fn create_session(&mut self, name: &str) -> anyhow::Result<()> {
        let s = self.store.create_session(name, &self.cwd, "sonnet")?;
        self.reload_sessions(Some(s.id));
        Ok(())
    }

    fn dispatch(&mut self, prompt: String) -> anyhow::Result<()> {
        let session = self
            .selected_session()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no session selected"))?;
        let busy = self
            .jobs
            .iter()
            .any(|j| j.session_name == session.name && !j.status.is_finished());
        if busy {
            anyhow::bail!("session {} is still running a job", session.name);
        }

        let resume = self.store.session_started(session.id)?;
        let job_id = self.store.insert_job(session.id, &prompt, &session.model)?;
        self.store.set_job_status(job_id, JobStatus::Running)?;
        self.store.touch_session(session.id)?;

        let spec = JobSpec {
            claude_bin: self.claude_bin.clone(),
            cwd: session.cwd.clone(),
            session_id: session.id,
            resume,
            fork: false,
            model: session.model.clone(),
            permission_mode: session.permission_mode.clone(),
            env: Vec::new(),
        };
        let (job_tx, mut job_rx) = unbounded_channel();
        let app_tx = self.tx.clone();
        self.rt.spawn(run_job(spec, prompt, job_tx));
        self.rt.spawn(async move {
            while let Some(msg) = job_rx.recv().await {
                if app_tx.send((job_id, msg)).is_err() {
                    break;
                }
            }
        });

        self.jobs
            .push(JobView::new(job_id, &session.name, &session.model));
        self.reload_sessions(Some(session.id));
        Ok(())
    }

    /// Applies every pending runner message to state and the store.
    pub fn drain(&mut self) {
        while let Ok((job_id, msg)) = self.rx.try_recv() {
            if let Err(e) = self.apply(job_id, msg) {
                self.error = Some(e.to_string());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn inject(&self, job_id: i64, msg: RunnerMsg) {
        self.tx.send((job_id, msg)).unwrap();
    }

    fn apply(&mut self, job_id: i64, msg: RunnerMsg) -> anyhow::Result<()> {
        let Some(view) = self.jobs.iter_mut().find(|j| j.job_id == job_id) else {
            return Ok(());
        };
        match msg {
            RunnerMsg::Event(ev) => apply_event(&self.store, view, ev),
            RunnerMsg::Exited { code, stderr_tail } => {
                if view.status.is_finished() {
                    return Ok(());
                }
                let reason = match code {
                    Some(c) => format!("claude exited with code {c}"),
                    None => "claude was terminated".to_string(),
                };
                let tail = stderr_tail.trim();
                let error = if tail.is_empty() {
                    reason
                } else {
                    format!("{reason}: {tail}")
                };
                fail(&self.store, view, error)
            }
            RunnerMsg::SpawnFailed(msg) => fail(&self.store, view, format!("spawn failed: {msg}")),
        }
    }
}

fn apply_event(store: &Store, view: &mut JobView, ev: Event) -> anyhow::Result<()> {
    if matches!(ev, Event::Ignored { .. }) {
        return Ok(());
    }
    if let Event::ToolUse {
        id, name, summary, ..
    } = &ev
        && (name == "Task" || name == "Agent")
    {
        let label = summary.split(':').next().unwrap_or(name).trim();
        let label = if label.is_empty() { name } else { label };
        view.task_labels.insert(id.clone(), label.to_string());
    }

    let actor = ev.actor();
    let label = view.label_for(&actor);
    let text = describe(&ev);
    store.insert_event(view.job_id, ev.kind(), &actor, &text)?;
    view.mark(&actor);

    match ev {
        Event::Usage {
            input,
            cache_read,
            cache_creation,
            parent: None,
            ..
        } => view.context_used = input + cache_read + cache_creation,
        Event::Usage { .. } => {}
        Event::Init { model, .. } => {
            view.model = model;
            view.log_line(&label, "init", &text);
        }
        Event::Result {
            is_error,
            subtype,
            cost_usd,
            duration_ms,
            tokens_in,
            tokens_out,
            cache_read,
            context_window,
            ..
        } => {
            view.cost_usd = cost_usd;
            view.tokens_in = tokens_in;
            view.tokens_out = tokens_out;
            view.context_window = context_window;
            view.status = if is_error {
                JobStatus::Failed
            } else {
                JobStatus::Done
            };
            view.log_line(&label, "result", &text);
            store.save_result(
                view.job_id,
                &JobResult {
                    tokens_in,
                    tokens_out,
                    cache_read,
                    cost_usd,
                    duration_ms,
                    outcome: subtype,
                    error: None,
                },
            )?;
            store.set_job_status(view.job_id, view.status)?;
        }
        other => view.log_line(&label, other.kind(), &text),
    }
    Ok(())
}

fn fail(store: &Store, view: &mut JobView, error: String) -> anyhow::Result<()> {
    view.status = JobStatus::Failed;
    view.log_line("swarmux", "error", &error);
    store.save_result(
        view.job_id,
        &JobResult {
            tokens_in: view.tokens_in,
            tokens_out: view.tokens_out,
            cache_read: 0,
            cost_usd: view.cost_usd,
            duration_ms: view.started.elapsed().as_millis() as u64,
            outcome: "failed".into(),
            error: Some(error),
        },
    )?;
    store.set_job_status(view.job_id, JobStatus::Failed)
}

/// Short human text for an event, used for the log line and the stored payload.
fn describe(ev: &Event) -> String {
    let s = match ev {
        Event::Init { model, cwd, .. } => format!("{model} in {cwd}"),
        Event::Text { text, .. } => text.lines().next().unwrap_or_default().to_string(),
        Event::ToolUse { name, summary, .. } if summary == name => name.clone(),
        Event::ToolUse { name, summary, .. } => format!("{name} {summary}"),
        Event::ToolResult {
            is_error, preview, ..
        } => {
            let first = preview.lines().next().unwrap_or_default();
            if *is_error {
                format!("ERROR {first}")
            } else {
                first.to_string()
            }
        }
        Event::Usage { input, output, .. } => format!("in {input} out {output}"),
        Event::Result {
            cost_usd,
            duration_ms,
            num_turns,
            ..
        } => format!(
            "${cost_usd:.4} · {:.1}s · {num_turns} turns",
            *duration_ms as f64 / 1000.0
        ),
        Event::Ignored { kind } | Event::Unknown { kind, .. } => kind.clone(),
        Event::Malformed { raw } => raw.clone(),
    };
    match s.char_indices().nth(LOG_TEXT_MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use tokio::runtime::Runtime;

    /// An app on an in-memory store whose `claude` can't be spawned.
    pub(crate) fn test_app() -> (App, Runtime) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let app = App::new(
            Store::open_in_memory().unwrap(),
            "definitely-not-a-binary-xyz".into(),
            rt.handle().clone(),
            "/work".into(),
        );
        (app, rt)
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::test_app;
    use super::*;
    use tokio::runtime::Runtime;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn app_with_job() -> (App, Runtime, i64) {
        let (mut app, rt) = test_app();
        let s = app
            .store
            .create_session("demo", "/w".as_ref(), "sonnet")
            .unwrap();
        let job = app.store.insert_job(s.id, "hi", "sonnet").unwrap();
        app.store.set_job_status(job, JobStatus::Running).unwrap();
        app.jobs.push(JobView::new(job, "demo", "sonnet"));
        (app, rt, job)
    }

    fn result_event(cost: f64, is_error: bool) -> Event {
        Event::Result {
            is_error,
            subtype: "success".into(),
            cost_usd: cost,
            duration_ms: 1000,
            num_turns: 1,
            tokens_in: 10,
            tokens_out: 20,
            cache_read: 0,
            context_window: Some(1_000_000),
            subagents_spawned: 0,
        }
    }

    #[test]
    fn number_keys_switch_screens() {
        let (mut app, _rt) = test_app();
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.screen, Screen::Queue);
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.screen, Screen::Dashboard);
    }

    #[test]
    fn q_quits() {
        let (mut app, _rt) = test_app();
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn create_session_via_keys() {
        let (mut app, _rt) = test_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('n'));
        type_str(&mut app, "demo");
        press(&mut app, KeyCode::Enter);
        let names: Vec<_> = app.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["demo"]);
        assert_eq!(app.sessions[0].cwd, PathBuf::from("/work"));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn q_inside_input_types_instead_of_quitting() {
        let (mut app, _rt) = test_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        assert!(matches!(&app.mode, Mode::Input(_, i) if i.text == "q"));
    }

    #[test]
    fn prompt_without_session_shows_hint() {
        let (mut app, _rt) = test_app();
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.error.as_deref().unwrap().contains("press 5 then n"));
    }

    #[test]
    fn dispatch_with_missing_claude_fails_cleanly() {
        let (mut app, rt) = test_app();
        app.create_session("demo").unwrap();
        press(&mut app, KeyCode::Char('p'));
        type_str(&mut app, "hello");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.jobs.len(), 1);
        // Let the runner task report the spawn failure.
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(200)).await });
        app.drain();
        let job = app.current_job().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert!(job.log.back().unwrap().contains("spawn failed"));
        assert_eq!(app.store.job_status(job.job_id).unwrap(), JobStatus::Failed);
    }

    #[test]
    fn drain_applies_result_and_persists() {
        let (mut app, _rt, job) = app_with_job();
        app.inject(job, RunnerMsg::Event(result_event(0.5, false)));
        app.drain();
        let view = app.current_job().unwrap();
        assert_eq!(view.cost_usd, 0.5);
        assert_eq!(view.status, JobStatus::Done);
        assert_eq!(app.store.job_status(job).unwrap(), JobStatus::Done);
        assert_eq!(app.store.job_result(job).unwrap().unwrap().cost_usd, 0.5);
    }

    #[test]
    fn exited_without_result_marks_failed() {
        let (mut app, _rt, job) = app_with_job();
        app.inject(
            job,
            RunnerMsg::Exited {
                code: Some(1),
                stderr_tail: "x".into(),
            },
        );
        app.drain();
        assert_eq!(app.current_job().unwrap().status, JobStatus::Failed);
        let r = app.store.job_result(job).unwrap().unwrap();
        assert_eq!(r.error.as_deref(), Some("claude exited with code 1: x"));
    }

    #[test]
    fn exit_after_result_keeps_done() {
        let (mut app, _rt, job) = app_with_job();
        app.inject(job, RunnerMsg::Event(result_event(0.1, false)));
        app.inject(
            job,
            RunnerMsg::Exited {
                code: Some(0),
                stderr_tail: String::new(),
            },
        );
        app.drain();
        assert_eq!(app.current_job().unwrap().status, JobStatus::Done);
    }

    #[test]
    fn subagent_gets_own_lane() {
        let (mut app, _rt, job) = app_with_job();
        app.inject(
            job,
            RunnerMsg::Event(Event::ToolUse {
                id: "toolu_A".into(),
                name: "Task".into(),
                summary: "explorer: map code".into(),
                parent: None,
            }),
        );
        app.inject(
            job,
            RunnerMsg::Event(Event::Text {
                text: "looking".into(),
                parent: Some("toolu_A".into()),
            }),
        );
        app.drain();
        let labels: Vec<_> = app
            .current_job()
            .unwrap()
            .lanes
            .iter()
            .map(|l| l.label.as_str())
            .collect();
        assert_eq!(labels, ["lead", "explorer"]);
        assert!(
            app.current_job()
                .unwrap()
                .log
                .back()
                .unwrap()
                .contains("explorer")
        );
    }
}
