use std::path::PathBuf;
use std::rc::Rc;

use swarmux_domain::{DEFAULT_MODEL, Session, SessionId};

use crate::AppResult;
use crate::ports::{Clock, SessionRepository};

/// Use cases for sessions.
pub struct SessionService {
    repo: Rc<dyn SessionRepository>,
    clock: Rc<dyn Clock>,
}

impl SessionService {
    pub fn new(repo: Rc<dyn SessionRepository>, clock: Rc<dyn Clock>) -> SessionService {
        SessionService { repo, clock }
    }

    pub fn create(&self, name: &str, cwd: PathBuf) -> AppResult<Session> {
        let session = Session::new(
            SessionId::random(),
            name,
            cwd,
            DEFAULT_MODEL,
            self.clock.now(),
        )?;
        self.repo.insert(&session)?;
        Ok(session)
    }

    pub fn list(&self) -> AppResult<Vec<Session>> {
        self.repo.list_active()
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
    fn invalid_name_is_rejected() {
        let (svc, _) = service();
        assert!(matches!(
            svc.create("  ", "/w".into()),
            Err(crate::AppError::Domain(DomainError::EmptySessionName))
        ));
    }
}
