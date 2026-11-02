use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::{
    PromptDraft, PromptId, PromptTemplate, SessionId, iso_date, render, variables,
};

use crate::ports::{Clock, PromptRepository, SessionRepository, Workspace};
use crate::{AppError, AppResult};

/// Variables swarmux fills in itself when a template is dispatched.
pub const BUILTIN_VARIABLES: [&str; 2] = ["date", "git_diff"];

/// A template rendered for one session, ready for [`crate::JobService::dispatch_template`].
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedPrompt {
    prompt_id: PromptId,
    text: String,
}

impl RenderedPrompt {
    pub fn prompt_id(&self) -> PromptId {
        self.prompt_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Use cases for the prompt library.
pub struct PromptService {
    prompts: Rc<dyn PromptRepository>,
    sessions: Rc<dyn SessionRepository>,
    workspace: Rc<dyn Workspace>,
    clock: Rc<dyn Clock>,
}

impl PromptService {
    pub fn new(
        prompts: Rc<dyn PromptRepository>,
        sessions: Rc<dyn SessionRepository>,
        workspace: Rc<dyn Workspace>,
        clock: Rc<dyn Clock>,
    ) -> PromptService {
        PromptService {
            prompts,
            sessions,
            workspace,
            clock,
        }
    }

    pub fn create(&self, name: &str, body: &str, tags: Vec<String>) -> AppResult<PromptId> {
        let draft = PromptDraft::new(name, body, tags)?;
        self.prompts.insert(&draft, self.clock.now())
    }

    pub fn update(&self, id: PromptId, name: &str, body: &str, tags: Vec<String>) -> AppResult<()> {
        let draft = PromptDraft::new(name, body, tags)?;
        self.prompts.update(id, &draft, self.clock.now())
    }

    pub fn delete(&self, id: PromptId) -> AppResult<()> {
        self.prompts.delete(id)
    }

    /// Most recently updated first.
    pub fn list(&self) -> AppResult<Vec<PromptTemplate>> {
        self.prompts.list()
    }

    pub fn get(&self, id: PromptId) -> AppResult<PromptTemplate> {
        self.prompts.get(id)?.ok_or(AppError::PromptNotFound)
    }

    /// Variables in `body` the user has to supply (built-ins excluded).
    pub fn user_variables(body: &str) -> Vec<String> {
        variables(body)
            .into_iter()
            .filter(|v| !BUILTIN_VARIABLES.contains(&v.as_str()))
            .collect()
    }

    /// The template with the user's values, for routing before a session is
    /// chosen: `{{git_diff}}` stays a placeholder, since it depends on the session.
    pub fn routing_text(
        &self,
        prompt_id: PromptId,
        user_values: &HashMap<String, String>,
    ) -> AppResult<String> {
        let prompt = self.get(prompt_id)?;
        let mut values = user_values.clone();
        values.insert("date".into(), iso_date(self.clock.now()));
        values.insert("git_diff".into(), "(the session's git diff)".into());
        Ok(render(&prompt.body, &values)?)
    }

    /// Fills the template for `session_id`: built-ins first, then `user_values`.
    pub fn render(
        &self,
        prompt_id: PromptId,
        session_id: SessionId,
        user_values: &HashMap<String, String>,
    ) -> AppResult<RenderedPrompt> {
        let prompt = self.get(prompt_id)?;
        let session = self
            .sessions
            .get(session_id)?
            .ok_or(AppError::SessionNotFound)?;
        let used = variables(&prompt.body);
        let mut values = user_values.clone();
        if used.iter().any(|v| v == "date") {
            values.insert("date".into(), iso_date(self.clock.now()));
        }
        if used.iter().any(|v| v == "git_diff") {
            values.insert("git_diff".into(), self.workspace.git_diff(&session.cwd)?);
        }
        Ok(RenderedPrompt {
            prompt_id,
            text: render(&prompt.body, &values)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeWorkspace, FixedClock, InMemoryStore};
    use swarmux_domain::{DomainError, Session};

    struct Fixture {
        svc: PromptService,
        clock: Rc<FixedClock>,
        workspace: Rc<FakeWorkspace>,
        session: Session,
    }

    /// 2026-11-02T10:00:00Z
    const NOV_2: i64 = 1_793_613_600_000;

    fn fixture() -> Fixture {
        let store = Rc::new(InMemoryStore::default());
        let clock = Rc::new(FixedClock::new(NOV_2));
        let workspace = Rc::new(FakeWorkspace::with_diff("+added line"));
        let session = Session::new(SessionId::random(), "demo", "/w".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(store.as_ref(), &session).unwrap();
        let svc = PromptService::new(store.clone(), store, workspace.clone(), clock.clone());
        Fixture {
            svc,
            clock,
            workspace,
            session,
        }
    }

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn create_update_delete_round_trip() {
        let f = fixture();
        let a = f.svc.create("a", "body a", vec!["x".into()]).unwrap();
        f.clock.advance(10);
        let b = f.svc.create("b", "body b", vec![]).unwrap();
        let names = |svc: &PromptService| -> Vec<String> {
            svc.list().unwrap().into_iter().map(|p| p.name).collect()
        };
        assert_eq!(names(&f.svc), ["b", "a"]);

        f.clock.advance(10);
        f.svc.update(a, " a2 ", "new", vec![]).unwrap();
        assert_eq!(names(&f.svc), ["a2", "b"]);
        let got = f.svc.get(a).unwrap();
        assert_eq!((got.body.as_str(), got.created_at), ("new", NOV_2));

        f.svc.delete(b).unwrap();
        assert_eq!(names(&f.svc), ["a2"]);
        assert!(matches!(f.svc.get(b), Err(AppError::PromptNotFound)));
    }

    #[test]
    fn invalid_drafts_are_rejected() {
        let f = fixture();
        assert!(matches!(
            f.svc.create(" ", "b", vec![]),
            Err(AppError::Domain(DomainError::EmptyPromptName))
        ));
        let id = f.svc.create("n", "b", vec![]).unwrap();
        assert!(matches!(
            f.svc.update(id, "n", "  ", vec![]),
            Err(AppError::Domain(DomainError::EmptyPromptBody))
        ));
        assert!(matches!(
            f.svc.update(PromptId(99), "n", "b", vec![]),
            Err(AppError::PromptNotFound)
        ));
    }

    #[test]
    fn user_variables_skip_builtins() {
        let vars =
            PromptService::user_variables("{{date}} {{file}} {{git_diff}} {{goal}} {{file}}");
        assert_eq!(vars, ["file", "goal"]);
    }

    #[test]
    fn render_fills_builtins_and_user_values() {
        let f = fixture();
        let id = f
            .svc
            .create("r", "on {{date}} review {{file}}:\n{{ git_diff }}", vec![])
            .unwrap();
        let out = f
            .svc
            .render(id, f.session.id, &values(&[("file", "main.rs")]))
            .unwrap();
        assert_eq!(out.text(), "on 2026-11-02 review main.rs:\n+added line");
        assert_eq!(out.prompt_id(), id);
        assert_eq!(f.workspace.calls(), std::slice::from_ref(&f.session.cwd));
    }

    #[test]
    fn routing_text_needs_no_session_or_git() {
        let f = fixture();
        let id = f
            .svc
            .create(
                "r",
                "fix {{file}} on {{date}}
{{git_diff}}",
                vec![],
            )
            .unwrap();
        let text = f
            .svc
            .routing_text(id, &values(&[("file", "a.rs")]))
            .unwrap();
        assert_eq!(
            text,
            "fix a.rs on 2026-11-02
(the session's git diff)"
        );
        assert!(f.workspace.calls().is_empty());
    }

    #[test]
    fn git_is_only_asked_when_needed() {
        let f = fixture();
        let id = f.svc.create("r", "plain {{date}}", vec![]).unwrap();
        f.svc.render(id, f.session.id, &values(&[])).unwrap();
        assert!(f.workspace.calls().is_empty());
    }

    #[test]
    fn render_reports_missing_values_and_unknown_ids() {
        let f = fixture();
        let id = f.svc.create("r", "{{goal}}", vec![]).unwrap();
        assert!(matches!(
            f.svc.render(id, f.session.id, &values(&[])),
            Err(AppError::Domain(DomainError::MissingVariable(v))) if v == "goal"
        ));
        assert!(matches!(
            f.svc.render(PromptId(42), f.session.id, &values(&[])),
            Err(AppError::PromptNotFound)
        ));
        assert!(matches!(
            f.svc
                .render(id, SessionId::random(), &values(&[("goal", "g")])),
            Err(AppError::SessionNotFound)
        ));
    }
}
