use super::*;

fn task(id: &str, files: &[&str], deps: &[&str]) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: String::new(),
    }
}

fn plan(tasks: Vec<PlanTask>) -> Plan {
    let p = Plan {
        summary: "s".into(),
        tasks,
    };
    p.validate().unwrap();
    p
}

fn fan() -> Plan {
    plan(vec![
        task("t1", &["src/a/**"], &[]),
        task("t2", &["src/b.rs"], &[]),
        task("t3", &["src/c/**", "tests/c.rs"], &[]),
        task("t4", &["docs/d.md"], &[]),
        task("t5", &["src/main.rs"], &["t1", "t2", "t3", "t4"]),
    ])
}

fn areas(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn overlap_is_decided_by_the_literal_part_and_is_cautious() {
    assert!(areas_overlap("src/a/**", "src/a/x.rs"));
    assert!(areas_overlap("src/a/x.rs", "src/a/**"));
    assert!(areas_overlap("src", "src/a/x.rs"));
    assert!(areas_overlap("src/a.rs", "./src/a.rs"));
    assert!(!areas_overlap("src/a/**", "src/b/**"));
    assert!(!areas_overlap("src/a.rs", "src/b.rs"));
    assert!(!areas_overlap("src/a.rs", "src/a.rs.bak"));
    // A wildcard at the start could be anything.
    assert!(areas_overlap("**/*.rs", "docs/x.md"));
    assert!(areas_overlap("*.md", "src/x.rs"));
    assert!(claims_overlap(
        &areas(&["x.rs", "src/**"]),
        &areas(&["src/y.rs"])
    ));
    assert!(!claims_overlap(&areas(&["x.rs"]), &areas(&["y.rs"])));
}

#[test]
fn a_chain_stays_one_session_and_a_fan_fans_out() {
    let chain = plan(vec![
        task("t1", &["a.rs"], &[]),
        task("t2", &["b.rs"], &["t1"]),
        task("t3", &["c.rs"], &["t2"]),
    ]);
    assert_eq!(Topology::choose(&chain), Topology::Single);
    assert_eq!(Topology::choose(&fan()), Topology::Fanout);
    // Two tasks on one folder can't run at once either.
    let same = plan(vec![
        task("t1", &["src/**"], &[]),
        task("t2", &["src/x.rs"], &[]),
    ]);
    assert_eq!(width(&same), 1);
    assert_eq!(Topology::choose(&same), Topology::Single);
    assert_eq!(Topology::parse("fanout"), Some(Topology::Fanout));
}

#[test]
fn ready_tasks_wait_for_merged_producers_and_free_claims() {
    let p = fan();
    let mut run = GraphRun::new(&p, "swarmux/m/integration", 0);
    assert_eq!(run.ready(&p), vec!["t1", "t2", "t3", "t4"]);
    for id in ["t1", "t2", "t3", "t4"] {
        run.task_mut(id).unwrap().state = TaskState::Running;
    }
    assert!(run.ready(&p).is_empty());
    for id in ["t1", "t2", "t3"] {
        run.task_mut(id).unwrap().state = TaskState::Merged;
    }
    assert!(run.ready(&p).is_empty(), "t4 has not merged yet");
    run.task_mut("t4").unwrap().state = TaskState::Merged;
    assert_eq!(run.ready(&p), vec!["t5"]);
}

#[test]
fn overlapping_claims_are_serialized() {
    let p = plan(vec![
        task("t1", &["src/api/**"], &[]),
        task("t2", &["src/api/routes.rs"], &[]),
        task("t3", &["src/db/**"], &[]),
    ]);
    let mut run = GraphRun::new(&p, "i", 0);
    assert_eq!(run.ready(&p), vec!["t1", "t3"]);
    run.task_mut("t1").unwrap().state = TaskState::Merging;
    run.task_mut("t3").unwrap().state = TaskState::Running;
    assert!(run.ready(&p).is_empty(), "t1 still holds src/api");
    run.task_mut("t1").unwrap().state = TaskState::Merged;
    assert_eq!(run.ready(&p), vec!["t2"]);
}

#[test]
fn merges_go_producer_first() {
    let p = plan(vec![
        task("t2", &["b.rs"], &["t1"]),
        task("t1", &["a.rs"], &[]),
        task("t3", &["c.rs"], &[]),
    ]);
    let mut run = GraphRun::new(&p, "i", 0);
    for id in ["t1", "t3"] {
        run.task_mut(id).unwrap().state = TaskState::Merging;
    }
    assert_eq!(run.queue(&p), vec!["t1", "t3"]);
    assert_eq!(run.next_merge(&p).as_deref(), Some("t1"));
    run.task_mut("t1").unwrap().state = TaskState::Merged;
    run.task_mut("t2").unwrap().state = TaskState::Merging;
    assert_eq!(run.queue(&p), vec!["t3", "t2"]);
    run.task_mut("t3").unwrap().state = TaskState::Merged;
    assert_eq!(run.next_merge(&p).as_deref(), Some("t2"));
    run.task_mut("t2").unwrap().state = TaskState::Merged;
    assert!(run.all_merged());
}

#[test]
fn a_failed_task_leaves_the_run_stuck_once_nothing_else_moves() {
    let p = fan();
    let mut run = GraphRun::new(&p, "i", 0);
    run.task_mut("t1").unwrap().state = TaskState::Failed;
    run.task_mut("t2").unwrap().state = TaskState::Running;
    assert!(run.blocked());
    assert!(!run.stuck(&p));
    for id in ["t2", "t3", "t4"] {
        run.task_mut(id).unwrap().state = TaskState::Merged;
    }
    assert!(run.stuck(&p), "t5 waits on t1 forever");
    assert_eq!(
        run.counts(),
        vec![
            (TaskState::Waiting, 1),
            (TaskState::Merged, 3),
            (TaskState::Failed, 1)
        ]
    );
}

#[test]
fn writes_outside_the_claim_or_the_worktree_are_out_of_scope() {
    let claim = areas(&["src/api/**", "tests/api.rs"]);
    let root = "/wt/abc/m-7";
    let check = |tool: &str, subject: &str| out_of_scope(&claim, root, tool, subject, root);
    assert_eq!(check("Edit", "src/api/routes.rs"), None);
    assert_eq!(check("Write", "/wt/abc/m-7/tests/api.rs"), None);
    assert_eq!(
        check("Edit", "src/db/pool.rs").as_deref(),
        Some("src/db/pool.rs")
    );
    assert_eq!(
        check("MultiEdit", "README.md").as_deref(),
        Some("README.md")
    );
    assert_eq!(
        check("Write", "/repo/src/api/routes.rs").as_deref(),
        Some("/repo/src/api/routes.rs"),
        "the user's own checkout is out of scope"
    );
    assert_eq!(
        check("Edit", "src/api/../db/x.rs").as_deref(),
        Some("src/db/x.rs")
    );
    assert_eq!(check("Bash", "cargo test 2>/dev/null"), None);
    assert_eq!(check("Bash", "echo hi > src/api/x.txt"), None);
    assert_eq!(
        check("Bash", "echo hi > src/lib.rs").as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(
        check("Bash", "rm -f src/db/old.rs").as_deref(),
        Some("src/db/old.rs")
    );
    assert_eq!(check("Read", "src/db/pool.rs"), None, "reading is fine");
    // From a subfolder, relative paths resolve against it.
    let sub = out_of_scope(&claim, root, "Edit", "routes.rs", "/wt/abc/m-7/src/api");
    assert_eq!(sub, None);
}

#[test]
fn windows_paths_compare_without_case() {
    let claim = areas(&["src/api/**"]);
    let root = "C:/wt/abc/m-7";
    assert_eq!(
        out_of_scope(&claim, root, "Edit", r"C:\WT\abc\m-7\SRC\api\x.rs", root),
        None
    );
    assert!(out_of_scope(&claim, root, "Edit", r"C:\wt\abc\m-7\src\db.rs", root).is_some());
}

#[test]
fn the_run_round_trips_through_json() {
    let p = fan();
    let mut run = GraphRun::new(&p, "swarmux/m/integration", 42);
    run.workdir = "/wt/m-integration".into();
    let t = run.task_mut("t2").unwrap();
    t.state = TaskState::Conflict;
    t.job = Some(JobId(9));
    t.branch = "swarmux/m-t2/9".into();
    t.reinvoked = true;
    t.merge_blocked = true;
    t.resumes = 2;
    t.note = "conflict in \"src/b.rs\"".into();
    t.started_at = Some(100);
    run.merged = vec!["t1".into()];
    run.denials = 3;
    run.spent_usd = 1.25;
    run.final_job = Some(JobId(12));
    run.verify_from = Some(500);
    let back = GraphRun::from_json(&Json::parse(&run.to_json()).unwrap()).unwrap();
    assert_eq!(back, run);
}

#[test]
fn a_run_kept_before_resumes_were_counted_reads_as_none() {
    let p = fan();
    let run = GraphRun::new(&p, "swarmux/m/integration", 42);
    let old = run.to_json().replace("\"resumes\":0,", "");
    assert!(!old.contains("resumes"));
    let back = GraphRun::from_json(&Json::parse(&old).unwrap()).unwrap();
    assert!(back.tasks.iter().all(|t| t.resumes == 0));
}

#[test]
fn briefs_name_the_scope_and_the_rebase() {
    let p = fan();
    let c = Contract {
        goal: "split the app".into(),
        ..Contract::default()
    };
    let b = task_brief(&c, &p, &p.tasks[2]);
    assert!(b.contains("Your task: t3"), "{b}");
    assert!(
        b.contains("you may change only src/c/**, tests/c.rs"),
        "{b}"
    );
    assert!(b.contains("- t1 task t1"), "{b}");
    let r = conflict_brief(
        &p.tasks[2],
        "swarmux/m/integration",
        &areas(&["src/c/x.rs"]),
    );
    assert!(r.contains("git rebase swarmux/m/integration"), "{r}");
    assert!(r.contains("src/c/x.rs"), "{r}");
    assert!(scope_reason("t3", &p.tasks[2].files, "src/z.rs").contains("t3's scope"));
    assert_eq!(
        integration_branch("Fast search"),
        "swarmux/fast-search/integration"
    );
}
