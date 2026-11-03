//! Seeded mission files, read from disk and linted: the lint must flag
//! missing acceptance and oracles an agent could edit, and nothing else.

use std::fs;
use std::rc::Rc;

use swarmux_application::MissionStore;
use swarmux_domain::mission::{FindingKind, Mission, Severity, compile, lint};
use swarmux_infrastructure::mission::{contract_path, parse_contract};
use swarmux_infrastructure::{MissionFiles, SqliteStore};

/// (name, file, expected findings, blocking)
const SEEDED: [(&str, &str, &[FindingKind], bool); 5] = [
    (
        "no acceptance",
        r#"
title = "Faster app"
goal = "make the app start in under one second"
[constraints]
paths_rw = ["src/**"]
[budget]
usd = 3.0
"#,
        &[FindingKind::MissingAcceptance],
        true,
    ),
    (
        "tests the agent could edit",
        r#"
goal = "make the parser accept trailing commas"
[acceptance]
checks = ["cargo test -p parser"]
[constraints]
paths_rw = ["crates/parser/src/**"]
[budget]
usd = 1.0
"#,
        &[FindingKind::UnprotectedOracle],
        false,
    ),
    (
        "a script oracle left open",
        r#"
goal = "keep the flaky rate on main under half a percent"
[acceptance]
checks = ["cargo test --workspace", "sh scripts/flake_rate.sh --days 7"]
[constraints]
paths_rw = ["src/**"]
protected = ["tests/**"]
[budget]
wall_minutes = 120
"#,
        &[FindingKind::UnprotectedOracle],
        false,
    ),
    (
        "go tests protected by a bare name",
        r#"
goal = "return 404 for unknown ids in the users handler"
[acceptance]
checks = ["go test ./..."]
[constraints]
paths_rw = ["internal/users/**"]
protected = ["*_test.go"]
[budget]
usd = 0.5
"#,
        &[],
        false,
    ),
    (
        "writable and protected at once",
        r#"
goal = "update the golden files for the new renderer output"
[acceptance]
checks = ["cargo test -p render"]
[constraints]
paths_rw = ["tests/**", "src/**"]
protected = ["tests/**"]
[budget]
usd = 0.5
"#,
        &[FindingKind::Conflict],
        true,
    ),
];

#[test]
fn seeded_missions_are_linted_from_their_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (i, (name, text, expected, blocking)) in SEEDED.iter().enumerate() {
        let path = contract_path(root, &format!("seed-{i}"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let contract = parse_contract(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let findings = lint(&contract);
        let kinds: Vec<FindingKind> = findings
            .iter()
            .filter(|f| f.severity != Severity::Info)
            .map(|f| f.kind)
            .collect();
        assert_eq!(kinds, *expected, "{name}: {findings:?}");
        let is_blocking = findings.iter().any(|f| f.severity == Severity::Blocking);
        assert_eq!(is_blocking, *blocking, "{name}");
        assert_eq!(
            compile(&contract).is_err(),
            *blocking,
            "{name}: compile agrees"
        );
        if kinds.contains(&FindingKind::UnprotectedOracle) {
            assert!(
                findings.iter().any(|f| f.protect.is_some()),
                "{name}: the fix names what to protect"
            );
        }
    }
}

#[test]
fn missions_are_mirrored_in_the_database_and_written_to_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let db = Rc::new(SqliteStore::open(&dir.path().join("swarmux.db")).unwrap());
    let files = MissionFiles::new(db.clone());
    let contract = parse_contract(SEEDED[1].1).unwrap();
    let root = dir.path().display().to_string();
    let m = Mission::new("trailing-commas", &root, contract.clone(), 5);
    files.save(&m).unwrap();
    let written = fs::read_to_string(contract_path(dir.path(), "trailing-commas")).unwrap();
    assert_eq!(parse_contract(&written).unwrap(), contract);
    // A fresh connection reads the same mission back.
    let again = SqliteStore::open(&dir.path().join("swarmux.db")).unwrap();
    assert_eq!(again.list().unwrap(), [m]);
}
