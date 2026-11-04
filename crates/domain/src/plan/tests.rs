use super::*;

fn task(id: &str, files: &[&str], deps: &[&str]) -> PlanTask {
    PlanTask {
        id: id.into(),
        title: format!("task {id}"),
        files: files.iter().map(|s| s.to_string()).collect(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        acceptance: "cargo test".into(),
    }
}

fn plan(tasks: Vec<PlanTask>) -> Plan {
    Plan {
        summary: "s".into(),
        tasks,
    }
}

// ---- validation -------------------------------------------------------

#[test]
fn a_good_graph_validates_and_layers_by_dependency() {
    let p = plan(vec![
        task("t3", &["c"], &["t1", "t2"]),
        task("t1", &["a"], &[]),
        task("t2", &["b"], &["t1"]),
    ]);
    assert_eq!(p.validate(), Ok(()));
    assert_eq!(p.layers(), vec![vec![1], vec![2], vec![0]]);
    let rows = p.tree();
    let shape: Vec<(usize, &str)> = rows.iter().map(|r| (r.depth, r.id.as_str())).collect();
    assert_eq!(shape, [(0, "t1"), (1, "t2"), (2, "t3")]);
    assert_eq!(rows[2].after, ["t1", "t2"]);
}

#[test]
fn bad_graphs_are_refused_with_the_reason() {
    let ok = || task("t1", &["a"], &[]);
    assert_eq!(plan(vec![]).validate(), Err(PlanError::NoTasks));
    assert_eq!(
        plan(vec![ok(), ok()]).validate(),
        Err(PlanError::DuplicateId("t1".into()))
    );
    assert_eq!(
        plan(vec![task("", &["a"], &[])]).validate(),
        Err(PlanError::BadId(String::new()))
    );
    assert_eq!(
        plan(vec![task("t1", &[], &[])]).validate(),
        Err(PlanError::NoFiles("t1".into()))
    );
    assert_eq!(
        plan(vec![task("t1", &["a"], &["t9"])]).validate(),
        Err(PlanError::UnknownDependency {
            task: "t1".into(),
            dep: "t9".into()
        })
    );
    assert_eq!(
        plan(vec![task("t1", &["a"], &["t1"])]).validate(),
        Err(PlanError::SelfDependency("t1".into()))
    );
    let cycle = plan(vec![
        task("t1", &["a"], &["t3"]),
        task("t2", &["b"], &["t1"]),
        task("t3", &["c"], &["t2"]),
        task("t4", &["d"], &[]),
    ]);
    assert_eq!(
        cycle.validate(),
        Err(PlanError::Cycle(vec![
            "t1".into(),
            "t2".into(),
            "t3".into()
        ]))
    );
    assert!(cycle.layers().is_empty());
    let many = plan(
        (0..=MAX_TASKS)
            .map(|i| task(&format!("t{i}"), &["a"], &[]))
            .collect(),
    );
    assert_eq!(many.validate(), Err(PlanError::TooMany(MAX_TASKS + 1)));
}

#[test]
fn plans_parse_from_bare_json_or_prose_and_are_validated() {
    let json = r#"{"summary":"do it","tasks":[
        {"id":"t1","title":"Index","files":["src/a.rs"],"depends_on":[],"acceptance":"cargo test"},
        {"id":"t2","title":"Query","files":["src/b/**"],"depends_on":["t1"],"acceptance":""}]}"#;
    let p = parse_plan(json).unwrap();
    assert_eq!(p.tasks.len(), 2);
    assert_eq!(p.tasks[1].depends_on, ["t1"]);
    let fenced = format!("Here you go:\n```json\n{json}\n```\n");
    assert_eq!(parse_plan(&fenced).unwrap(), p);
    assert!(parse_plan("no json").unwrap_err().contains("no JSON"));
    let cyc = r#"{"summary":"","tasks":[{"id":"a","title":"","files":["x"],"depends_on":["a"],"acceptance":""}]}"#;
    assert!(parse_plan(cyc).unwrap_err().contains("depends on itself"));
    let schema: Json = Json::parse(PLAN_SCHEMA).unwrap();
    assert!(schema.get("properties").is_some());
}

