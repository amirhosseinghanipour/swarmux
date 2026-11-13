//! What the Work screen's keys do: move through the queue, switch tabs and
//! run the actions of the selected outcome. Every action goes through the
//! service the old screens used; the answers to a held call and to a plan
//! are the decision keys' own flow (`y`, `n`, `a`, with the typed
//! confirmation for an irreversible call), not a copy of it.

use std::collections::HashMap;

use swarmux_application::LandReport;
use swarmux_application::console::OutcomeView;
use swarmux_domain::JobId;

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, InputPurpose, Mode, Screen};
use crate::ui::input::Input;
use crate::work::{
    ActionRow, DiffRead, Do, NoteComposer, NoteTarget, Tab, held_request, holds_of, rows_for,
};

/// Lines of a diff read.
const DIFF_LINES: usize = 400;

impl App {
    /// Builds the console snapshot when what it reads changed, and keeps
    /// the Work screen's selection on an outcome that exists.
    pub(crate) fn work_refresh(&mut self) {
        let _ = self.console_snapshot();
        self.work_settle();
    }

    pub(crate) fn work_settle(&mut self) {
        if let Some(snap) = self.console.current() {
            self.work_screen.settle(snap);
        }
    }

    /// The selected outcome, copied out so the app can be changed while it
    /// is used.
    pub(crate) fn work_selected(&self) -> Option<OutcomeView> {
        let snap = self.console.current()?;
        self.work_screen.current(snap).cloned()
    }

    pub(crate) fn work_step(&mut self, delta: isize) {
        if let Some(snap) = self.console.current() {
            self.work_screen.step(snap, delta);
        }
    }

    pub(crate) fn work_tab(&mut self, tab: Tab) {
        self.work_screen.show(tab);
        if tab == Tab::Diff {
            self.work_read_diff();
        }
    }

    /// The rows of the actions panel for the selected outcome.
    pub(crate) fn work_rows(&self) -> Vec<ActionRow> {
        let Some(snap) = self.console.current() else {
            return Vec::new();
        };
        let Some(o) = self.work_screen.current(snap) else {
            return Vec::new();
        };
        rows_for(snap, o)
    }

    /// The job an action on `o` is about: the lead's.
    fn work_job(o: &OutcomeView) -> Option<JobId> {
        o.jobs.first().map(|j| JobId(j.id))
    }

    /// Runs one of the selected outcome's actions, or says why it cannot.
    pub(crate) fn work_do(&mut self, what: Do) {
        let Some(o) = self.work_selected() else {
            return;
        };
        let rows = self.work_rows();
        let Some(row) = rows.iter().find(|r| r.what == what) else {
            self.notice = Some("that does not apply to this outcome".into());
            return;
        };
        if !row.enabled {
            self.notice = Some(format!("{}: {}", row.label, row.detail));
            return;
        }
        let result = self.work_run(what, &o);
        self.show(result);
    }

    fn work_run(&mut self, what: Do, o: &OutcomeView) -> swarmux_application::AppResult<()> {
        let action_job = |kind| {
            o.actions
                .iter()
                .find(|a| a.kind == kind)
                .and_then(|a| a.job)
                .map(JobId)
        };
        use swarmux_application::console::ActionKind as Kind;
        match what {
            Do::Kill => {
                if let Some(job) = action_job(Kind::Kill) {
                    self.engine.jobs.cancel(job)?;
                    self.notice = Some(format!("stopping job #{job}"));
                }
            }
            Do::Retry => {
                if let Some(job) = action_job(Kind::Retry) {
                    let new = self.engine.jobs.retry(job)?;
                    self.notice = Some(format!("job #{job} queued again as #{new}"));
                }
            }
            Do::Resume => {
                if let Some(job) = action_job(Kind::Resume) {
                    let new = self.engine.jobs.resume(job)?;
                    self.notice = Some(format!("job #{job} resumes as #{new}"));
                }
            }
            Do::Land => self.work_land(o)?,
            Do::Evidence => self.work_open_evidence(o)?,
            Do::SendBack => self.work_begin_note(o),
            // The decision keys carry these; the rows only show them.
            Do::AllowOnce | Do::AlwaysAllow | Do::Deny | Do::ApprovePlan | Do::ChangePlan => {}
        }
        Ok(())
    }

