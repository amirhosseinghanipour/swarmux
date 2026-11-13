//! Plan-first missions on screen: answering a proposed plan, from the
//! Missions screen and from the inbox.
//!
//! Approving starts the build with the plan in its prompt; changing asks
//! for a note and sends the plan back for a new one; cancelling drops it.

use swarmux_application::AppResult;
use swarmux_domain::AttentionKind;
use swarmux_domain::plan::{PlanRecord, PlanStatus};

use crate::app::{App, InputPurpose, Mode, Screen};
use crate::inbox::Decision;
use crate::mission::Pane;
use crate::ui::input::Input;

impl App {
    /// The mission whose proposed plan the decision keys answer: the selected
    /// card on the Missions screen, or the selected plan item of the inbox.
    pub(crate) fn plan_target(&self) -> Option<String> {
        let missions = self.engine.missions.as_ref()?;
        let slug = match self.screen {
            Screen::Missions if self.missions_screen.composer.is_none() => self
                .missions()
                .get(self.missions_screen.selected)?
                .slug
                .clone(),
            Screen::Work => crate::work::plan_slug(&self.work_selected()?)?.to_string(),
            Screen::Mission if self.mission.pane == Pane::Inbox => {
                let item = self
                    .mission
                    .selected_item()
                    .filter(|i| i.kind == AttentionKind::PlanReady)?;
                item.id.as_str().strip_prefix("plan:")?.to_string()
            }
            _ => return None,
        };
        missions
            .plan(&slug)
            .is_some_and(|r| r.status == PlanStatus::Proposed)
            .then_some(slug)
    }

    /// The mission's plan record, when it has one.
    pub fn mission_plan(&self, slug: &str) -> Option<&PlanRecord> {
        self.engine.missions.as_ref()?.plan(slug)
    }

    /// Whether the mission plans first (its own setting, else the default).
    pub fn mission_plans_first(&self, slug: &str) -> bool {
        self.engine
            .missions
            .as_ref()
            .is_some_and(|m| m.plan_first(slug))
    }

    /// A planning run is out for the mission.
    pub fn mission_planning(&self, slug: &str) -> bool {
        self.engine
            .missions
            .as_ref()
            .is_some_and(|m| m.planning(slug))
    }

    /// Yes approves, no changes the plan (and asks for a note), cancel
    /// cancels it.
    pub(crate) fn answer_plan(&mut self, answer: Decision, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.engine.missions else {
            return Ok(());
        };
        match answer {
            Decision::Yes => {
                let job = service.approve_plan(slug, &mut self.engine.jobs)?;
                self.followed_mission(slug, job)?;
                self.notice = Some(format!(
                    "plan approved: {slug} is out as job #{job} with the plan in its prompt"
                ));
            }
            Decision::No => {
                self.missions_screen.note_for = Some(slug.to_string());
                self.mode = Mode::Input(InputPurpose::PlanNote, Input::default());
            }
            _ => {
                service.cancel_plan(slug)?;
                self.notice = Some(format!("plan cancelled: {slug} waits as it was"));
            }
        }
        Ok(())
    }

    /// The note that sends the plan back: a new plan is asked for.
    pub(crate) fn submit_plan_note(&mut self, note: &str) -> AppResult<()> {
        let Some(slug) = self.missions_screen.note_for.take() else {
            return Ok(());
        };
        let Some(service) = &mut self.engine.missions else {
            return Ok(());
        };
        service.reject_plan(&slug, note)?;
        self.notice = Some(format!("{slug}: asking for a new plan"));
        Ok(())
    }

    /// `p` on the Missions screen: plan before building, or not.
    pub(crate) fn toggle_plan_first(&mut self, slug: &str) -> AppResult<()> {
        let Some(service) = &mut self.engine.missions else {
            return Ok(());
        };
        let on = !service.plan_first(slug);
        service.set_plan_first(slug, on)?;
        self.notice = Some(if on {
            format!("{slug} plans first: r asks for a plan to approve")
        } else {
            format!("{slug} builds straight away")
        });
        Ok(())
    }

    /// `⏎` on a plan item of the inbox: the Missions screen, on that mission.
    pub(crate) fn open_plan_item(&mut self) -> bool {
        if self.mission.pane != Pane::Inbox {
            return false;
        }
        let Some(item) = self
            .mission
            .selected_item()
            .filter(|i| i.kind == AttentionKind::PlanReady)
        else {
            return false;
        };
        let Some(slug) = item.id.as_str().strip_prefix("plan:").map(str::to_string) else {
            return false;
        };
        let Some(at) = self.missions().iter().position(|m| m.slug == slug) else {
            return false;
        };
        self.missions_screen.selected = at;
        self.navigate(Screen::Missions);
        true
    }
}

#[cfg(test)]
mod tests;