#[test]
fn a_plan_survives_its_json_and_the_record_survives_its_encoding() {
    let p = plan(vec![
        task("t1", &["src/\"q\".rs"], &[]),
        task("t2", &["b"], &["t1"]),
    ]);
    assert_eq!(parse_plan(&p.to_json()).unwrap(), p);
    assert_eq!(p.digest(), parse_plan(&p.to_json()).unwrap().digest());
    let drift = check_drift(&p, &["zzz.rs".to_string()], DEFAULT_DRIFT_THRESHOLD);
    let p2 = p.clone();
    let rec = PlanRecord {
        plan_first: Some(true),
        status: PlanStatus::Approved,
        plan: Some(p),
        note: "tighter\nplease".into(),
        rounds: 2,
        cost_usd: 0.31,
        drift: Some(drift),
        graph: Some(crate::taskgraph::GraphRun::new(
            &p2,
            "swarmux/m/integration",
            7,
        )),
    };
    assert_eq!(PlanRecord::decode(&rec.encode()).unwrap(), rec);
    let bare = PlanRecord::default();
    assert_eq!(PlanRecord::decode(&bare.encode()).unwrap(), bare);
    assert!(bare.approved().is_none());
    assert!(rec.approved().is_some());
}

#[test]
fn prompts_carry_the_goal_the_plan_and_the_rejection() {
    let c = Contract {
        goal: "make search fast".into(),
        ..Contract::default()
    };
    let first = plan_prompt(&c, None, "");
    assert!(first.contains("make search fast") && first.contains("depends_on"));
    assert!(!first.contains("rejected"));
    let p = plan(vec![task("t1", &["a"], &[])]);
    let again = plan_prompt(&c, Some(&p), "split the index work");
    assert!(again.contains("rejected") && again.contains("split the index work"));
    let brief = plan_brief(&plan(vec![
        task("t1", &["a"], &[]),
        task("t2", &["b", "c/**"], &["t1"]),
    ]));
    assert!(brief.starts_with(PLAN_MARKER));
    assert!(brief.contains("- t2 task t2 (after t1) [files: b, c/**]"));
}

// ---- drift ------------------------------------------------------------

