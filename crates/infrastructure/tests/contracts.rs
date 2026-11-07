//! Cross-repo contracts end to end, against the real file system and
//! SQLite: a workspace of `backend/` (SQL migrations), `app/` (Dart) and
//! `admin/` (TypeScript). The contracts and their consumers are found; a
//! job that reads the migration is told who consumes it; a mission job that
//! renames the table without touching the consumers is blocked at the gate
//! with both places, and let go once they are updated.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use swarmux_application::ports::HookRequest;
use swarmux_application::{
    ContractService, GateKeeper, HookNotice, JobGates, JobStart, JobStarts, StopAnswer,
    StopContext, StopPolicy, WorkspaceStore,
};
use swarmux_domain::mission::MissionGate;
use swarmux_domain::{HookEvent, JobId, Repo, Session, SessionId, Workspace, WorkspaceId};
use swarmux_infrastructure::{
    FsGateSource, FsSourceTree, ProcessChecker, SqliteStore, SystemClock,
};

const MIGRATION: &str = "-- the shop's orders\n\
create table orders (\n  id uuid primary key,\n  total numeric not null\n);\n\
create function get_orders(uid uuid) returns setof orders language sql as $$ select * from orders $$;\n";
const APP: &str = "import 'package:supabase/supabase.dart';\n\
Future<List> load() async {\n  return supabase.from('orders').select('id, total');\n}\n";
const ADMIN: &str = "export const list = () => supabase.rpc('get_orders', { uid });\n\
export const raw = () => supabase.from('orders').select('*');\n";

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "swarmux-contracts-e2e-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn repo(root: &Path, name: &str, commands: &[(&str, &str)]) -> Repo {
    fs::create_dir_all(root.join(name).join(".git")).unwrap();
    Repo {
        name: name.into(),
        path: name.into(),
        git_root: root.join(name),
        default_branch: Some("main".into()),
        stacks: Vec::new(),
        commands: commands
            .iter()
            .map(|(n, l)| swarmux_domain::workspace::RepoCommand {
                name: (*n).into(),
                line: (*l).into(),
                source: "test".into(),
            })
            .collect(),
    }
}

fn request(job: JobId, tool: &str, path: &Path) -> HookRequest {
    HookRequest {
        request_id: 1,
        job_id: Some(job),
        tool_name: tool.into(),
        subject: path.to_string_lossy().into_owned(),
        input: String::new(),
        cwd: String::new(),
        content: String::new(),
    }
}

