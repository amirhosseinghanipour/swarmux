//! "Let swarmux route" and the JEV consent question, as the app handles them.

use std::collections::HashMap;

use swarmux_application::{AppError, AppResult, Routed};
use swarmux_domain::{JobId, PromptId};

use crate::app::App;
use crate::prompts::PendingDispatch;

/// What to send once the router has picked a session.
pub enum RoutePayload {
    Plain(String),
    Template {
        prompt_id: PromptId,
        values: HashMap<String, String>,
    },
}

impl App {
    /// Starts routing a typed prompt; it is queued once a session is picked.
    pub(crate) fn route_prompt(&mut self, prompt: &str) -> AppResult<()> {
        let fallback = self
            .selected_session()
            .map(|s| s.id)
            .ok_or(AppError::SessionNotFound)?;
        let ticket = self.routing.start(prompt, fallback)?;
        self.routes
            .insert(ticket, RoutePayload::Plain(prompt.to_string()));
        self.notice = Some("routing…".into());
        Ok(())
    }

    /// Starts routing a template whose variables are all known.
    pub(crate) fn route_template(&mut self, pending: PendingDispatch) -> AppResult<()> {
        let text = self
            .prompt_service
            .routing_text(pending.prompt_id, &pending.values)?;
        let ticket = self.routing.start(&text, pending.session_id)?;
        self.routes.insert(
            ticket,
            RoutePayload::Template {
                prompt_id: pending.prompt_id,
                values: pending.values,
            },
        );
        self.notice = Some("routing…".into());
        Ok(())
    }

    /// Queues prompts whose session was picked. Returns errors to show.
    pub(crate) fn pump_routes(&mut self) -> Vec<AppError> {
        let report = self.routing.pump();
        let mut errors = report.errors;
        for routed in report.routed {
            let Some(payload) = self.routes.remove(&routed.ticket) else {
                continue;
            };
            match self.send_routed(&routed, payload) {
                Ok(job) => {
                    let line = routed.summary();
                    self.jobs.note(job, "route", &line);
                    // Choosing the session was the job's first decision.
                    self.jobs.observe_decision(job);
                    self.dashboard.follow(job, None);
                    self.notice = Some(line);
                    self.refresh_sessions(Some(&routed.session));
                }
                Err(e) => errors.push(e),
            }
        }
        errors
    }

    fn send_routed(&mut self, routed: &Routed, payload: RoutePayload) -> AppResult<JobId> {
        let session = routed.session.id;
        match payload {
            RoutePayload::Plain(prompt) => self.jobs.dispatch(session, &prompt),
            RoutePayload::Template { prompt_id, values } => {
                let rendered = self.prompt_service.render(prompt_id, session, &values)?;
                self.jobs.dispatch_template(session, &rendered)
            }
        }
    }