fn files(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn areas_match_as_files_folders_and_globs() {
    assert!(area_matches("src/a.rs", "src/a.rs"));
    assert!(area_matches("src/search", "src/search/index.rs"));
    assert!(area_matches("src/search/", "src/search/index.rs"));
    assert!(!area_matches("src/search", "src/searchx/a.rs"));
    assert!(area_matches("src/**", "src/a/b.rs"));
    assert!(area_matches("**/*.md", "docs/x/y.md"));
    assert!(area_matches("src/*.rs", "src/main.rs"));
    assert!(!area_matches("src/*.rs", "src/cli/args.rs"));
    assert!(area_matches(r"src\search\**", "src/search/a.rs"));
    assert!(area_matches("./src/a.rs", "src/a.rs"));
    assert!(area_matches(".", "anything/at/all"));
    assert!(!area_matches("src/a.rs", "src/b.rs"));
}

#[test]
fn lockfiles_and_swarmux_files_are_not_scope() {
    let p = plan(vec![task("t1", &["src/a.rs"], &[])]);
    let r = check_drift(
        &p,
        &files(&[
            "src/a.rs",
            "Cargo.lock",
            "web/package-lock.json",
            ".swarmux/missions/x.toml",
        ]),
        DEFAULT_DRIFT_THRESHOLD,
    );
    assert_eq!((r.changed, r.outside_count), (1, 0));
    assert!(!r.drifted());
    assert_eq!(r.summary(), "the change matches the plan");
}

#[test]
fn nothing_changed_is_planned_work_left_undone() {
    let p = plan(vec![task("t1", &["src/a.rs"], &[])]);
    let r = check_drift(&p, &[], DEFAULT_DRIFT_THRESHOLD);
    assert!(r.drifted());
    assert_eq!(r.untouched, ["t1"]);
    assert!(r.summary().contains("1 of 1 tasks with no change"));
}

#[test]
fn the_threshold_is_the_line_and_bad_ones_fall_back() {
    let p = plan(vec![
        task("t1", &["a"], &[]),
        task("t2", &["b"], &[]),
        task("t3", &["c"], &[]),
        task("t4", &["d"], &[]),
    ]);
    let one_idle = files(&["a", "b", "c"]);
    assert!(check_drift(&p, &one_idle, 0.25).drifted());
    assert!(!check_drift(&p, &one_idle, 0.26).drifted());
    assert_eq!(
        check_drift(&p, &one_idle, f64::NAN).threshold,
        DEFAULT_DRIFT_THRESHOLD
    );
    let r = check_drift(&p, &one_idle, 0.5);
    assert_eq!(
        DriftReport::from_json(&Json::parse(&r.to_json()).unwrap()),
        Some(r)
    );
}

// ---- the seeded suite -------------------------------------------------

/// Three fixed plans, each with the files a faithful build touches per task.
struct Seed {
    plan: Plan,
    /// Files a faithful build changes for each task.
    built: Vec<Vec<&'static str>>,
}

fn seeds() -> Vec<Seed> {
    let search = plan(vec![
        task("t1", &["src/search/index.rs", "src/search/index/**"], &[]),
        task("t2", &["src/search/query.rs"], &["t1"]),
        task("t3", &["src/main.rs", "src/cli/**"], &["t2"]),
        task("t4", &["tests/search_*.rs"], &["t1", "t2"]),
    ]);
    let auth = plan(vec![
        task("t1", &["src/auth/**"], &[]),
        task("t2", &["src/api/login.rs", "src/api/session.rs"], &["t1"]),
        task("t3", &["docs/auth.md", "tests/auth/**"], &["t2"]),
    ]);
    let ui = plan(vec![
        task("t1", &["web/src/components/Button/**"], &[]),
        task("t2", &["web/src/styles/"], &[]),
        task("t3", &["web/src/pages/settings/**"], &["t1"]),
        task("t4", &["web/src/api/settings.ts"], &[]),
        task("t5", &["web/tests/settings/**"], &["t3", "t4"]),
    ]);
    vec![
        Seed {
            plan: search,
            built: vec![
                vec!["src/search/index.rs", "src/search/index/tokens.rs"],
                vec!["src/search/query.rs"],
                vec!["src/main.rs", "src/cli/args.rs"],
                vec!["tests/search_basic.rs", "tests/search_ranking.rs"],
            ],
        },
        Seed {
            plan: auth,
            built: vec![
                vec!["src/auth/mod.rs", "src/auth/token.rs", "src/auth/hash.rs"],
                vec!["src/api/login.rs", "src/api/session.rs"],
                vec!["docs/auth.md", "tests/auth/login.rs"],
            ],
        },
        Seed {
            plan: ui,
            built: vec![
                vec![
                    "web/src/components/Button/Button.tsx",
                    "web/src/components/Button/index.ts",
                ],
                vec!["web/src/styles/settings.css"],
                vec![
                    "web/src/pages/settings/Page.tsx",
                    "web/src/pages/settings/Form.tsx",
                ],
                vec!["web/src/api/settings.ts"],
                vec!["web/tests/settings/form.test.tsx"],
            ],
        },
    ]
}

/// A build: the tasks done (by index) plus extra files.
fn build(seed: &Seed, done: &[usize], extra: &[&str]) -> Vec<String> {
    done.iter()
        .flat_map(|i| seed.built[*i].iter().copied())
        .chain(extra.iter().copied())
        .map(str::to_string)
        .collect()
}

/// `(seed, tasks done, extra files)` that must not be flagged: faithful
/// builds, builds with incidental changes, and small, tolerated strays.
fn clean_cases() -> Vec<(usize, Vec<usize>, Vec<&'static str>)> {
    vec![
        (0, vec![0, 1, 2, 3], vec![]),
        (0, vec![0, 1, 2, 3], vec!["Cargo.lock"]),
        (
            0,
            vec![0, 1, 2, 3],
            vec!["Cargo.lock", ".swarmux/missions/search.toml"],
        ),
        (0, vec![0, 1, 2], vec![]), // tests left to a later pass: 1 of 4 tasks
        (0, vec![0, 1, 3], vec!["src/search/index/mod.rs"]),
        (0, vec![0, 1, 2, 3], vec!["README.md"]), // 1 stray of 9 files
        (0, vec![0, 1, 2, 3], vec!["src/cli/help.rs"]), // inside a planned folder
        (0, vec![1, 2, 3, 0], vec!["src/search/index/extra/deep.rs"]),
        (1, vec![0, 1, 2], vec![]),
        (1, vec![0, 1, 2], vec!["Cargo.lock"]),
        (1, vec![0, 1, 2], vec!["src/auth/errors.rs"]),
        (1, vec![0, 1, 2], vec!["CHANGELOG.md"]), // 1 stray of 7 files
        (1, vec![0, 1, 2], vec!["tests/auth/helpers/mod.rs"]),
        (
            1,
            vec![0, 1, 2],
            vec!["Cargo.lock", ".swarmux/evidence/x.json"],
        ),
        (2, vec![0, 1, 2, 3, 4], vec![]),
        (2, vec![0, 1, 2, 3, 4], vec!["web/package-lock.json"]),
        (2, vec![0, 1, 2, 3], vec![]), // tests later: 1 of 5 tasks
        (2, vec![0, 1, 3, 4], vec![]), // page later: 1 of 5 tasks
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec!["web/src/styles/extra/dark.css"],
        ),
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec!["web/src/pages/settings/hooks/use_form.ts"],
        ),
        (2, vec![0, 1, 2, 3, 4], vec!["docs/ui.md"]), // 1 stray of 8 files
        (0, vec![0, 1, 2, 3], vec!["src/lib.rs"]),    // 1 stray of 9 files
        (1, vec![0, 1, 2], vec!["Cargo.lock", "Cargo.lock"]),
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec!["yarn.lock", "web/pnpm-lock.yaml"],
        ),
    ]
}

