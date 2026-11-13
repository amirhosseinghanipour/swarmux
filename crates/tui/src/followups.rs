//! The Follow-ups screen: what agents left behind, as decision cards, one
//! key per answer; the approved ones waiting to become missions; and the
//! standing orders that decide without asking.

use std::rc::Rc;

use swarmux_application::{FollowUpService, Intent, Reply};
use swarmux_domain::followup::{FollowUp, OwnerBrief, StandingOrder};
use swarmux_domain::keys::{Action, actions};

use crate::app::{App, Screen};
use crate::missions::Composer;
use crate::screens::Handled;
use crate::screens::followups::{APPROVE, DECLINE, DRAFT_MISSION, LATER, OPEN_JOB};
use crate::ui::input::Input;

/// Frames between reads of the database while the screen is open.
const REFRESH_FRAMES: u64 = 30;
/// A day, for the brief preview.
const DAY_MS: i64 = 86_400_000;

/// The screen's selection and what it shows.
#[derive(Default)]
pub struct FollowScreen {
    /// The selected row: cards first, then approved intents.
    pub selected: usize,
    pub cards: Vec<FollowUp>,
    pub intents: Vec<Intent>,
    /// What the next brief would say, for its headline.
    pub brief: Option<OwnerBrief>,
    pub loaded_at: u64,
}

impl FollowScreen {
    pub fn rows(&self) -> usize {
        self.cards.len() + self.intents.len()
    }
}

impl App {
    /// Collects follow-ups, applies the owner's standing orders, and brings
    /// the brief and the urgent items.
    pub fn with_followups(mut self, service: Option<Rc<FollowUpService>>) -> App {
        self.engine.followups = service;
        self
    }

    /// Whether follow-through is on.
    pub fn followups_on(&self) -> bool {
        self.engine.followups.is_some()
    }

    /// The standing orders, for the screen.
    pub fn standing_orders(&self) -> Vec<StandingOrder> {
        self.engine
            .followups
            .as_ref()
            .map(|s| s.orders().to_vec())
            .unwrap_or_default()
    }

    /// Urgent follow-ups, for Mission control's inbox.
    pub(crate) fn followup_items(&self) -> Vec<swarmux_domain::AttentionItem> {
        self.engine
            .followups
            .as_ref()
            .map(|s| s.urgent_items())
            .unwrap_or_default()
    }

    /// Takes the pump's results: errors, and the briefs that came due go
    /// out as one short notification each (counts only, never card text).
    pub(crate) fn pump_followups(&mut self) {
        let pumped = self
            .engine
            .pump_followups(&self.mission.facts, &self.mission.gates);
        if let Some(e) = pumped.errors.into_iter().last() {
            self.error = Some(e.to_string());
        }
        for brief in &pumped.briefs {
            self.announce_brief(brief);
        }
        if pumped.changed {
            self.refresh_followups();
        }
    }

    /// While the screen shows, re-reads the database now and then.
    pub(crate) fn refresh_followups_if_stale(&mut self) {
        if self.frame.saturating_sub(self.followup_screen.loaded_at) >= REFRESH_FRAMES {
            self.refresh_followups();
        }
    }

    fn announce_brief(&mut self, brief: &OwnerBrief) {
        let now = self.now();
        let n = brief.cards.len() + brief.more;
        let body = if n == 0 {
            "nothing needs you".to_string()
        } else {
            format!("{n} follow-up{} need you", if n == 1 { "" } else { "s" })
        };
        let note = swarmux_application::Notification {
            id: swarmux_domain::AttentionId::new(format!("brief:{now}")),
            priority: swarmux_domain::Priority::P2,
            kind: swarmux_domain::AttentionKind::FollowUp,
            title: "swarmux brief".into(),
            body,
        };
        if let Some(notifier) = &mut self.notifier
            && let Some(e) = notifier.announce(&note).into_iter().last()
        {
            self.error = Some(e.to_string());
        }
        self.show_toast(brief.headline());
    }