    /// The user's answer to the JEV consent question.
    pub(crate) fn answer_consent(&mut self, yes: bool) -> AppResult<()> {
        let Some(jev) = self.guard.jev().cloned() else {
            return Ok(());
        };
        if yes {
            jev.grant()?;
            self.notice = Some(format!(
                "JEV ({}) is on: unmatched tool calls and routing ask it first",
                jev.provider()
            ));
        } else {
            jev.decline();
            self.notice = Some("JEV stays off for this run".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use swarmux_application::ports::AdviceAnswer;
    use swarmux_application::testing::{advice, picks};
    use swarmux_application::{JevState, Source};

    use crate::app::tests_support::{test_app, test_app_with_jev};
    use crate::app::{InputPurpose, Mode, Screen};
    use crate::ui::render_to_string;

    #[test]
    fn shift_p_routes_then_queues_on_the_pick() {
        let mut t = test_app();
        t.with_session("api");
        t.clock.advance(1);
        t.with_session("web");
        t.keys([KeyCode::Down]);
        let selected = t.app.selected_session().unwrap().name.clone();
        assert_eq!(selected, "api");
        t.keys([KeyCode::Char('P')]);
        assert!(matches!(
            t.app.mode,
            Mode::Input(InputPurpose::RoutedPrompt, _)
        ));
        let out = render_to_string(&t.app, 120, 8);
        assert!(out.contains("swarmux picks the session"), "{out}");
        t.keys("fix the css".chars().map(KeyCode::Char));
        t.keys([KeyCode::Enter]);
        assert_eq!(t.runner.started(), 0, "waits for the router");
        assert_eq!(t.app.notice.as_deref(), Some("routing…"));
        assert!(render_to_string(&t.app, 120, 8).contains("routing 1"));

        t.router
            .reply(0, picks("web", Some("haiku"), "front-end work"));
        t.app.update();
        assert_eq!(t.runner.started(), 1);
        assert_eq!(t.runner.request(0).prompt, "fix the css");
        assert_eq!(t.app.selected_session().unwrap().name, "web");
        let notice = t.app.notice.clone().unwrap();
        assert_eq!(
            notice,
            "routed → web (haiku, suggests haiku, $0.0002): front-end work"
        );
        let job = t.app.current_job().unwrap();
        assert!(
            job.log
                .iter()
                .any(|l| l.contains("swarmux") && l.contains("routed → web"))
        );
        let out = render_to_string(&t.app, 140, 30);
        assert!(out.contains("routed → web"), "{out}");
    }

    #[test]
    fn a_failed_route_keeps_the_selected_session() {
        let mut t = test_app();
        t.with_session("api");
        t.with_session("web");
        t.keys([KeyCode::Char('P')]);
        t.keys("x".chars().map(KeyCode::Char));
        t.keys([KeyCode::Enter]);
        t.router.reply(0, picks("nope", None, "?"));
        t.app.update();
        assert_eq!(t.runner.started(), 1);
        assert_eq!(t.app.current_job().unwrap().session_name, "web");
        assert!(t.app.notice.as_deref().unwrap().contains("routing failed"));
    }

    #[test]
    fn shift_d_routes_a_template() {
        let mut t = test_app();
        t.with_session("api");
        t.with_session("web");
        t.add_prompt("fix", "fix {{file}}", &[]);
        t.keys([KeyCode::Char('3'), KeyCode::Char('D')]);
        t.keys("a.css".chars().map(KeyCode::Char));
        t.keys([KeyCode::Enter]);
        assert_eq!(t.app.screen, Screen::Dashboard);
        assert_eq!(t.router.requests()[0].prompt, "fix a.css");
        t.router.reply(0, picks("api", None, "backend"));
        t.app.update();
        assert_eq!(t.runner.request(0).prompt, "fix a.css");
        let job = t.app.current_job().unwrap().job_id;
        assert!(t.store.prompt_of(job).is_some(), "linked to its template");
    }

    #[test]
    fn consent_overlay_asks_once_and_y_turns_jev_on() {
        let mut t = test_app_with_jev(false);
        t.app.screen = Screen::Dashboard;
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("fake.example"), "{out}");
        assert!(out.contains("Never file contents"), "{out}");
        t.keys([KeyCode::Char('y')]);
        let jev = t.app.guard.jev().unwrap().clone();
        assert_eq!(jev.state(), JevState::On);
        let out = render_to_string(&t.app, 160, 48);
        assert!(!out.contains("Never file contents"), "{out}");
        assert!(out.contains("JEV typesafe · threshold 0.85"), "{out}");
    }

    #[test]
    fn n_keeps_jev_off_and_nothing_is_sent() {
        let mut t = test_app_with_jev(false);
        t.app.screen = Screen::Dashboard;
        t.keys([KeyCode::Char('n')]);
        assert_eq!(t.app.guard.jev().unwrap().state(), JevState::Declined);
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("JEV typesafe off for this run"), "{out}");
        t.inbox.push(None, "Bash", "make");
        t.app.update();
        assert!(t.advisor.as_ref().unwrap().requests().is_empty());
        assert_eq!(t.app.guard.pending_count(), 1);
    }

    #[test]
    fn jev_decisions_show_with_confidence_bars() {
        let mut t = test_app_with_jev(true);
        t.with_session("api");
        t.with_session("web");
        let advisor = t.advisor.clone().unwrap();
        t.inbox.push(None, "Bash", "make deploy");
        t.app.update();
        advisor.reply(0, advice(AdviceAnswer::Allow, 0.91));
        t.keys([KeyCode::Char('P')]);
        t.keys("deploy".chars().map(KeyCode::Char));
        t.keys([KeyCode::Enter]);
        advisor.reply(1, advice(AdviceAnswer::Session("api".into()), 0.93));
        t.app.update();
        assert!(t.router.requests().is_empty(), "confident jev skips haiku");
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("GUARD · fork layer"), "{out}");
        assert!(
            out.contains("Bash make deploy █████████░ 0.91 → allow"),
            "{out}"
        );
        assert!(
            out.contains("which session ███████████▏ 0.93 → api"),
            "{out}"
        );
        let rows = t.decisions.rows();
        assert!(
            rows.iter()
                .any(|r| r.source == Source::Jev && r.confidence == Some(0.93))
        );
    }

    #[test]
    fn jev_off_panel_explains_how_to_turn_it_on() {
        let mut t = test_app();
        t.app.screen = Screen::Dashboard;
        let out = render_to_string(&t.app, 160, 48);
        assert!(
            out.contains("JEV off · bring your own key: swarmux jev set-key typesafe"),
            "{out}"
        );
    }

    #[test]
    fn without_a_key_no_consent_is_asked() {
        let mut t = test_app_with_jev(false);
        t.app.screen = Screen::Dashboard;
        t.advisor.as_ref().unwrap().set_no_key(true);
        let out = render_to_string(&t.app, 160, 48);
        assert!(!out.contains("Never file contents"), "{out}");
        assert!(out.contains("no key found"), "{out}");
        t.with_session("a");
        assert_eq!(
            t.app.sessions.len(),
            1,
            "n wasn't taken as a consent answer"
        );
    }
}