    /// Opens the note line for the selected outcome: to the held call when
    /// one waits (it is denied with the note as the reason), else to the
    /// agent of the job that ended (it resumes with the note). Nothing is
    /// sent until the user confirms with Enter.
    fn work_begin_note(&mut self, o: &OutcomeView) {
        let held = held_request(o);
        let (target, about) = match (held, self.console.current()) {
            (Some(request), Some(snap)) => {
                let about = holds_of(snap, o)
                    .into_iter()
                    .find(|h| h.request == Some(request))
                    .map_or_else(
                        || "a held call".to_string(),
                        |h| format!("{} {}", h.tool, h.target),
                    );
                (NoteTarget::Held(request), about)
            }
            _ => {
                let job = o
                    .actions
                    .iter()
                    .find(|a| a.kind == swarmux_application::console::ActionKind::SendBack)
                    .and_then(|a| a.job);
                match job {
                    Some(job) => (NoteTarget::Work(job), format!("job #{job}")),
                    None => {
                        self.notice = Some("send back: no job to send back".into());
                        return;
                    }
                }
            }
        };
        self.work_screen.note = Some(NoteComposer::new(target, &about, self.send_back_notes()));
        self.mode = Mode::Input(InputPurpose::SendBackNote, Input::default());
    }

    /// The keys of the note line beyond typing: Tab and Down pick the next
    /// suggested note, Shift-Tab and Up the previous one. A pick fills the
    /// line (the user edits it, or sends it as it is). Whether the key was
    /// one of them.
    pub(crate) fn work_note_key(&mut self, key: KeyEvent) -> bool {
        let delta = match key.code {
            KeyCode::Tab | KeyCode::Down => 1,
            KeyCode::BackTab | KeyCode::Up => -1,
            _ => return false,
        };
        let Some(composer) = &mut self.work_screen.note else {
            return false;
        };
        if let Some(text) = composer.cycle(delta).map(str::to_string) {
            self.mode = Mode::Input(InputPurpose::SendBackNote, Input::with_text(&text));
        }
        true
    }

    /// The user closed the note line without sending.
    pub(crate) fn work_note_cancelled(&mut self) {
        self.work_screen.note = None;
        self.notice = Some("not sent".into());
    }

    /// The user confirmed: the note goes to the agent. The note is the
    /// user's own words; nothing is added to it but the framing the agent
    /// is told it came from.
    pub(crate) fn submit_send_back(&mut self, note: &str) -> swarmux_application::AppResult<()> {
        let Some(composer) = self.work_screen.note.take() else {
            return Ok(());
        };
        let now = self.now();
        match composer.target {
            NoteTarget::Work(job) => {
                let sent = self.engine.send_back(JobId(job), note)?;
                self.notice = Some(match sent.seq {
                    Some(seq) => format!(
                        "job #{job} sent back as #{}: the gate judges its next claim · ledger #{seq}",
                        sent.job
                    ),
                    None => format!(
                        "job #{job} sent back as #{}: the gate judges its next claim",
                        sent.job
                    ),
                });
            }
            NoteTarget::Held(request) => {
                if let Some(item) = self.mission.alarms().find_request(request).cloned() {
                    self.mission.alarms_mut().attend(&item, now);
                }
                let seq = self.engine.send_back_held(request, note)?;
                self.mission.alarms_mut().answered_request(request, false);
                self.notice = Some(match seq {
                    Some(seq) => format!("call denied, your note goes with it · ledger #{seq}"),
                    None => "call denied, your note goes with it".to_string(),
                });
            }
        }
        // The new state shows at once.
        self.work_refresh();
        Ok(())
    }

