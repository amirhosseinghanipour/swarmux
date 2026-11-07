//! Workspaces in the interface: sessions grouped by the workspace they
//! work on, and the line a workspace's card shows.

use swarmux_domain::{Session, Workspace, WorkspaceId};

use crate::mission::Tile;

/// What an unknown value shows as.
const UNKNOWN: &str = "—";

/// Brings each workspace's sessions together where its first one is,
/// keeping the order otherwise: plain sessions stay where they are.
pub fn group_sessions(sessions: &mut [Session]) {
    let key = |s: &Session, all: &[Session]| -> usize {
        let pos = |want: &dyn Fn(&Session) -> bool| all.iter().position(want);
        match s.workspace {
            Some(id) => pos(&|x| x.workspace == Some(id)),
            None => pos(&|x| x.id == s.id),
        }
        .unwrap_or(usize::MAX)
    };
    let snapshot = sessions.to_vec();
    sessions.sort_by_key(|s| key(s, &snapshot));
}

/// The same for the wall's tiles, by their sessions' workspaces.
pub fn group_tiles(tiles: &mut [Tile]) {
    let snapshot: Vec<Option<String>> = tiles.iter().map(|t| t.workspace.clone()).collect();
    let mut keyed: Vec<(usize, Tile)> = tiles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let at = match &t.workspace {
                Some(w) => snapshot
                    .iter()
                    .position(|x| x.as_ref() == Some(w))
                    .unwrap_or(i),
                None => i,
            };
            (at, t.clone())
        })
        .collect();
    keyed.sort_by_key(|(at, _)| *at);
    for (slot, (_, tile)) in tiles.iter_mut().zip(keyed) {
        *slot = tile;
    }
}

/// `Product · app, backend, admin`: a workspace and its repositories.
pub fn label(w: &Workspace) -> String {
    format!("{} · {}", w.name, repos(w))
}

/// `app, backend, admin`, or `—` with none.
pub fn repos(w: &Workspace) -> String {
    let names = w.repo_names();
    if names.is_empty() {
        UNKNOWN.to_string()
    } else {
        names.join(", ")
    }
}

/// The workspace `id` names among `all`.
pub fn find(all: &[Workspace], id: Option<WorkspaceId>) -> Option<&Workspace> {
    let id = id?;
    all.iter().find(|w| w.id == id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::rc::Rc;

    use crossterm::event::KeyCode;
    use swarmux_application::WorkspaceService;
    use swarmux_application::testing::{FakeScanner, FixedClock, InMemoryWorkspaces};

    use super::*;
    use crate::app::tests_support::test_app;
    use swarmux_domain::{Repo, SessionId};

    #[test]
    fn w_opens_a_session_on_a_workspace_shown_with_its_repos() {
        let mut t = test_app();
        t.with_session("plain");
        let scanner = Rc::new(FakeScanner::default());
        scanner.put("/p", "app", &["pubspec.yaml"]);
        scanner.put("/p", "backend", &["supabase/config.toml"]);
        let service = WorkspaceService::new(
            Rc::new(InMemoryWorkspaces::default()),
            scanner,
            Rc::new(FixedClock::new(5)),
        );
        service.add(Path::new("/p"), Some("Product")).unwrap();
        t.app.set_workspaces(service);
        assert_eq!(t.app.workspaces.len(), 1);

        t.keys([KeyCode::Char('6'), KeyCode::Char('w')]);
        // The first workspace is offered; Enter takes it.
        t.keys([KeyCode::Enter]);
        let s = t.app.selected_session().unwrap().clone();
        assert_eq!(s.name, "Product");
        assert_eq!(s.cwd, Path::new("/p"));
        assert_eq!(s.workspace, Some(t.app.workspaces[0].id));
        let out = crate::ui::render_to_string(&t.app, 110, 10);
        assert!(out.contains("◆ Product"), "{out}");
        assert!(out.contains("app, backend"), "{out}");
        assert!(out.contains("plain"), "{out}");

        // On the wall the session's card names the workspace and its repos.
        let tiles = t.app.wall();
        let tile = tiles.iter().find(|x| x.session == s.id).unwrap();
        assert_eq!(tile.workspace.as_deref(), Some("Product · app, backend"));
        t.keys([KeyCode::Char('1')]);
        let out = crate::ui::render_to_string(&t.app, 160, 40);
        assert!(out.contains("Product · app, backend"), "{out}");
    }

    #[test]
    fn w_without_workspaces_says_how_to_add_one() {
        let mut t = test_app();
        t.keys([KeyCode::Char('6'), KeyCode::Char('w')]);
        assert!(t.app.error.as_deref().unwrap().contains("workspace add"));
        assert_eq!(t.app.mode, crate::app::Mode::Normal);
    }

    fn session(name: &str, ws: Option<WorkspaceId>) -> Session {
        let mut s = Session::new(SessionId::random(), name, "/w".into(), "sonnet", 0).unwrap();
        s.workspace = ws;
        s
    }

    #[test]
    fn sessions_of_a_workspace_come_together_where_the_first_is() {
        let (a, b) = (WorkspaceId::random(), WorkspaceId::random());
        let mut all = vec![
            session("p1", Some(a)),
            session("plain", None),
            session("g1", Some(b)),
            session("p2", Some(a)),
            session("g2", Some(b)),
        ];
        group_sessions(&mut all);
        let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["p1", "p2", "plain", "g1", "g2"]);
    }

    #[test]
    fn a_label_names_the_repositories_or_a_dash() {
        let mut w = Workspace::new(WorkspaceId::random(), "Product", "/p".into(), 0).unwrap();
        assert_eq!(label(&w), "Product · —");
        let repo = |n: &str| Repo {
            name: n.into(),
            path: n.into(),
            git_root: "/p".into(),
            default_branch: None,
            stacks: Vec::new(),
            commands: Vec::new(),
        };
        w.set_repos(vec![repo("app"), repo("backend")], 0);
        assert_eq!(label(&w), "Product · app, backend");
        assert_eq!(find(std::slice::from_ref(&w), Some(w.id)), Some(&w));
        assert_eq!(find(std::slice::from_ref(&w), None), None);
    }
}
