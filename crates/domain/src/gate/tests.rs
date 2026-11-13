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

// --- X2: a gate that can say it could not vouch -----------------------------

fn passing_run() -> CheckRun {
    run("ok", Some(0))
}

fn clean_findings() -> GateFindings {
    GateFindings {
        runs: vec![passing_run()],
        ran_in: Some(RanIn::CleanCopy),
        ..GateFindings::default()
    }
}

#[test]
fn a_clean_run_in_a_clean_copy_passes() {
    let f = clean_findings();
    assert!(f.clean());
    assert!(f.incomplete().is_empty());
    assert_eq!(judge(&f, 0, 3), GateOutcome::Pass);
}

#[test]
fn a_gate_that_could_not_read_its_file_escalates_and_never_passes() {
    let mut f = clean_findings();
    f.config_problem = Some(".swarmux/gates.toml: expected an equals sign".into());
    assert!(f.clean(), "nothing is wrong with the change");
    assert_eq!(f.incomplete().len(), 1);
    // Not a block (the agent cannot fix the file), and not a pass.
    assert_eq!(judge(&f, 0, 3), GateOutcome::Escalate);
    assert_eq!(judge(&f, 0, 0), GateOutcome::Escalate);
    let d = digest(&f, GateOutcome::Escalate, 0, 3, &[]);
    assert!(d.contains("cannot vouch"), "{d}");
    assert!(d.contains("gates.toml could not be used"), "{d}");
    // Attended or not makes no difference for this one.
    f.unattended = true;
    assert_eq!(judge(&f, 0, 3), GateOutcome::Escalate);
}

#[test]
fn what_a_mission_cannot_be_passed_without_is_not_asked_of_an_attended_stop() {
    let mut f = clean_findings();
    f.hidden_not_run = Some(HiddenNotRun {
        planned: 2,
        reason: "no clean copy was made (git broke)".into(),
    });
    f.ran_in = Some(RanIn::Worktree);
    f.unscanned = Some("the change could not be read (no commit)".into());
    // Somebody is watching: it passes, and says what did not happen.
    assert_eq!(judge(&f, 0, 3), GateOutcome::Pass);
    let d = digest(&f, GateOutcome::Pass, 0, 3, &[]);
    assert!(d.contains("hidden checks: not run (no clean copy"), "{d}");
    assert!(d.contains("ran in the worktree"), "{d}");
    assert!(d.contains("not scanned for tampering"), "{d}");
    assert!(d.starts_with("swarmux gate: 1 check passed"), "{d}");
    // Nobody is: Escalate, with each reason.
    f.unattended = true;
    assert_eq!(f.incomplete().len(), 3, "{:?}", f.incomplete());
    assert_eq!(judge(&f, 0, 3), GateOutcome::Escalate);
    let d = digest(&f, GateOutcome::Escalate, 0, 3, &[]);
    assert!(d.contains("cannot vouch"), "{d}");
    assert!(d.contains("hidden checks: not run"), "{d}");
    // A repository that accepts worktree runs is believed about that only.
    f.worktree_allowed = true;
    assert_eq!(f.incomplete().len(), 2);
}

#[test]
fn harness_findings_block_and_say_which_file() {
    let mut f = clean_findings();
    f.substrate =
        vec!["conftest.py: harness file created (pytest runs it before every test)".into()];
    assert!(!f.clean());
    assert_eq!(judge(&f, 0, 3), GateOutcome::Block);
    assert_eq!(judge(&f, 3, 3), GateOutcome::Escalate);
    let d = digest(&f, GateOutcome::Block, 0, 3, &[]);
    assert!(d.contains("test harness changed: conftest.py"), "{d}");
    assert!(d.contains("put them back"), "{d}");
}

#[test]
fn dependencies_are_listed_and_block_only_when_asked() {
    let mut f = clean_findings();
    f.dependencies = vec!["Cargo.lock: leftpad".into()];
    assert!(f.clean());
    let d = digest(&f, GateOutcome::Pass, 0, 3, &[]);
    assert!(
        d.contains("new or changed dependencies: Cargo.lock: leftpad"),
        "{d}"
    );
    f.block_dependencies = true;
    assert!(!f.clean());
    let d = digest(&f, GateOutcome::Block, 0, 3, &[]);
    assert!(d.contains("A person has to look at these first"), "{d}");
    assert_eq!(d.matches("leftpad").count(), 1, "told once: {d}");
}

#[test]
fn an_optional_check_is_reported_and_blocks_nothing() {
    let mut f = GateFindings {
        runs: vec![run("assertion failed", Some(1))],
        optional: vec!["cargo test".into()],
        ran_in: Some(RanIn::CleanCopy),
        ..GateFindings::default()
    };
    assert!(f.clean());
    assert_eq!(f.failed().count(), 0);
    assert_eq!(f.failed_optional().count(), 1);
    assert_eq!(judge(&f, 0, 3), GateOutcome::Pass);
    f.optional.clear();
    assert_eq!(judge(&f, 0, 3), GateOutcome::Block);
}

