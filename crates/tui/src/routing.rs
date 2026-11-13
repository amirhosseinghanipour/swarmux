//! "Let swarmux route", as the app handles it.

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
                    self.engine.jobs.note(job, "route", &line);
                    // Choosing the session was the job's first decision.
                    self.engine.jobs.observe_decision(job);
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
            RoutePayload::Plain(prompt) => self.engine.jobs.dispatch(session, &prompt),
            RoutePayload::Template { prompt_id, values } => {
                let rendered = self.prompt_service.render(prompt_id, session, &values)?;
                self.engine.jobs.dispatch_template(session, &rendered)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use swarmux_application::ports::DecisionRepository;
    use swarmux_application::testing::picks;
    use swarmux_application::{DecisionRecord, Outcome, Source};

    use crate::app::tests_support::test_app;
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
    fn routing_goes_straight_to_haiku_and_the_guard_panel_has_no_jev() {
        let mut t = test_app();
        t.with_session("api");
        t.with_session("web");
        t.app.screen = Screen::Dashboard;
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("GUARD · fork layer"), "{out}");
        assert!(!out.to_lowercase().contains("jev"), "{out}");
        assert!(!out.contains("consent"), "{out}");
        t.keys([KeyCode::Char('P')]);
        t.keys("deploy".chars().map(KeyCode::Char));
        t.keys([KeyCode::Enter]);
        assert_eq!(t.router.requests().len(), 1, "Haiku is asked at once");
        t.router.reply(0, picks("api", None, "back-end"));
        t.app.update();
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("haiku → api"), "{out}");
        let rows = t.decisions.rows();
        assert!(
            rows.iter()
                .all(|r| r.source != Source::Jev && r.source != Source::JevError)
        );
        assert!(rows.iter().any(|r| r.source == Source::Haiku));
    }

    #[test]
    fn an_unmatched_call_waits_for_the_owner_and_y_n_answer_it_with_no_model() {
        let mut t = test_app();
        t.app.screen = Screen::Dashboard;
        t.inbox.push(None, "Bash", "make deploy");
        t.app.update();
        assert_eq!(t.app.engine.guard.pending_count(), 1);
        let out = render_to_string(&t.app, 160, 48);
        assert!(out.contains("approval needed"), "{out}");
        assert!(out.contains("Bash make deploy"), "{out}");
        assert!(!out.to_lowercase().contains("jev"), "{out}");
        t.keys([KeyCode::Char('y')]);
        assert_eq!(t.app.engine.guard.pending_count(), 0);
        let row = t.decisions.rows().pop().unwrap();
        assert_eq!((row.source, row.outcome), (Source::Guard, Outcome::Allow));
        assert_eq!(row.extra.resolver.as_deref(), Some("human-key"));
    }

    #[test]
    fn a_route_made_by_the_removed_jev_shows_as_removed() {
        let mut t = test_app();
        t.app.screen = Screen::Dashboard;
        t.decisions
            .record(&DecisionRecord {
                job_id: None,
                tool: "route".into(),
                subject: "deploy".into(),
                outcome: Outcome::Route,
                rule: Some("→ api: jev's pick".into()),
                latency_us: 40_000,
                ts: 1_000,
                source: Source::Jev,
                confidence: Some(0.93),
                extra: swarmux_application::DecisionExtra::default(),
            })
            .unwrap();
        assert_eq!(Source::Jev.label(), "jev (removed)");
        let rows = t.decisions.rows();
        let line = crate::ui::guard::decision_line(&rows[0], &t.app.look.theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("jev (removed)"), "{text}");
    }
}
