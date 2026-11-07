use std::rc::Rc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_application::KnowledgeService;
use swarmux_application::testing::{FakeAnchors, InMemoryKnowledge};
use swarmux_domain::AttentionKind;
use swarmux_domain::knowledge::{
    Action, Anchor, Draft, Enforcer, GuardRule, Home, ItemId, Kind, Status,
};

use crate::app::tests_support::{TestApp, test_app};
use crate::app::{Mode, Screen};

struct Rig {
    t: TestApp,
    knowledge: Rc<KnowledgeService>,
    files: Rc<FakeAnchors>,
}

fn home() -> Home {
    Home::repo("/w")
}

fn rig() -> Rig {
    let mut t = test_app();
    let files = Rc::new(FakeAnchors::default());
    let knowledge = Rc::new(
        KnowledgeService::new(Rc::new(InMemoryKnowledge::default()), t.clock.clone())
            .unwrap()
            .with_reader(files.clone()),
    );
    t.app.set_knowledge(Some(knowledge.clone()));
    Rig {
        t,
        knowledge,
        files,
    }
}

impl Rig {
    fn key(&mut self, code: KeyCode) {
        self.t.app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn typed(&mut self, text: &str) {
        for c in text.chars() {
            self.key(KeyCode::Char(c));
        }
        self.key(KeyCode::Enter);
    }

    fn screen(&self) -> String {
        crate::ui::render_to_string(&self.t.app, 120, 30)
    }
}

#[test]
fn the_screen_groups_items_and_shows_the_registry() {
    let mut r = rig();
    let mut shadow = Draft::new(home(), Kind::Rule, "Ask before auth changes.");
    shadow.enforcer = Some(Enforcer::Guard(GuardRule {
        tools: Vec::new(),
        paths: vec!["db/**".into()],
        pattern: None,
        action: Action::Ask,
        message: String::new(),
    }));
    r.knowledge.add(shadow).unwrap();
    r.knowledge
        .add(Draft::new(home(), Kind::Fact, "The app is a client."))
        .unwrap();
    r.key(KeyCode::Char('8'));
    assert_eq!(r.t.app.screen, Screen::Knowledge);
    let text = r.screen();
    assert!(
        text.contains("1 rules · 0 enforced · 0 failing · 0 advisory-only"),
        "{text}"
    );
    assert!(text.contains("rules in shadow"), "{text}");
    assert!(text.contains("Ask before auth changes."), "{text}");
    assert!(text.contains("facts (1)"), "{text}");

    // p promotes the selected rule out of shadow.
    r.key(KeyCode::Char('p'));
    let r1 = ItemId::parse("R1").unwrap();
    assert_eq!(r.knowledge.get(&r1).unwrap().status, Status::Enforced);
    assert!(r.t.app.notice.as_deref().unwrap().contains("enforced"));
}

#[test]
fn items_are_added_edited_and_retired_from_the_screen() {
    let mut r = rig();
    r.key(KeyCode::Char('8'));
    r.key(KeyCode::Char('a'));
    assert!(matches!(r.t.app.mode, Mode::Input(..)));
    r.typed("decision: Use row level security everywhere.");
    let d1 = ItemId::parse("D1").unwrap();
    assert_eq!(r.knowledge.get(&d1).unwrap().status, Status::Advisory);
    r.key(KeyCode::Char('e'));
    for _ in 0..10 {
        r.key(KeyCode::Backspace);
    }
    r.typed("everywhere!");
    assert!(r.knowledge.get(&d1).unwrap().text.ends_with("everywhere!"));
    r.key(KeyCode::Char('x'));
    assert_eq!(r.knowledge.get(&d1).unwrap().status, Status::Retired);
    assert!(
        r.t.app.knowledge_list().is_empty(),
        "retired items aren't listed"
    );
}

#[test]
fn stale_items_reach_the_inbox_and_are_confirmed_with_c() {
    let mut r = rig();
    r.files.set("db/auth.sql", "v1");
    let mut d = Draft::new(home(), Kind::Fact, "auth.sql defines users.");
    d.anchors = vec![Anchor::parse("db/auth.sql")];
    let f = r.knowledge.add(d).unwrap();
    r.files.set("db/auth.sql", "v2");
    r.knowledge.recheck(&home()).unwrap();
    r.t.app.update();
    assert!(
        r.t.app
            .mission
            .items
            .iter()
            .any(|i| i.kind == AttentionKind::Knowledge && i.title.contains("F1"))
    );
    r.key(KeyCode::Char('8'));
    assert!(r.screen().contains("stale: confirm"));
    r.key(KeyCode::Char('c'));
    assert_eq!(r.knowledge.get(&f.id).unwrap().status, Status::Advisory);
    r.t.app.update();
    assert!(
        r.t.app
            .mission
            .items
            .iter()
            .all(|i| i.kind != AttentionKind::Knowledge)
    );
}

#[test]
fn the_contracts_tab_lists_what_repositories_provide_and_who_consumes_it() {
    use std::path::PathBuf;
    use std::sync::Arc;
    use swarmux_application::ContractService;
    use swarmux_application::testing::{FakeSources, InMemoryContracts, NoWorkspaces};
    use swarmux_domain::{Repo, Workspace, WorkspaceId};

    let mut r = rig();
    let repo = |name: &str| Repo {
        name: name.into(),
        path: name.into(),
        git_root: PathBuf::from(format!("/shop/{name}")),
        default_branch: None,
        stacks: Vec::new(),
        commands: Vec::new(),
    };
    let mut w = Workspace::new(WorkspaceId::random(), "shop", PathBuf::from("/shop"), 1).unwrap();
    w.repos = vec![repo("backend"), repo("app")];
    let sources = Arc::new(FakeSources::default());
    sources.put(
        "/shop/backend",
        "migrations/001.sql",
        "create table orders (id uuid);",
    );
    sources.put(
        "/shop/app",
        "lib/orders.dart",
        "final r = supabase.from('orders').select('id');",
    );
    let contracts = Rc::new(ContractService::new(
        Rc::new(InMemoryContracts::default()),
        Rc::new(NoWorkspaces),
        sources,
        r.t.clock.clone(),
    ));
    contracts.rescan(&w).unwrap();
    r.t.app.workspaces = vec![w];
    r.t.app.engine.contracts = Some(contracts);

    r.key(KeyCode::Char('8'));
    assert!(!r.screen().contains("contracts ·"), "items come first");
    r.key(KeyCode::Char('t'));
    let screen = r.screen();
    assert!(screen.contains("contracts · shop"), "{screen}");
    assert!(screen.contains("table orders"), "{screen}");
    assert!(screen.contains("backend/migrations/001.sql:1"), "{screen}");
    // The selected contract shows its consumers under it.
    let sites = r.t.app.selected_contract_sites();
    assert_eq!(sites.len(), 1);
    assert!(r.screen().contains("app/lib/orders.dart:1"));
    r.key(KeyCode::Char('t'));
    assert!(!r.screen().contains("contracts ·"));
}
