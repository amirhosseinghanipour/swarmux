use super::*;

fn created(path: &str) -> FileDiff {
    FileDiff {
        path: path.into(),
        created: true,
        added: vec!["x".into()],
        ..FileDiff::default()
    }
}

fn changed(path: &str) -> FileDiff {
    FileDiff {
        path: path.into(),
        added: vec!["x".into()],
        removed: vec!["y".into()],
        ..FileDiff::default()
    }
}

fn deleted(path: &str) -> FileDiff {
    FileDiff {
        path: path.into(),
        deleted: true,
        removed: vec!["y".into()],
        ..FileDiff::default()
    }
}

fn none(_: &str) -> Option<String> {
    None
}

fn found(files: &[FileDiff], extra: &[String], allow: &[String]) -> SubstrateScan {
    let globs = substrate_globs(extra, &[]);
    scan(files, &globs, allow, &none, &none, false)
}

#[test]
fn a_planted_harness_file_is_a_finding_at_any_depth() {
    let paths = [
        "conftest.py",
        "tests/conftest.py",
        "a/b/c/conftest.py",
        "pytest.ini",
        "sitecustomize.py",
        "evil.pth",
        "jest.config.js",
        "web/jest.config.ts",
        "vitest.config.mts",
        ".mocharc.json",
        "build.rs",
        "crates/x/build.rs",
        ".cargo/config.toml",
        "crates/x/.cargo/config.toml",
        ".cargo/config",
        "Makefile",
        "justfile",
        ".github/workflows/ci.yml",
        "dart_test.yaml",
        "tsconfig.build.json",
        "babel.config.cjs",
    ];
    for p in paths {
        let scan = found(&[created(p)], &[], &[]);
        assert_eq!(scan.changes.len(), 1, "{p}");
        assert_eq!(scan.changes[0].kind, SubstrateKind::Created, "{p}");
        assert!(scan.changes[0].describe().starts_with(p), "{p}");
    }
}

#[test]
fn ordinary_files_are_not_harness() {
    for p in [
        "src/lib.rs",
        "tests/visible.rs",
        "README.md",
        "src/conftest_helpers.py",
    ] {
        assert!(found(&[changed(p)], &[], &[]).changes.is_empty(), "{p}");
    }
}

#[test]
fn deleted_moved_and_changed_harness_files_count_too() {
    let moved = FileDiff {
        path: "ci/Makefile.old".into(),
        renamed_from: Some("Makefile".into()),
        ..FileDiff::default()
    };
    let scan = found(
        &[deleted("pytest.ini"), changed("tox.ini"), moved],
        &[],
        &[],
    );
    let kinds: Vec<(&str, SubstrateKind)> = scan
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.kind))
        .collect();
    assert!(
        kinds.contains(&("pytest.ini", SubstrateKind::Deleted)),
        "{kinds:?}"
    );
    assert!(kinds.contains(&("tox.ini", SubstrateKind::Modified)));
    assert!(kinds.contains(&("Makefile", SubstrateKind::MovedAway)));
}

#[test]
fn the_repository_can_extend_the_list_and_allow_a_change() {
    let extra = vec!["scripts/ci/**".to_string()];
    let scan = found(&[changed("scripts/ci/run.sh")], &extra, &[]);
    assert_eq!(scan.changes.len(), 1);
    assert!(
        scan.changes[0].why.contains("gates.toml"),
        "{:?}",
        scan.changes[0]
    );

    let allow = vec!["scripts/ci/**".to_string(), "conftest.py".to_string()];
    let scan = found(
        &[changed("scripts/ci/run.sh"), created("conftest.py")],
        &extra,
        &allow,
    );
    assert!(scan.changes.is_empty(), "{:?}", scan.changes);
}

#[test]
fn swarmux_own_folder_is_left_to_the_pins() {
    let scan = found(
        &[changed(".swarmux/gates.toml")],
        &[".swarmux/**".into()],
        &[],
    );
    assert!(scan.changes.is_empty());
}

#[test]
fn a_check_program_that_is_a_repository_path_is_harness() {
    let check = GateCheck::new("t", "./scripts/test.sh", &[]);
    assert_eq!(
        check_program_paths(std::slice::from_ref(&check)),
        ["scripts/test.sh"]
    );
    let in_web = check.clone().in_dir("web");
    assert_eq!(check_program_paths(&[in_web]), ["web/scripts/test.sh"]);
    let globs = substrate_globs(&[], &[check]);
    let s = scan(
        &[changed("scripts/test.sh")],
        &globs,
        &[],
        &none,
        &none,
        false,
    );
    assert_eq!(s.changes.len(), 1);
    // A program on the PATH, an absolute path or a path that climbs out is not.
    for p in ["cargo", "/usr/bin/make", "C:\\x\\y.exe", "../other/run.sh"] {
        assert!(
            check_program_paths(&[GateCheck::new("t", p, &[])]).is_empty(),
            "{p}"
        );
    }
}

#[test]
fn a_manifest_that_only_gained_a_dependency_is_not_harness() {
    let old = |_: &str| {
        Some(
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n"
                .to_string(),
        )
    };
    let new = |_: &str| {
        Some(
            "[package]\nname = \"a\"\nversion = \"0.1.1\"\n\n[dependencies]\nserde = \"1\"\nleftpad = \"2\"\n"
                .to_string(),
        )
    };
    let globs = substrate_globs(&[], &[]);
    let s = scan(&[changed("Cargo.toml")], &globs, &[], &old, &new, false);
    assert!(s.changes.is_empty(), "{:?}", s.changes);
    assert_eq!(s.dependencies, ["Cargo.toml: leftpad"]);

    // A [patch] section in the same file is.
    let patched = |_: &str| {
        Some(
            "[package]\nname = \"a\"\n\n[dependencies]\nserde = \"1\"\n\n[patch.crates-io]\nserde = { path = \"../evil\" }\n"
                .to_string(),
        )
    };
    let s = scan(&[changed("Cargo.toml")], &globs, &[], &old, &patched, false);
    assert_eq!(s.changes.len(), 1);
    assert!(s.changes[0].why.contains("[patch]"), "{:?}", s.changes[0]);
}

#[test]
fn dependencies_are_listed_even_when_the_manifest_change_is_allowed() {
    let old = |_: &str| Some("{\"scripts\":{\"test\":\"jest\"}}".to_string());
    let new = |_: &str| {
        Some("{\"scripts\":{\"test\":\"true\"},\"dependencies\":{\"x\":\"1\"}}".to_string())
    };
    let globs = substrate_globs(&[], &[]);
    let allow = vec!["package.json".to_string()];
    let s = scan(
        &[changed("package.json")],
        &globs,
        &allow,
        &old,
        &new,
        false,
    );
    assert!(s.changes.is_empty());
    assert_eq!(s.dependencies, ["package.json: x"]);
}

#[test]
fn a_manifest_without_content_to_compare_is_harness() {
    let s = found(&[changed("Cargo.toml")], &[], &[]);
    assert_eq!(s.changes.len(), 1);
    assert!(s.changes[0].describe().contains("dependencies, targets"));
}

#[test]
fn lockfile_gains_are_listed_by_package() {
    let lock = FileDiff {
        path: "Cargo.lock".into(),
        added: vec![
            "[[package]]".into(),
            "name = \"leftpad\"".into(),
            "version = \"2.0.0\"".into(),
        ],
        removed: vec!["version = \"1.0.0\"".into()],
        ..FileDiff::default()
    };
    let s = found(&[lock], &[], &[]);
    assert!(s.changes.is_empty());
    assert_eq!(s.dependencies, ["Cargo.lock: leftpad"]);
}