    /// Lands the mission on request: every repository into the integration
    /// branch, producers first, all or none.
    fn work_land(&mut self, o: &OutcomeView) -> swarmux_application::AppResult<()> {
        let Some(slug) = o.mission.clone() else {
            return Ok(());
        };
        let facts = self.engine.jobs.facts();
        let gates: HashMap<_, _> = self
            .engine
            .gate
            .as_ref()
            .map(|g| g.states().clone())
            .unwrap_or_default();
        let Some(missions) = &mut self.engine.missions else {
            return Ok(());
        };
        let into = o
            .landing
            .as_ref()
            .map_or_else(String::new, |l| l.into.clone());
        match missions.land_on_request(&slug, &facts, &gates)? {
            LandReport::Delivered => {
                self.notice = Some(format!("{slug} landed in {into}"));
            }
            LandReport::Undone(why) => {
                self.notice = Some(format!("{slug} did not land, nothing stays landed: {why}"));
            }
            LandReport::Unsealed(why) => {
                self.notice = Some(format!("{slug} landed in {into}, but: {why}"));
            }
        }
        // The new state shows at once.
        self.work_refresh();
        Ok(())
    }

    /// The Ledger, on the outcome's job.
    fn work_open_evidence(&mut self, o: &OutcomeView) -> swarmux_application::AppResult<()> {
        let Some(job) = Self::work_job(o) else {
            return Ok(());
        };
        self.refresh_ledger();
        match self.ledger.rows.iter().position(|r| r.job_id == job) {
            Some(i) => {
                self.ledger.selected = i;
                self.load_detail();
            }
            None => {
                self.notice = Some(format!(
                    "job #{job} is not in the ledger's list: clear its search or filter"
                ));
            }
        }
        self.navigate(Screen::Ledger);
        Ok(())
    }

    /// `⏎`: the outcome's job in Focus, the full action stream and answer.
    pub(crate) fn work_open_focus(&mut self) {
        let Some(job) = self.work_selected().as_ref().and_then(Self::work_job) else {
            return;
        };
        let result = self.view(job);
        self.show(result);
    }

    /// Reads the uncommitted changes of the selected outcome's session, as
    /// Focus does for its diff.
    fn work_read_diff(&mut self) {
        let Some(o) = self.work_selected() else {
            return;
        };
        let Some(job) = o.jobs.first() else {
            self.work_screen.diff = Some(DiffRead {
                job: 0,
                files: Vec::new(),
                lines: Vec::new(),
                note: Some("no job has run yet".into()),
            });
            return;
        };
        let (id, session) = (job.id, job.session.clone());
        let cwd = self
            .sessions
            .iter()
            .find(|s| s.name == session)
            .map(|s| s.cwd.clone());
        let cwd_for_counts = cwd.clone();
        let read = match cwd {
            None => Err("its session is not known here".to_string()),
            Some(cwd) => match self.engine.jobs.working_diff(&cwd) {
                Ok(None) => Err("diffs need a git workspace".to_string()),
                Ok(Some(d)) if d.trim().is_empty() => Err("no uncommitted changes".to_string()),
                Ok(Some(d)) => Ok(d.lines().take(DIFF_LINES).map(str::to_string).collect()),
                Err(e) => Err(e.to_string()),
            },
        };
        // The same basis as the text, counted by git itself.
        let files: Vec<swarmux_application::console::DiffFileView> = cwd_for_counts
            .and_then(|cwd| self.engine.jobs.working_changes(&cwd).ok().flatten())
            .unwrap_or_default()
            .iter()
            .map(Into::into)
            .collect();
        self.work_screen.diff = Some(match read {
            Ok(lines) => DiffRead {
                job: id,
                files,
                lines,
                note: None,
            },
            Err(note) => DiffRead {
                job: id,
                files,
                lines: Vec::new(),
                note: Some(note),
            },
        });
    }
}

#[cfg(test)]
mod tests;
