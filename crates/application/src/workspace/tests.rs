use std::path::Path;

use swarmux_domain::Stack;

use super::*;
use crate::testing::{FakeScanner, FixedClock, InMemoryStore, InMemoryWorkspaces};

struct Rig {
    scanner: Rc<FakeScanner>,
    sessions: Rc<InMemoryStore>,
    service: WorkspaceService,
}

fn rig() -> Rig {
    let scanner = Rc::new(FakeScanner::default());
    scanner.put("/p", "app-flutter", &["pubspec.yaml"]);
    scanner.put("/p", "backend", &["supabase/config.toml", "package.json"]);
    let service = WorkspaceService::new(
        Rc::new(InMemoryWorkspaces::default()),
        scanner.clone(),
        Rc::new(FixedClock::new(1_000)),
    );
    Rig {
        scanner,
        sessions: Rc::new(InMemoryStore::default()),
        service,
    }
}

#[test]
fn add_finds_the_repositories_and_what_they_are_built_with() {
    let r = rig();
    let w = r.service.add(Path::new("/p"), None).unwrap();
    assert_eq!(w.name, "p");
    assert_eq!(w.repo_names(), ["app-flutter", "backend"]);
    // No pubspec text: a Dart package, not Flutter.
    assert_eq!(w.repos[0].stacks, [Stack::Dart]);
    assert_eq!(w.repos[1].stacks, [Stack::Node, Stack::Supabase]);
    assert_eq!(w.repos[1].git_root, Path::new("/p").join("backend"));
    assert_eq!(r.service.list().unwrap(), [w.clone()]);
    assert_eq!(r.service.find("P").unwrap().id, w.id);
}

#[test]
fn names_and_folders_are_not_taken_twice() {
    let r = rig();
    r.service.add(Path::new("/p"), Some("Product")).unwrap();
    let again = r.service.add(Path::new("/p"), Some("Other")).unwrap_err();
    assert!(again.to_string().contains("already"), "{again}");
    let same_name = r.service.add(Path::new("/q"), Some("product")).unwrap_err();
    assert!(
        same_name.to_string().contains("named Product"),
        "{same_name}"
    );
    assert!(matches!(
        r.service.find("nope"),
        Err(AppError::WorkspaceNotFound(_))
    ));
}

#[test]
fn rescan_reports_what_changed_and_keeps_names() {
    let r = rig();
    r.service.add(Path::new("/p"), Some("Product")).unwrap();
    r.scanner.drop_repo("/p", "app-flutter");
    r.scanner.put("/p", "admin", &["package.json"]);
    let (w, change) = r.service.rescan("product").unwrap();
    assert_eq!(change.added, ["admin"]);
    assert_eq!(change.removed, ["app-flutter"]);
    assert_eq!(w.repo_names(), ["backend", "admin"]);
    assert_eq!(r.service.find("product").unwrap(), w);
}

#[test]
fn remove_forgets_and_sessions_open_on_the_root() {
    let r = rig();
    let w = r.service.add(Path::new("/p"), None).unwrap();
    let s = r
        .service
        .open_session(&*r.sessions, &w, "product", "sonnet")
        .unwrap();
    assert_eq!(s.workspace, Some(w.id));
    assert_eq!(s.cwd, Path::new("/p"));
    assert_eq!(r.sessions.get(s.id).unwrap(), Some(s));
    assert_eq!(r.service.remove("p").unwrap().id, w.id);
    assert!(r.service.list().unwrap().is_empty());
}
