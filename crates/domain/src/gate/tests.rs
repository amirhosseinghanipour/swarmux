use super::*;

fn facts(files: &[&str]) -> RepoFacts {
    RepoFacts {
        files: files.iter().map(|f| f.to_string()).collect(),
        ..RepoFacts::default()
    }
}

fn names(checks: &[GateCheck]) -> Vec<String> {
    checks.iter().map(GateCheck::command_line).collect()
}

#[test]
fn detects_each_ecosystem_from_its_marker_files() {
    assert_eq!(
        names(&detect(&facts(&["Cargo.toml", "README.md"]))),
        ["cargo test --workspace --quiet"]
    );
    assert_eq!(names(&detect(&facts(&["go.mod"]))), ["go test ./..."]);
    assert_eq!(names(&detect(&facts(&["pytest.ini"]))), ["pytest -q"]);
    let mut py = facts(&["pyproject.toml"]);
    assert!(detect(&py).is_empty(), "pyproject alone says nothing");
    py.pytest_configured = true;
    assert_eq!(names(&detect(&py)), ["pytest -q"]);

    let mut npm = facts(&["package.json"]);
    assert!(detect(&npm).is_empty(), "no test script");
    npm.npm_test = Some("echo \"Error: no test specified\" && exit 1".into());
    assert!(detect(&npm).is_empty(), "npm's placeholder always fails");
    npm.npm_test = Some("vitest run".into());
    assert_eq!(names(&detect(&npm)), ["npm test"]);
    npm.files.push("pnpm-lock.yaml".into());
    assert_eq!(names(&detect(&npm)), ["pnpm test"]);

    assert!(detect(&facts(&["notes.txt"])).is_empty());
    let all = detect(&facts(&["Cargo.toml", "go.mod", "conftest.py"]));
    assert_eq!(all.len(), 3);
    assert!(
        all.iter()
            .all(|c| c.timeout_secs == DEFAULT_CHECK_TIMEOUT_SECS)
    );
}