#[test]
fn a_revert_probe_that_proves_nothing_blocks() {
    let mut f = clean_findings();
    f.revert_probe = Some(RevertProbe {
        proved: true,
        detail: "1 file reverted; cargo test fail without the change".into(),
    });
    assert!(f.clean());
    assert!(digest(&f, GateOutcome::Pass, 0, 3, &[]).contains("revert probe: 1 file reverted"));
    f.revert_probe = Some(RevertProbe {
        proved: false,
        detail: "1 file reverted and every check still passes".into(),
    });
    assert!(!f.clean());
    let d = digest(&f, GateOutcome::Block, 0, 3, &[]);
    assert!(
        d.contains("revert probe: 1 file reverted and every check still passes"),
        "{d}"
    );
}

#[test]
fn levels_decide_which_checks_run_and_the_rest_are_named() {
    let mut fast = GateCheck::new("lint", "cargo", &["clippy"]);
    fast.level = GateLevel::Fast;
    let full = GateCheck::new("unit", "cargo", &["test"]);
    let mut deep = GateCheck::new("soak", "cargo", &["test", "--ignored"]);
    deep.level = GateLevel::Deep;
    let mut spec = GateSpec {
        checks: vec![fast, full, deep],
        ..GateSpec::default()
    };
    let names = |s: &GateSpec| -> (Vec<String>, Vec<String>) {
        let (run, left) = s.planned();
        (
            run.iter().map(|c| c.name.clone()).collect(),
            left.iter().map(|c| c.name.clone()).collect(),
        )
    };
    assert_eq!(
        names(&spec),
        (vec!["lint".into(), "unit".into()], vec!["soak".into()])
    );
    spec.level = GateLevel::Fast;
    assert_eq!(
        names(&spec),
        (vec!["lint".into()], vec!["unit".into(), "soak".into()])
    );
    spec.level = GateLevel::Deep;
    assert_eq!(names(&spec).1, Vec::<String>::new());
    assert_eq!(GateLevel::parse("deep"), Some(GateLevel::Deep));
    assert_eq!(GateLevel::parse("slow"), None);
    assert!(GateLevel::Fast < GateLevel::Full && GateLevel::Full < GateLevel::Deep);
}

#[test]
fn the_policy_hash_changes_with_anything_that_decides_a_verdict() {
    let base = GateSpec {
        checks: vec![GateCheck::new("unit", "cargo", &["test"])],
        ..GateSpec::default()
    };
    let h = base.policy_sha256();
    assert_eq!(h.len(), 64);
    assert_eq!(h, base.clone().policy_sha256(), "stable");
    let changed = |f: &dyn Fn(&mut GateSpec)| {
        let mut s = base.clone();
        f(&mut s);
        assert_ne!(s.policy_sha256(), h);
    };
    changed(&|s| s.max_blocks = 5);
    changed(&|s| s.level = GateLevel::Deep);
    changed(&|s| s.checks[0].required = false);
    changed(&|s| s.checks[0].env.push("DATABASE_URL".into()));
    changed(&|s| s.verifier.revert_probe = true);
    changed(&|s| s.verifier.worktree_checks = WorktreeChecks::Allow);
    changed(&|s| s.verifier.dependencies = DependencyChanges::Block);
    changed(&|s| s.allow_substrate.push("Makefile".into()));
    changed(&|s| s.substrate.push("scripts/**".into()));
    changed(&|s| s.allow_test_changes.push("tests/old/**".into()));
}

#[test]
fn checks_see_their_tools_and_never_credentials() {
    for ok in [
        "PATH",
        "path",
        "HOME",
        "LC_ALL",
        "LC_CTYPE",
        "CARGO_HOME",
        "SystemRoot",
        "TEMP",
    ] {
        assert!(env_passes(ok, &[]), "{ok}");
    }
    for bad in [
        "SWARMUX_JOB_ID",
        "swarmux_hook_pipe",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "GITHUB_TOKEN",
        "NPM_TOKEN",
        "MY_SERVICE_TOKEN",
        "DB_PASSWORD",
        "STRIPE_API_KEY",
        "DATABASE_URL",
        "RUST_LOG",
    ] {
        assert!(!env_passes(bad, &[]), "{bad}");
    }
    // A check can ask for what is not a credential, and never for one.
    assert!(env_passes("DATABASE_URL", &["DATABASE_URL".into()]));
    assert!(env_passes("database_url", &["DATABASE_URL".into()]));
    assert!(!env_passes("SWARMUX_JOB_ID", &["SWARMUX_JOB_ID".into()]));
    assert!(!env_passes(
        "MY_SERVICE_TOKEN",
        &["MY_SERVICE_TOKEN".into()]
    ));
    assert!(env_denied("ANTHROPIC_BASE_URL"));
}