/// The same builds, gone wrong: scope creep, planned work skipped,
/// the wrong place altogether.
fn drift_cases() -> Vec<(usize, Vec<usize>, Vec<&'static str>)> {
    vec![
        // scope creep: a third or more of the change is unplanned
        (
            0,
            vec![0, 1, 2, 3],
            vec![
                "src/auth/login.rs",
                "src/db/schema.rs",
                "src/db/migrate.rs",
                "build.rs",
                "deploy/ci.yml",
            ],
        ),
        (
            0,
            vec![0, 1, 2, 3],
            vec![
                "src/server/mod.rs",
                "src/server/routes.rs",
                "src/server/tls.rs",
                "Dockerfile",
            ],
        ),
        (
            0,
            vec![0, 1],
            vec!["src/ranking/bm25.rs", "src/ranking/mod.rs"],
        ),
        (
            0,
            vec![0, 1, 2, 3],
            vec![
                "scripts/bench.sh",
                "benches/search.rs",
                "src/telemetry.rs",
                "src/config.rs",
                "src/error.rs",
            ],
        ),
        (
            1,
            vec![0, 1, 2],
            vec![
                "src/db/users.rs",
                "src/db/sessions.rs",
                "migrations/001.sql",
            ],
        ),
        (
            1,
            vec![0, 1, 2],
            vec![
                "src/billing/mod.rs",
                "src/billing/invoice.rs",
                "src/billing/tax.rs",
                "src/billing/pdf.rs",
            ],
        ),
        (1, vec![0, 1], vec!["src/ui/login.rs", "src/ui/theme.rs"]),
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec![
                "server/api.py",
                "server/models.py",
                "server/db.py",
                "server/auth.py",
                "docker-compose.yml",
            ],
        ),
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec![
                "web/src/pages/billing/Page.tsx",
                "web/src/pages/billing/Form.tsx",
                "web/src/pages/admin/Page.tsx",
                "web/src/api/billing.ts",
            ],
        ),
        (
            2,
            vec![0, 2],
            vec!["web/src/state/store.ts", "web/src/state/slices.ts"],
        ),
        // planned work skipped: a third or more of the tasks never touched
        (0, vec![0], vec![]),
        (0, vec![0, 1], vec![]),
        (0, vec![2, 3], vec![]),
        (1, vec![0], vec![]),
        (1, vec![1, 2], vec![]),
        (1, vec![2], vec![]),
        (2, vec![0], vec![]),
        (2, vec![0, 1, 3], vec![]),
        (2, vec![4], vec![]),
        // nothing built, or only the wrong place
        (0, vec![], vec![]),
        (1, vec![], vec!["Cargo.lock"]),
        (2, vec![], vec!["README.md"]),
        (0, vec![], vec!["src/other/a.rs", "src/other/b.rs"]),
        (
            1,
            vec![0, 1, 2],
            vec!["src/x1.rs", "src/x2.rs", "src/x3.rs"],
        ),
        (
            2,
            vec![0, 1, 2, 3, 4],
            vec!["a/b.rs", "c/d.rs", "e/f.rs", "g/h.rs"],
        ),
    ]
}