#[test]
fn command_lines_quote_what_needs_it() {
    let c = GateCheck::new("x", "node", &["-e", "console.log(\"a b\")", ""]);
    assert_eq!(c.command_line(), r#"node -e "console.log(\"a b\")" """#);
}

fn run(output: &str, exit: Option<i32>) -> CheckRun {
    CheckRun {
        name: "cargo test".into(),
        command: "cargo test --workspace --quiet".into(),
        exit,
        timed_out: false,
        spawn_error: None,
        duration_ms: 4200,
        output: output.into(),
        flaky: false,
    }
}

const CARGO_FAIL: &str = "\
running 2 tests
test parse::ok ... ok
test parse::answer ... FAILED

failures:

---- parse::answer stdout ----

thread 'parse::answer' panicked at src/parse.rs:12:5:
assertion `left == right` failed
  left: 41
 right: 42

test result: FAILED. 1 passed; 1 failed";

#[test]
fn the_excerpt_starts_at_the_assertion_not_the_summary() {
    let r = run(CARGO_FAIL, Some(101));
    let ex = r.failure_excerpt();
    assert_eq!(
        ex[0],
        "thread 'parse::answer' panicked at src/parse.rs:12:5:"
    );
    assert_eq!(ex[1], "assertion `left == right` failed");
    assert!(ex.len() <= EXCERPT_LINES);
    assert_eq!(r.status_text(), "exited 101 after 4.2 s");
    // Nothing recognisable: the last lines.
    let plain = run("one\ntwo\n\n", Some(1));
    assert_eq!(plain.failure_excerpt(), ["one", "two"]);
}

#[test]
fn a_run_passes_only_with_exit_zero_in_time() {
    assert!(run("", Some(0)).passed());
    assert!(!run("", Some(1)).passed());
    assert!(!run("", None).passed());
    let mut slow = run("", Some(0));
    slow.timed_out = true;
    assert!(!slow.passed());
    assert_eq!(slow.status_text(), "timed out after 4 s");
    let mut missing = run("", None);
    missing.spawn_error = Some("program not found".into());
    assert_eq!(missing.status_text(), "could not start: program not found");
}

fn pin(path: &str, hash: &str) -> Pin {
    Pin {
        path: path.into(),
        sha256: hash.into(),
    }
}

#[test]
fn pin_changes_by_path() {
    let before = [pin("t/a", "1"), pin("t/b", "2"), pin("t/c", "3")];
    let after = [pin("t/a", "1"), pin("t/b", "9"), pin("t/d", "4")];
    assert_eq!(
        pin_changes(&before, &after),
        [
            PinChange::Changed("t/b".into()),
            PinChange::Removed("t/c".into()),
            PinChange::Added("t/d".into()),
        ]
    );
    assert!(pin_changes(&before, &before).is_empty());
}

fn globs() -> Vec<String> {
    GateSpec {
        protected: vec!["tests/oracles/**".into(), "golden.json".into()],
        ..GateSpec::default()
    }
    .protected_globs()
}

#[test]
fn protected_globs_always_cover_the_gate_itself() {
    let g = GateSpec::default().protected_globs();
    assert_eq!(g, [ALWAYS_PROTECTED]);
    assert_eq!(globs().len(), 3);
}

#[test]
fn edits_of_protected_paths_are_refused() {
    let g = globs();
    let root = "C:/work/repo";
    let w = |tool: &str, path: &str| protected_write(&g, root, tool, path, root);
    assert_eq!(
        w("Edit", "C:/work/repo/tests/oracles/parse.rs").as_deref(),
        Some("tests/oracles/**")
    );
    assert_eq!(
        w("Write", r"C:\Work\Repo\tests\oracles\new.rs").as_deref(),
        Some("tests/oracles/**"),
        "Windows paths fold case"
    );
    assert_eq!(
        w("MultiEdit", "tests/oracles/x").as_deref(),
        Some("tests/oracles/**")
    );
    assert_eq!(w("Edit", "sub/golden.json").as_deref(), Some("golden.json"));
    assert_eq!(
        w("Edit", ".swarmux/gates.toml").as_deref(),
        Some(ALWAYS_PROTECTED)
    );
    assert_eq!(w("Edit", "src/parse.rs"), None);
    assert_eq!(w("Edit", "tests/other.rs"), None);
    assert_eq!(w("Read", "tests/oracles/parse.rs"), None, "reading is fine");
    assert_eq!(
        w("Edit", "C:/elsewhere/tests/oracles/a"),
        None,
        "another repo"
    );
    // From a subfolder of the repository.
    assert_eq!(
        protected_write(&g, root, "Edit", "../tests/oracles/a", "C:/work/repo/src").as_deref(),
        Some("tests/oracles/**")
    );
}

#[test]
fn shell_writes_into_protected_paths_are_refused() {
    let g = globs();
    let root = "/w";
    let sh = |cmd: &str| protected_write(&g, root, "Bash", cmd, root);
    for cmd in [
        "echo 41 > tests/oracles/expected.txt",
        "echo 41 >tests/oracles/expected.txt",
        "cat a >> tests/oracles/expected.txt",
        "rm tests/oracles/expected.txt",
        "cd /w && rm -f tests/oracles/expected.txt",
        "sed -i 's/42/41/' tests/oracles/expected.txt",
        "cp /tmp/x tests/oracles/expected.txt",
        "mv tests/oracles/a tests/oracles/b",
        "git rm tests/oracles/a",
        "sudo tee tests/oracles/a",
        "printf x | tee 'tests/oracles/a'",
        "rm .swarmux/gates.toml",
    ] {
        assert!(sh(cmd).is_some(), "{cmd}");
    }
    for cmd in [
        "cat tests/oracles/expected.txt",
        "pytest tests/oracles/test_parse.py",
        "cp tests/oracles/expected.txt /tmp/copy",
        "sed 's/a/b/' tests/oracles/expected.txt",
        "git diff tests/oracles",
        "git checkout -- tests/oracles/expected.txt",
        "cargo test 2>&1",
        "echo hi > out.txt",
        "rm -rf target",
    ] {
        assert_eq!(sh(cmd), None, "{cmd}");
    }
    assert!(
        protected_write(
            &g,
            root,
            "PowerShell",
            "Set-Content tests/oracles/a 41",
            root
        )
        .is_some()
    );
    assert_eq!(
        protected_write(&g, root, "PowerShell", "Get-Content tests/oracles/a", root),
        None
    );
}

fn ran(command: &str, ok: bool) -> Executed {
    Executed {
        command: command.into(),
        ok,
    }
}

#[test]
fn claims_without_runs_are_gaps() {
    let gaps = evidence_gaps("Done. All tests pass.", &[], &[]);
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].contains("no test run succeeded"), "{}", gaps[0]);

    let gaps = evidence_gaps("I ran `cargo test` and it is green.", &[], &[]);
    assert_eq!(
        gaps,
        ["you cite `cargo test`, but it never ran in this session"]
    );
}

#[test]
fn claims_backed_by_a_successful_latest_run_are_fine() {
    let executed = [
        ran("cargo build", true),
        ran("cd crates/x && RUST_BACKTRACE=1 cargo test -q", true),
    ];
    assert!(evidence_gaps("All tests pass.", &executed, &[]).is_empty());
    assert!(evidence_gaps("`cargo test --quiet` passes.", &executed, &[]).is_empty());
    // The latest run is what counts.
    let fixed = [ran("cargo test", false), ran("cargo test", true)];
    assert!(evidence_gaps("cargo test passes", &fixed, &[]).is_empty());
    let broke = [ran("cargo test", true), ran("cargo test", false)];
    assert_eq!(
        evidence_gaps("cargo test passes", &broke, &[]),
        ["you cite `cargo test`, but its latest run in this session failed"]
    );
}

#[test]
fn configured_checks_count_as_runners() {
    let check = GateCheck::new("answer", "git", &["diff", "--no-index", "a", "b"]);
    let executed = [ran("git diff --no-index a b", true)];
    assert!(evidence_gaps("All tests pass.", &executed, std::slice::from_ref(&check)).is_empty());
    assert!(
        !evidence_gaps("All tests pass.", &[ran("ls", true)], &[check]).is_empty(),
        "ls is no test run"
    );
}

