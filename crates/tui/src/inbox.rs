//! The inbox's keys beyond `y`/`n`/`a`: answering a batch, shelving, focus
//! mode, offered rules and the typed confirmation of irreversible calls.

use crossterm::event::{KeyCode, KeyEvent};
use swarmux_application::{AppResult, PendingApproval, ShelveFor};
use swarmux_domain::alarm::{
    Breakpoint, Challenge, IDLE_BREAKPOINT_MS, Reversibility, reversibility,
};
use swarmux_domain::{AttentionItem, AttentionKind, Millis, Priority};

use crate::app::{App, Mode, Screen};
use crate::mission::Pane;
use crate::ui::input::{Input, InputAction};

/// Keys count as working the inbox for this long after the last one.
const ENGAGED_WITHIN_MS: Millis = 10_000;

/// An irreversible call waiting to be confirmed by naming its target.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub challenge: Challenge,
    /// What has been typed.
    pub input: Input,
    /// Choosing from the list (digits) rather than typing.
    pub pick: bool,
    /// The last try named the wrong thing.
    pub wrong: bool,
}

impl App {
    /// The inbox is where the keys go: Mission control, inbox panel.
    fn on_inbox(&self) -> bool {
        self.screen == Screen::Mission && self.mission.pane == Pane::Inbox
    }

    fn selected_ask(&self) -> Option<AttentionItem> {
        self.mission
            .selected_item()
            .filter(|i| i.kind == AttentionKind::Approval)
            .cloned()
    }

    /// Whether a held call can't be taken back.
    pub(crate) fn is_irreversible(call: &PendingApproval) -> bool {
        let r = &call.request;
        reversibility(&r.tool_name, &r.subject, &r.cwd) == Reversibility::R4
    }

    /// `y` on an irreversible call: the target has to be named first.
    pub(crate) fn begin_confirm(&mut self, call: &PendingApproval) {
        let r = &call.request;
        let challenge = Challenge::new(r.request_id, &r.tool_name, &r.subject, self.now());
        self.mode = Mode::Confirm(Confirm {
            challenge,
            input: Input::default(),
            pick: false,
            wrong: false,
        });
    }

