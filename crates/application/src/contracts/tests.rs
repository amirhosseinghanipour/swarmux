use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use swarmux_domain::estimate_tokens;

use super::hub::JobTree;
use super::*;
use crate::ports::HookRequest;

/// An in-memory tree of files: `(folder, relative path) -> (text, mtime)`.
#[derive(Default)]
struct FakeTree {
    files: Mutex<BTreeMap<(String, String), (String, i64)>>,
    reads: AtomicUsize,
}

impl FakeTree {
    fn put(&self, dir: &str, rel: &str, text: &str, mtime: i64) {
        self.files
            .lock()
            .unwrap()
            .insert((dir.into(), rel.into()), (text.into(), mtime));
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn copy(&self, from: &str, to: &str) {
        let mut f = self.files.lock().unwrap();
        let moved: Vec<_> = f
            .iter()
            .filter(|((d, _), _)| d.starts_with(from))
            .map(|((d, r), v)| ((d.replacen(from, to, 1), r.clone()), v.clone()))
            .collect();
        f.extend(moved);
    }
}

impl SourceTree for FakeTree {
    fn files(&self, dir: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<SourceFile> {
        let dir = dir.to_string_lossy().replace('\\', "/");
        self.files
            .lock()
            .unwrap()
            .iter()
            .filter(|((d, r), _)| *d == dir && wanted(r))
            .map(|((_, r), (t, m))| SourceFile {
                rel: r.clone(),
                size: t.len() as u64,
                mtime: *m,
            })
            .collect()
    }

    fn read(&self, dir: &Path, rel: &str) -> Option<String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let dir = dir.to_string_lossy().replace('\\', "/");
        self.files
            .lock()
            .unwrap()
            .get(&(dir, rel.to_string()))
            .map(|(t, _)| t.clone())
    }
}

const MIGRATION: &str = "create table orders (id uuid primary key, total numeric not null);\n\
                         create function get_orders(uid uuid) returns setof orders language sql as $$ select 1 $$;\n";
const APP: &str = "final r = supabase.from('orders').select('id, total');\n";
const ADMIN: &str =
    "await supabase.rpc('get_orders');\nawait supabase.from('orders').select('*');\n";

fn repos() -> Vec<(String, String)> {
    vec![
        ("backend".into(), "backend".into()),
        ("app".into(), "app".into()),
        ("admin".into(), "admin".into()),
    ]
}

fn workspace_tree(t: &FakeTree, root: &str) {
    t.put(
        &format!("{root}/backend"),
        "supabase/migrations/001.sql",
        MIGRATION,
        1,
    );
    t.put(&format!("{root}/app"), "lib/orders.dart", APP, 1);
    t.put(&format!("{root}/admin"), "src/api.ts", ADMIN, 1);
    t.put(&format!("{root}/admin"), "README.md", "# not read", 1);
}

#[test]
fn only_changed_files_are_read_again() {
    let t = FakeTree::default();
    workspace_tree(&t, "/ws");
    let none = Cache::new();
    let first = scan_tree(&t, Path::new("/ws"), &repos(), &none);
    assert_eq!(first.stats.files, 3, "markdown is not read");
    assert_eq!((first.stats.read, first.stats.extracted), (3, 3));
    let cache: Cache = first
        .records
        .iter()
        .cloned()
        .map(|r| ((r.repo.clone(), r.path.clone()), r))
        .collect();

    // Nothing changed: nothing is even opened.
    let before = t.reads();
    let again = scan_tree(&t, Path::new("/ws"), &repos(), &cache);
    assert_eq!(t.reads(), before);
    assert_eq!(again.stats.extracted, 0);
    assert!(again.upserts.is_empty());

    // One file changed: that file is read and extracted, no other.
    t.put(
        "/ws/app",
        "lib/orders.dart",
        "final r = supabase.from('purchases');\n",
        2,
    );
    let before = t.reads();
    let one = scan_tree(&t, Path::new("/ws"), &repos(), &cache);
    assert_eq!(t.reads() - before, 1);
    assert_eq!(one.stats.extracted, 1);
    assert_eq!(one.upserts.len(), 1);
    assert_eq!(one.upserts[0].path, "lib/orders.dart");

    // Touched but identical: read once, early cutoff, no extraction.
    t.put("/ws/admin", "src/api.ts", ADMIN, 9);
    let before = t.reads();
    let cache: Cache = one
        .records
        .iter()
        .cloned()
        .map(|r| ((r.repo.clone(), r.path.clone()), r))
        .collect();
    let touched = scan_tree(&t, Path::new("/ws"), &repos(), &cache);
    assert_eq!(t.reads() - before, 1);
    assert_eq!(touched.stats.extracted, 0);

    // A file that went away is forgotten.
    t.files
        .lock()
        .unwrap()
        .remove(&("/ws/admin".to_string(), "src/api.ts".to_string()));
    let gone = scan_tree(&t, Path::new("/ws"), &repos(), &cache);
    assert_eq!(
        gone.removed,
        vec![("admin".to_string(), "src/api.ts".to_string())]
    );
}

fn tree_for_job(t: &FakeTree, baseline_root: &str, job_root: &str) -> JobTree {
    let scan = scan_tree(t, Path::new(baseline_root), &repos(), &Cache::new());
    JobTree {
        root: PathBuf::from(job_root),
        repos: repos(),
        commands: vec![
            (
                "app".into(),
                vec![("analyze".into(), "flutter analyze".into())],
            ),
            (
                "admin".into(),
                vec![("typecheck".into(), "tsc --noEmit".into())],
            ),
        ],
        baseline: Arc::new(scan.index),
        cache: Arc::new(
            scan.records
                .into_iter()
                .map(|r| ((r.repo.clone(), r.path.clone()), r))
                .collect(),
        ),
    }
}

fn hub_with(t: Arc<FakeTree>, tree: JobTree) -> (Arc<ContractHub>, JobId) {
    let hub = ContractHub::new(t);
    let job = JobId(7);
    hub.register(job, tree);
    (hub, job)
}

#[test]
fn a_renamed_table_with_unchanged_consumers_fails_the_contracts_check() {
    let t = Arc::new(FakeTree::default());
    workspace_tree(&t, "/ws");
    t.copy("/ws", "/task");
    let (hub, job) = hub_with(t.clone(), tree_for_job(&t, "/ws", "/task"));
    assert!(hub.judge(job).is_none(), "nothing changed yet");

    t.put(
        "/task/backend",
        "supabase/migrations/001.sql",
        &MIGRATION.replace("orders (", "purchases ("),
        5,
    );
    let j = hub.judge(job).expect("a provider changed");
    assert_eq!(j.run.exit, Some(1));
    assert!(j.run.output.contains("table orders"), "{}", j.run.output);
    assert!(
        j.run.output.contains("app/lib/orders.dart:1"),
        "{}",
        j.run.output
    );
    assert!(
        j.run.output.contains("admin/src/api.ts:2"),
        "{}",
        j.run.output
    );
    // The consumers' repositories get their own checks.
    let names: Vec<&str> = j.checks.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"app: flutter analyze"), "{names:?}");
    assert!(names.contains(&"admin: tsc --noEmit"), "{names:?}");
    let app = j
        .checks
        .iter()
        .find(|c| c.name.starts_with("app:"))
        .unwrap();
    assert_eq!(app.dir.as_deref(), Some("app"));

    // Updating the consumers clears it.
    t.put(
        "/task/app",
        "lib/orders.dart",
        &APP.replace("orders", "purchases"),
        6,
    );
    t.put(
        "/task/admin",
        "src/api.ts",
        &ADMIN.replace("'orders'", "'purchases'"),
        6,
    );
    let j = hub.judge(job).expect("still a change");
    assert_eq!(j.run.exit, Some(0), "{}", j.run.output);
    assert!(hub.escalations().is_empty());
    let events = hub.drain();
    assert!(matches!(
        events.first(),
        Some(ContractEvent::Checked { passed: false, .. })
    ));
    assert!(matches!(
        events.last(),
        Some(ContractEvent::Checked { passed: true, .. })
    ));
}