#[test]
fn negated_or_absent_claims_are_not_gaps() {
    for msg in [
        "I could not run the tests.",
        "Tests don't pass yet; the parser is half done.",
        "Refactored the parser.",
        "I didn't run cargo test because it needs a database.",
    ] {
        assert!(evidence_gaps(msg, &[], &[]).is_empty(), "{msg}");
    }
}

fn findings(runs: Vec<CheckRun>) -> GateFindings {
    GateFindings {
        runs,
        ..GateFindings::default()
    }
}

#[test]
fn judge_blocks_up_to_the_limit_then_escalates() {
    let bad = findings(vec![run(CARGO_FAIL, Some(101))]);
    assert_eq!(judge(&bad, 0, 3), GateOutcome::Block);
    assert_eq!(judge(&bad, 2, 3), GateOutcome::Block);
    assert_eq!(judge(&bad, 3, 3), GateOutcome::Escalate);
    assert_eq!(judge(&bad, 0, 0), GateOutcome::Escalate, "0 never blocks");
    let good = findings(vec![run("ok", Some(0))]);
    assert_eq!(judge(&good, 3, 3), GateOutcome::Pass);
    let mut pinned = good.clone();
    pinned
        .pins
        .push(PinChange::Changed("tests/oracles/a".into()));
    assert_eq!(judge(&pinned, 0, 3), GateOutcome::Block);
    let mut claimed = good;
    claimed.gaps.push("x".into());
    assert_eq!(judge(&claimed, 0, 3), GateOutcome::Block);
}

#[test]
fn the_digest_names_the_failure_verbatim() {
    let mut f = findings(vec![run(CARGO_FAIL, Some(101)), run("fine", Some(0))]);
    f.pins.push(PinChange::Changed("tests/oracles/a".into()));
    f.gaps
        .push("you cite `cargo test`, but it never ran in this session".into());
    let d = digest(&f, GateOutcome::Block, 0, 3, &globs());
    assert!(
        d.starts_with("swarmux gate: not done yet (block 1 of 3)."),
        "{d}"
    );
    assert!(d.contains("✗ cargo test: `cargo test --workspace --quiet` exited 101 after 4.2 s"));
    assert!(d.contains("    thread 'parse::answer' panicked at src/parse.rs:12:5:"));
    assert!(d.contains("⛔ protected file changed: tests/oracles/a."));
    assert!(d.contains("⚑ you cite `cargo test`, but it never ran in this session."));
    assert!(
        d.ends_with("Protected (writes are refused): .swarmux/**, tests/oracles/**, golden.json")
    );
    assert_eq!(d.matches("✗").count(), 1, "only failures are listed");
    let e = digest(&f, GateOutcome::Escalate, 3, 3, &globs());
    assert!(e.starts_with("swarmux gate: still failing after 3 blocks."));
    assert!(!e.contains("Protected (writes"));
    let huge = findings(vec![run(&"panicked at x\n".repeat(5000), Some(1)); 40]);
    assert!(digest(&huge, GateOutcome::Block, 0, 3, &[]).chars().count() <= DIGEST_MAX + 1);
    assert_eq!(
        digest(
            &findings(vec![run("", Some(0))]),
            GateOutcome::Pass,
            0,
            3,
            &[]
        ),
        "swarmux gate: 1 check passed"
    );
}

#[test]
fn search_roots_are_the_literal_starts() {
    let g = |v: &[&str]| search_roots(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        g(&[".swarmux/**", "tests/oracles/**", "tests/oracles/deep/*.rs"]),
        [".swarmux", "tests/oracles"]
    );
    assert_eq!(
        g(&["tests/golden.json", "fixtures/"]),
        ["fixtures", "tests"]
    );
    assert_eq!(
        g(&[".swarmux/**", "golden.json"]),
        [""],
        "a bare name is anywhere"
    );
    assert_eq!(g(&["src/*/snap/**"]), ["src"]);
}

#[test]
fn hidden_checks_and_tampering_block_without_saying_what_they_are() {
    let mut f = GateFindings {
        hidden: Some(HiddenTally { ran: 3, passed: 2 }),
        ..GateFindings::default()
    };
    assert!(!f.clean());
    assert!(f.checked());
    let d = digest(&f, GateOutcome::Block, 0, 3, &[]);
    assert!(d.contains("hidden checks: 1 of 3 failed"), "{d}");
    assert!(d.contains("kept from you"), "{d}");

    f.hidden = Some(HiddenTally { ran: 3, passed: 3 });
    assert!(f.clean());
    assert_eq!(
        digest(&f, GateOutcome::Pass, 0, 3, &[]),
        "swarmux gate: 0 checks passed, 3 hidden"
    );

    f.tampering = vec!["tests/a.rs: a test was switched off (`#[ignore]`)".into()];
    assert!(!f.clean());
    let d = digest(&f, GateOutcome::Block, 0, 3, &[]);
    assert!(d.contains("Tests are the referee"), "{d}");
    assert!(!GateFindings::default().checked());
}