    /// Keys while a confirmation is open. Nothing counts until it has armed.
    pub(crate) fn confirm_key(&mut self, key: KeyEvent) -> AppResult<()> {
        let now = self.now();
        let Mode::Confirm(mut c) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Ok(());
        };
        if key.code == KeyCode::Esc {
            self.notice = Some("not confirmed: the call stays held".into());
            return Ok(());
        }
        let stay = |app: &mut App, c: Confirm| app.mode = Mode::Confirm(c);
        if !c.challenge.armed(now) {
            stay(self, c);
            return Ok(());
        }
        if key.code == KeyCode::Tab {
            c.pick = !c.pick;
            c.wrong = false;
            stay(self, c);
            return Ok(());
        }
        let hit = if c.pick {
            match key.code {
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let n = usize::from(d as u8 - b'0');
                    let ok = c.challenge.accepts_choice(n, now);
                    c.wrong = !ok;
                    ok
                }
                _ => false,
            }
        } else {
            match c.input.handle(key) {
                InputAction::Submit(text) => {
                    let ok = c.challenge.accepts_text(&text, now);
                    c.wrong = !ok;
                    if !ok {
                        c.input = Input::default();
                    }
                    ok
                }
                InputAction::Cancel => {
                    self.notice = Some("not confirmed: the call stays held".into());
                    return Ok(());
                }
                InputAction::None => {
                    c.wrong = false;
                    false
                }
            }
        };
        if !hit {
            stay(self, c);
            return Ok(());
        }
        let request = c.challenge.request;
        let target = c.challenge.target.clone();
        let item = self.mission.alarms().find_request(request).cloned();
        self.engine.guard.approve(request)?;
        if let Some(item) = &item {
            self.mission
                .alarms_mut()
                .record_confirmed(item, &target, now);
            if let Some(job) = item.job {
                self.focus_state.note_human(job, now);
            }
        }
        self.mission.alarms_mut().answered_request(request, true);
        self.notice = Some(format!("approved after naming {target}"));
        self.refresh_mission();
        Ok(())
    }

    /// `A`: approves every ask of the selected one's signature.
    pub(crate) fn answer_batch(&mut self) -> AppResult<()> {
        let now = self.now();
        let Some(item) = self.selected_ask() else {
            return Ok(());
        };
        let ids = self.mission.alarms().batch_requests(&item);
        if ids.is_empty() {
            self.notice = Some(if item.priority == Priority::P1 {
                "P1 asks are answered one at a time".into()
            } else {
                "nothing to batch: A answers asks of one tool, pattern and tier".into()
            });
            return Ok(());
        }
        self.mission.attend_selected(now);
        let mut first_error = None;
        let mut done = 0;
        for id in &ids {
            match self.engine.guard.approve(*id) {
                Ok(()) => {
                    done += 1;
                    self.mission.alarms_mut().answered_request(*id, true);
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        let label = item
            .signature
            .as_ref()
            .map(swarmux_domain::alarm::Signature::label)
            .unwrap_or_default();
        self.mission
            .alarms_mut()
            .record_batch(&item, done, true, now);
        self.notice = Some(format!("approved {done} × {label}"));
        self.refresh_mission();
        first_error.map_or(Ok(()), Err)
    }

    /// `s`: asks how long to shelve the selected item.
    pub(crate) fn begin_shelve(&mut self) {
        let Some(item) = self.mission.selected_item() else {
            return;
        };
        if item.priority == Priority::P1 {
            self.notice = Some("P1 can't be shelved: answer it".into());
            return;
        }
        self.mode = Mode::Shelve;
    }

    /// The choice after `s`: `1` 15 min, `2` 60 min, `3` until its job ends.
    pub(crate) fn shelve_key(&mut self, key: KeyEvent) -> AppResult<()> {
        let how = match key.code {
            KeyCode::Char('1') => ShelveFor::Minutes(15),
            KeyCode::Char('2') => ShelveFor::Minutes(60),
            KeyCode::Char('3') => ShelveFor::JobEnds,
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.mode = Mode::Normal;
        let now = self.now();
        let Some(item) = self.mission.selected_item().cloned() else {
            return Ok(());
        };
        self.mission.attend_selected(now);
        let done = self.mission.alarms_mut().shelve(&item, how, now)?;
        self.notice = Some(if done {
            match how {
                ShelveFor::Minutes(m) => format!("shelved for {m} min; it comes back by itself"),
                ShelveFor::JobEnds => "shelved until its job ends".into(),
            }
        } else {
            "that can't be shelved".into()
        });
        self.refresh_mission();
        Ok(())
    }

    /// `u`: brings every shelved item back.
    pub(crate) fn unshelve(&mut self) {
        let now = self.now();
        let n = self.mission.alarms_mut().unshelve_all(now);
        self.notice = Some(match n {
            0 => "nothing is shelved".into(),
            n => format!("{n} shelved item{} back", if n == 1 { "" } else { "s" }),
        });
        self.refresh_mission();
    }

    /// `F`: focus mode on or off.
    pub(crate) fn toggle_focus_mode(&mut self) {
        let now = self.now();
        let on = !self.mission.alarms().focus();
        self.mission.alarms_mut().set_focus(on, now);
        self.notice = Some(if on {
            "focus on: P1 still shows at once; P2 waits for a breakpoint (2 min at most), P3 for the end of focus (45 min at most)".into()
        } else {
            "focus off".into()
        });
        self.refresh_mission();
    }

    /// `r` takes the offered rule, `R` turns it down.
    pub(crate) fn settle_rule(&mut self, take: bool) -> AppResult<()> {
        let now = self.now();
        let Some(item) = self.selected_ask() else {
            return Ok(());
        };
        let Some(offer) = self.mission.alarms().suggestion(&item) else {
            return Ok(());
        };
        if take {
            let done = self.engine.guard.allow_rule(&offer.rule)?;
            self.notice = Some(match done.not_saved {
                None => format!("rule saved: {}", done.rule),
                Some(why) => format!("rule {} for this run; not saved: {why}", done.rule),
            });
        } else {
            self.notice = Some("no rule; this ask won't be offered one again".into());
        }
        self.mission
            .alarms_mut()
            .settle_suggestion(&item, take, now);
        self.refresh_mission();
        Ok(())
    }

    /// A moment where interrupting costs little.
    pub(crate) fn note_breakpoint(&mut self, breakpoint: Breakpoint) {
        if !self.mission.alarms().focus() {
            return;
        }
        self.mission.alarms_mut().breakpoint(breakpoint);
        self.refresh_mission();
    }

    /// Once per update: how much of their time the user spends in the
    /// inbox, the queue told about it, and the idle breakpoint.
    pub(crate) fn tick_inbox(&mut self) {
        let now = self.now();
        let working = self.on_inbox()
            && !self.mission.items.is_empty()
            && now - self.last_input <= ENGAGED_WITHIN_MS;
        let engaged = working || matches!(self.mode, Mode::Confirm(_));
        self.load.record(now, engaged);
        self.engine.jobs.set_supervisor_load(self.load.share(now));
        if now - self.last_input >= IDLE_BREAKPOINT_MS && !self.idle_noted {
            self.idle_noted = true;
            self.note_breakpoint(Breakpoint::Idle);
        }
        if let Some(e) = self.mission.alarms_mut().take_error() {
            self.error = Some(e);
        }
    }
}