#[test]
fn a_rename_that_leaves_consumers_behind_is_blocked_with_both_places() {
    let root = temp("rename");
    write(
        &root,
        "backend/supabase/migrations/001_orders.sql",
        MIGRATION,
    );
    write(&root, "app/lib/orders.dart", APP);
    write(&root, "admin/src/api.ts", ADMIN);
    write(
        &root,
        "admin/node_modules/dep/index.ts",
        "supabase.from('orders')",
    );
    write(&root, "app/README.md", "supabase.from('orders') in prose");

    let store = Rc::new(SqliteStore::open_in_memory().unwrap());
    let mut w = Workspace::new(WorkspaceId::random(), "shop", root.clone(), 1).unwrap();
    w.repos = vec![
        repo(&root, "backend", &[]),
        repo(&root, "app", &[("analyze", "flutter analyze")]),
        repo(&root, "admin", &[("typecheck", "tsc --noEmit")]),
    ];
    store.save(&w).unwrap();
    let svc = ContractService::new(
        store.clone(),
        store.clone(),
        Arc::new(FsSourceTree),
        Rc::new(SystemClock),
    );

    // The contracts and their consumers are found, and a second scan reads
    // nothing again.
    let first = svc.rescan(&w).unwrap();
    assert_eq!(
        first.stats.files, 3,
        "node_modules and markdown are left out"
    );
    assert_eq!(first.stats.extracted, 3);
    let second = svc.rescan(&w).unwrap();
    assert_eq!((second.stats.read, second.stats.extracted), (0, 0));
    let orders = svc.consumers(w.id, "orders").unwrap();
    assert_eq!(orders.len(), 1);
    assert_eq!(
        orders[0].provided.place(),
        "backend/supabase/migrations/001_orders.sql:2"
    );
    let places: Vec<String> = orders[0].sites.iter().map(|s| s.place()).collect();
    assert_eq!(places, vec!["admin/src/api.ts:2", "app/lib/orders.dart:3"]);
    let f = svc.consumers(w.id, "get_orders").unwrap();
    assert_eq!(f[0].sites[0].place(), "admin/src/api.ts:1");
    assert!(
        svc.rows(w.id)
            .unwrap()
            .iter()
            .any(|r| r.provided.sym.label() == "column orders.total" && r.consumers == 1)
    );

    // A job starts in the workspace root, as a mission's job would.
    let mut session = Session::new(SessionId::random(), "shop", root.clone(), "sonnet", 0).unwrap();
    session.workspace = Some(w.id);
    let job = JobId(7);
    svc.started(&JobStart {
        job,
        session: &session,
        cwd: &root,
        repos: Vec::new(),
        review: false,
    })
    .unwrap();

    // Reading the migration tells it who consumes it.
    let hub = svc.hub();
    let note = hub
        .pull(
            &request(
                job,
                "Read",
                &root.join("backend/supabase/migrations/001_orders.sql"),
            ),
            300,
        )
        .expect("a note about the migration");
    assert!(note.contains("app/lib/orders.dart:3"), "{note}");
    assert!(note.contains("admin/src/api.ts:2"), "{note}");

    // The mission's gate.
    let (keeper, _rx) = GateKeeper::new(Arc::new(FsGateSource::new()), Arc::new(ProcessChecker));
    let keeper = keeper.with_contracts(hub.clone());
    keeper.assign(job, MissionGate::default());
    let cwd = root.to_string_lossy().into_owned();
    let edit = || {
        keeper.observe(&HookNotice {
            event: HookEvent::PostToolUse,
            job: Some(job),
            session: "claude-1".into(),
            cwd: cwd.clone(),
            tool: Some("Edit".into()),
            summary: "Edit".into(),
            detail: String::new(),
            input_sha256: String::new(),
            input_len: 0,
            raw: String::new(),
        });
    };
    let stop = || {
        keeper.on_stop(&StopContext {
            event: HookEvent::Stop,
            job: Some(job),
            session: "claude-1".into(),
            cwd: cwd.clone(),
            last_message: "done".into(),
        })
    };

    write(
        &root,
        "backend/supabase/migrations/001_orders.sql",
        &MIGRATION.replace("table orders", "table purchases"),
    );
    edit();
    let StopAnswer::Block { reason } = stop() else {
        panic!("the rename must be blocked");
    };
    assert!(reason.contains("table orders"), "{reason}");
    assert!(reason.contains("app/lib/orders.dart:3"), "{reason}");
    assert!(reason.contains("admin/src/api.ts:2"), "{reason}");

    // Updating the consumers lets it through (the function still exists).
    write(
        &root,
        "app/lib/orders.dart",
        &APP.replace("'orders'", "'purchases'"),
    );
    write(
        &root,
        "admin/src/api.ts",
        &ADMIN.replace("'orders'", "'purchases'"),
    );
    write(
        &root,
        "backend/supabase/migrations/001_orders.sql",
        &MIGRATION
            .replace("table orders", "table purchases")
            .replace("setof orders", "setof purchases")
            .replace("from orders", "from purchases"),
    );
    edit();
    match stop() {
        StopAnswer::Allow => {}
        StopAnswer::Block { reason } => {
            assert!(!reason.contains("BROKEN"), "{reason}");
        }
    }
    let _ = fs::remove_dir_all(&root);
}