    /// Re-reads the cards, the intents and the brief preview.
    pub(crate) fn refresh_followups(&mut self) {
        self.followup_screen.loaded_at = self.frame;
        let Some(service) = self.engine.followups.clone() else {
            return;
        };
        let now = self.now();
        let mut cards = service.waiting().unwrap_or_default();
        cards.sort_by_key(|f| (f.kind.as_str() != "risk", f.created, f.id));
        self.followup_screen.cards = cards;
        self.followup_screen.intents = service
            .intents()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.mission.is_none())
            .collect();
        self.followup_screen.brief = service
            .compose_brief(
                &self.mission.facts,
                &self.mission.gates,
                now - DAY_MS,
                false,
            )
            .ok();
        let rows = self.followup_screen.rows();
        self.followup_screen.selected = self.followup_screen.selected.min(rows.saturating_sub(1));
    }

    fn selected_card(&self) -> Option<&FollowUp> {
        self.followup_screen
            .cards
            .get(self.followup_screen.selected)
    }

    fn selected_intent(&self) -> Option<&Intent> {
        let s = &self.followup_screen;
        s.selected
            .checked_sub(s.cards.len())
            .and_then(|i| s.intents.get(i))
    }

    /// The Follow-ups actions: select, approve, decline, later, open the
    /// job it came from, draft a mission from it.
    pub(crate) fn followups_action(&mut self, action: Action) -> Handled {
        let rows = self.followup_screen.rows();
        match action {
            actions::LIST_PREV => {
                self.followup_screen.selected = self.followup_screen.selected.saturating_sub(1);
            }
            actions::LIST_NEXT => {
                self.followup_screen.selected =
                    (self.followup_screen.selected + 1).min(rows.saturating_sub(1));
            }
            APPROVE => self.answer_card(Reply::Approve),
            DECLINE => self.answer_card(Reply::Decline),
            LATER => self.answer_card(Reply::Defer),
            OPEN_JOB => {
                let job = self
                    .selected_card()
                    .and_then(FollowUp::first_job)
                    .or_else(|| {
                        let i = self.selected_intent()?;
                        self.engine
                            .followups
                            .as_ref()?
                            .get(i.followup)
                            .ok()
                            .flatten()?
                            .first_job()
                    });
                match job {
                    Some(job) => {
                        let r = self.view(job);
                        self.show(r);
                    }
                    None => self.notice = Some("no job to open for this one".into()),
                }
            }
            DRAFT_MISSION => self.draft_mission(),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn answer_card(&mut self, reply: Reply) {
        let Some(card) = self.selected_card().cloned() else {
            return;
        };
        let Some(service) = self.engine.followups.clone() else {
            return;
        };
        match service.reply(card.id, reply) {
            Ok(_) => {
                self.notice = Some(match reply {
                    Reply::Approve => format!(
                        "#{} approved: press {} to draft a mission",
                        card.id,
                        crate::hints::key_or_name(self, crate::screens::followups::DRAFT_MISSION)
                    ),
                    Reply::Decline => format!("#{} declined", card.id),
                    Reply::Defer => format!("#{} comes back later", card.id),
                });
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.refresh_followups();
    }

    /// Fills the mission composer with the selected follow-up: an approved
    /// one waiting as an intent, or a card (approved on the way).
    fn draft_mission(&mut self) {
        if self.engine.missions.is_none() {
            self.notice = Some("missions are not set up in this run".into());
            return;
        }
        if self.sessions.is_empty() {
            self.error = Some(self.need_session());
            return;
        }
        let Some(service) = self.engine.followups.clone() else {
            return;
        };
        let intent = if let Some(i) = self.selected_intent() {
            i.clone()
        } else if let Some(card) = self.selected_card().cloned() {
            match service.reply(card.id, Reply::Approve) {
                Ok(Some(i)) => i,
                Ok(None) => {
                    self.notice = Some("a question is answered on the Board (key 7)".into());
                    return;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                    return;
                }
            }
        } else {
            return;
        };
        let mut composer = Composer::blank();
        composer.inputs[0] = Input::with_text(&title_of(&intent.text));
        composer.inputs[1] = Input::with_text(&intent.text);
        composer.inputs[3] = Input::with_text(&intent.paths.join(", "));
        self.missions_screen.composer = Some(composer);
        self.screen = Screen::Missions;
        // The mission is not saved yet; the backlog of a later milestone
        // links intents to missions. Until then a drafted intent is set aside.
        let _ = service.intent_became(intent.id, "draft");
        self.notice = Some("mission drafted from the follow-up: check it, then save".into());
        self.refresh_followups();
    }
}

fn title_of(text: &str) -> String {
    let t: String = text.chars().take(60).collect();
    t.trim().to_string()
}

#[cfg(test)]
mod tests;