#[test]
fn a_mission_that_already_checks_a_repo_adds_nothing_for_it() {
    let t = Arc::new(FakeTree::default());
    workspace_tree(&t, "/ws");
    t.copy("/ws", "/task");
    let tree = tree_for_job(&t, "/ws", "/task");
    let (hub, job) = hub_with(t.clone(), tree);
    hub.set_mission(
        job,
        vec!["backend".into(), "app".into(), "admin".into()],
        vec!["app: flutter test".into()],
    );
    t.put(
        "/task/backend",
        "supabase/migrations/001.sql",
        &MIGRATION.replace("uid uuid", "uid uuid, since date"),
        5,
    );
    let j = hub.judge(job).unwrap();
    assert_eq!(j.run.exit, Some(0), "a changed signature is not a removal");
    assert!(j.run.output.contains("CHANGED"), "{}", j.run.output);
    let names: Vec<&str> = j.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["admin: tsc --noEmit"],
        "app has its own check already"
    );
}

#[test]
fn consumers_outside_the_mission_escalate() {
    let t = Arc::new(FakeTree::default());
    workspace_tree(&t, "/ws");
    t.copy("/ws", "/task");
    let tree = tree_for_job(&t, "/ws", "/task");
    let (hub, job) = hub_with(t.clone(), tree);
    hub.set_mission(job, vec!["backend".into()], Vec::new());
    t.put(
        "/task/backend",
        "supabase/migrations/001.sql",
        &MIGRATION.replace("orders (", "purchases ("),
        5,
    );
    let j = hub.judge(job).unwrap();
    assert_eq!(j.run.exit, Some(1));
    assert!(
        j.run.output.contains("OUTSIDE THE MISSION"),
        "{}",
        j.run.output
    );
    assert!(
        j.checks.is_empty(),
        "their repositories are not in the mission"
    );
    let e = hub.escalations();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].repos, vec!["admin".to_string(), "app".to_string()]);
    assert!(e[0].places.contains(&"app/lib/orders.dart:1".to_string()));
}