fn flagged(seeds: &[Seed], cases: &[(usize, Vec<usize>, Vec<&'static str>)]) -> Vec<bool> {
    cases
        .iter()
        .map(|(s, done, extra)| {
            let seed = &seeds[*s];
            check_drift(
                &seed.plan,
                &build(seed, done, extra),
                DEFAULT_DRIFT_THRESHOLD,
            )
            .drifted()
        })
        .collect()
}

#[test]
fn the_seeded_plans_are_valid() {
    for seed in seeds() {
        assert_eq!(seed.plan.validate(), Ok(()));
        assert_eq!(seed.built.len(), seed.plan.tasks.len());
        // The faithful build touches every task and nothing else.
        let all: Vec<usize> = (0..seed.built.len()).collect();
        let r = check_drift(
            &seed.plan,
            &build(&seed, &all, &[]),
            DEFAULT_DRIFT_THRESHOLD,
        );
        assert_eq!((r.outside_count, r.untouched.len()), (0, 0), "{:?}", r);
    }
}

#[test]
fn seeded_drift_is_flagged_and_clean_builds_are_not() {
    let seeds = seeds();
    let drifts = drift_cases();
    let cleans = clean_cases();
    let caught = flagged(&seeds, &drifts);
    let false_alarms = flagged(&seeds, &cleans);
    let recall = caught.iter().filter(|f| **f).count() as f64 / drifts.len() as f64;
    let false_rate = false_alarms.iter().filter(|f| **f).count() as f64 / cleans.len() as f64;
    println!(
        "plan drift, seeded suite: flagged {}/{} drift cases ({:.1}%), false drift on {}/{} clean cases ({:.1}%)",
        caught.iter().filter(|f| **f).count(),
        drifts.len(),
        recall * 100.0,
        false_alarms.iter().filter(|f| **f).count(),
        cleans.len(),
        false_rate * 100.0,
    );
    for (i, f) in caught.iter().enumerate() {
        assert!(*f, "drift case {i} was not flagged: {:?}", drifts[i]);
    }
    for (i, f) in false_alarms.iter().enumerate() {
        assert!(!*f, "clean case {i} was flagged: {:?}", cleans[i]);
    }
    assert!(recall >= 0.9, "recall {recall}");
    assert_eq!(false_rate, 0.0);
}
