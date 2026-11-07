use std::path::PathBuf;
use std::rc::Rc;

use swarmux_domain::{DEFAULT_MODEL, Session, SessionId, Workspace};

use crate::ports::{Clock, SessionRepository};
use crate::{AppError, AppResult};

/// Use cases for sessions.
pub struct SessionService {
    repo: Rc<dyn SessionRepository>,
    clock: Rc<dyn Clock>,
    default_model: String,
}

impl SessionService {
    pub fn new(repo: Rc<dyn SessionRepository>, clock: Rc<dyn Clock>) -> SessionService {
        SessionService {
            repo,
            clock,
            default_model: DEFAULT_MODEL.to_string(),
        }
    }

    /// Model for sessions created from now on.
    pub fn with_default_model(mut self, model: &str) -> SessionService {
        self.default_model = model.to_string();
        self
    }

    pub fn create(&self, name: &str, cwd: PathBuf) -> AppResult<Session> {
        let session = Session::new(
            SessionId::random(),
            name,
            cwd,
            &self.default_model,
            self.clock.now(),
        )?;
        self.repo.insert(&session)?;
        Ok(session)
    }

    /// A session on `workspace`: it works from the workspace's root and
    /// its jobs see every repository of it.
    pub fn create_in_workspace(&self, name: &str, workspace: &Workspace) -> AppResult<Session> {
        let mut session = Session::new(
            SessionId::random(),
            name,
            workspace.root.clone(),
            &self.default_model,
            self.clock.now(),
        )?;
        session.workspace = Some(workspace.id);
        self.repo.insert(&session)?;
        Ok(session)
    }

    pub fn list(&self) -> AppResult<Vec<Session>> {
        self.repo.list_active()
    }

    /// Sets worktree mode for the session's next jobs: `Some(true)` always,
    /// `Some(false)` never, `None` when its folder is a git repository.
    pub fn set_worktree(&self, id: SessionId, worktree: Option<bool>) -> AppResult<()> {
        self.repo.get(id)?.ok_or(AppError::SessionNotFound)?;
        self.repo.set_worktree(id, worktree)
    }

    /// Turns lean mode on or off for the session's next jobs. Returns the new value.
    pub fn toggle_lean(&self, id: SessionId) -> AppResult<bool> {
        let session = self.repo.get(id)?.ok_or(AppError::SessionNotFound)?;
        self.repo.set_lean(id, !session.lean)?;
        Ok(!session.lean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FixedClock, InMemoryStore};
    use swarmux_domain::DomainError;

    fn service() -> (SessionService, Rc<FixedClock>) {
        let clock = Rc::new(FixedClock::new(1_000));
        let store = Rc::new(InMemoryStore::default());
        (SessionService::new(store, clock.clone()), clock)
    }

    #[test]
    fn create_lists_newest_first() {
        let (svc, clock) = service();
        svc.create("a", "/w".into()).unwrap();
        clock.advance(10);
        svc.create("b", "/w".into()).unwrap();
        let names: Vec<_> = svc.list().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["b", "a"]);
    }

    #[test]
    fn a_workspace_session_works_from_its_root() {
        let (svc, _) = service();
        let w = Workspace::new(
            swarmux_domain::WorkspaceId::random(),
            "Product",
            "/p".into(),
            0,
        )
        .unwrap();
        let s = svc.create_in_workspace("product", &w).unwrap();
        assert_eq!((s.workspace, s.cwd.clone()), (Some(w.id), "/p".into()));
        assert_eq!(svc.list().unwrap(), [s]);
    }

    #[test]
    fn new_sessions_use_the_default_model() {
        let (svc, _) = service();
        let svc = svc.with_default_model("opus");
        assert_eq!(svc.create("a", "/w".into()).unwrap().model, "opus");
    }

    #[test]
    fn toggle_lean_flips_and_persists() {
        let (svc, _) = service();
        let s = svc.create("a", "/w".into()).unwrap();
        assert!(svc.toggle_lean(s.id).unwrap());
        assert!(svc.list().unwrap()[0].lean);
        assert!(!svc.toggle_lean(s.id).unwrap());
        assert!(matches!(
            svc.toggle_lean(SessionId::random()),
            Err(crate::AppError::SessionNotFound)
        ));
    }

    #[test]
    fn invalid_name_is_rejected() {
        let (svc, _) = service();
        assert!(matches!(
            svc.create("  ", "/w".into()),
            Err(crate::AppError::Domain(DomainError::EmptySessionName))
        ));
    }
}