fn request(job: JobId, tool: &str, path: &str) -> HookRequest {
    HookRequest {
        request_id: 1,
        job_id: Some(job),
        tool_name: tool.into(),
        subject: path.into(),
        input: String::new(),
        cwd: String::new(),
        content: String::new(),
        ..HookRequest::default()
    }
}

#[test]
fn reading_a_provider_lists_its_consumers_once_and_a_consumer_shows_the_provider() {
    let t = Arc::new(FakeTree::default());
    workspace_tree(&t, "/ws");
    let (hub, job) = hub_with(t.clone(), tree_for_job(&t, "/ws", "/ws"));
    let note = hub
        .pull(
            &request(job, "Read", "/ws/backend/supabase/migrations/001.sql"),
            300,
        )
        .expect("consumers");
    assert!(note.contains("consumed by"), "{note}");
    assert!(note.contains("app/lib/orders.dart:1"), "{note}");
    assert!(note.contains("admin/src/api.ts:"), "{note}");
    assert!(estimate_tokens(&note) <= 300);
    assert!(
        hub.pull(
            &request(job, "Edit", "backend/supabase/migrations/001.sql"),
            300
        )
        .is_none(),
        "once per job"
    );
    let n = hub
        .pull(&request(job, "Edit", "/ws/app/lib/orders.dart"), 300)
        .expect("the provider's shape");
    assert!(n.contains("provided by"), "{n}");
    assert!(n.contains("backend/supabase/migrations/001.sql:1"), "{n}");
    assert!(hub.pull(&request(job, "Bash", "ls"), 300).is_none());
    assert!(
        hub.pull(&request(job, "Read", "/elsewhere/x.sql"), 300)
            .is_none()
    );
    let injected = hub
        .drain()
        .into_iter()
        .filter(|e| matches!(e, ContractEvent::Injected { .. }))
        .count();
    assert_eq!(injected, 2);
}
