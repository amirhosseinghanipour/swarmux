//! `swarmux gate`: shows a repository's done-gate, runs its checks, lints
//! its `gates.toml` and tests that the gate says no.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Context;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::Clock;
use swarmux_application::{
    GateRecord, GateSource, LoadedGate, Verifier, VerifyRequest, manual_outcome, plan_checks,
    run_checks,
};
use swarmux_domain::gate::{
    CheckRun, DependencyChanges, GateFindings, GateLevel, GateOutcome, WorktreeChecks, digest,
};
use swarmux_infrastructure::gate::GATES_FILE;

use crate::holdout;
use swarmux_infrastructure::{FsGateSource, Paths, ProcessChecker, SqliteLedger, SystemClock};

fn load(dir: Option<PathBuf>) -> anyhow::Result<LoadedGate> {
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir().context("cannot read the current folder")?,
    };
    anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
    Ok(FsGateSource::new().load(&dir))
}

/// The level named on the command line, if any.
fn with_level(mut gate: LoadedGate, level: Option<&str>) -> anyhow::Result<LoadedGate> {
    if let Some(word) = level {
        gate.spec.level = GateLevel::parse(word)
            .with_context(|| format!("--level is fast, full or deep, not {word:?}"))?;
    }
    Ok(gate)
}

/// Prints the gate that applies to `dir` (default: the current folder).
pub fn show(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let gate = load(dir)?;
    let spec = &gate.spec;
    println!("root        {}", gate.root.display());
    println!(
        "from        {}",
        match gate.origin {
            swarmux_domain::gate::GateOrigin::File => GATES_FILE.to_string(),
            swarmux_domain::gate::GateOrigin::Detected => format!("detected (no {GATES_FILE})"),
        }
    );
    if let Some(p) = &gate.problem {
        println!("problem     {p}");
        println!("            until it is fixed the gate cannot pass: its verdict is escalate");
    }
    for w in &spec.warnings {
        println!("warning     {w}");
    }
    println!("max blocks  {}", spec.max_blocks);
    println!("level       {}", spec.level.as_str());
    println!("evidence    {}", if spec.evidence { "on" } else { "off" });
    println!(
        "subagents   {}",
        if spec.subagent_checks {
            "run the checks"
        } else {
            "protected files and evidence only"
        }
    );
    println!("protected   {}", spec.protected_globs().join(", "));
    let v = &spec.verifier;
    println!(
        "verifier    clean copy {}, test edits {}, base suite {}, revert probe {}",
        if v.clean { "on" } else { "off" },
        if v.block_test_edits { "block" } else { "allow" },
        if v.base_suite { "on" } else { "off" },
        if v.revert_probe { "on" } else { "off" },
    );
    println!(
        "            checks that ran in the worktree: {}; new dependencies: {}",
        match v.worktree_checks {
            WorktreeChecks::Escalate => "a mission escalates",
            WorktreeChecks::Allow => "allowed",
        },
        match v.dependencies {
            DependencyChanges::Flag => "listed",
            DependencyChanges::Block => "block",
        }
    );
    if !spec.substrate.is_empty() {
        println!("harness     also {}", spec.substrate.join(", "));
    }
    if !spec.allow_substrate.is_empty() {
        println!(
            "            allowed to change: {}",
            spec.allow_substrate.join(", ")
        );
    }
    if spec.checks.is_empty() {
        println!("checks      none");
    }
    for c in &spec.checks {
        println!(
            "check       {:<14} {}  (timeout {} s, {}, level {}{})",
            c.name,
            c.command_line(),
            c.timeout_secs,
            if c.required { "required" } else { "optional" },
            c.level.as_str(),
            if c.env.is_empty() {
                String::new()
            } else {
                format!(", env {}", c.env.join(" "))
            }
        );
    }
    println!("policy      {}", spec.policy_sha256());
    Ok(())
}

/// Lints the gate of `dir`: what is wrong with `gates.toml` and what was
/// read with a fallback. Whether it is clean (and with `strict`, free of
/// warnings).
pub fn lint(dir: Option<PathBuf>, strict: bool) -> anyhow::Result<bool> {
    let gate = load(dir)?;
    let mut clean = true;
    match &gate.problem {
        Some(p) => {
            println!("✗ {p}");
            println!("  The gate cannot pass until this is fixed: every verdict is escalate.");
            clean = false;
        }
        None => println!("✓ {} reads", gate.origin.as_str()),
    }
    for w in &gate.spec.warnings {
        println!("! {w}");
        clean &= !strict;
    }
    Ok(clean)
}

/// Runs the checks of `dir`'s gate, prints each result and records the run
/// in the ledger. Returns whether the gate passed.
///
/// In a git repository the checks run in a clean copy of the work, with
/// the hidden checks after them and the tamper scan, as the gate runs
/// them when an agent stops; `in_place` runs them in the folder itself.
pub fn run(dir: Option<PathBuf>, in_place: bool, level: Option<&str>) -> anyhow::Result<bool> {
    let gate = with_level(load(dir)?, level)?;
    let paths = Paths::default_home()?;
    let verifier = holdout::verifier(&paths, holdout::checker());
    let clean = !in_place && gate.spec.verifier.clean;
    if clean && verifier.applies(&gate.root, &gate.spec, true) {
        return run_clean(&gate, &verifier);
    }
    let (planned, left_out, optional) = plan_checks(&gate.spec);
    if planned.is_empty() && gate.problem.is_none() {
        println!(
            "no checks for {}: none detected and no {GATES_FILE}",
            gate.root.display()
        );
        return Ok(true);
    }
    let checker = ProcessChecker::new();
    let mut runs = Vec::new();
    for check in &planned {
        println!("… {}  {}", check.name, check.command_line());
        let run = run_checks(&checker, std::slice::from_ref(check), &gate.root)
            .into_iter()
            .next()
            .context("the check produced no result")?;
        print_run(&run, optional.contains(&run.name));
        runs.push(run);
    }
    let hidden_not_run = verifier
        .hidden_planned(&gate.root)
        .filter(|_| !in_place)
        .map(|planned| swarmux_domain::gate::HiddenNotRun {
            planned,
            reason: "no clean copy: the checks ran in the folder itself".into(),
        });
    let findings = GateFindings {
        ran_in: (!runs.is_empty()).then_some(swarmux_domain::gate::RanIn::Worktree),
        runs,
        optional,
        left_out,
        hidden_not_run,
        planned: planned.iter().map(|c| c.name.clone()).collect(),
        // A person ran it and is watching: what ran in the folder counts.
        unattended: false,
        ..GateFindings::default()
    };
    finish(&gate, findings, None)
}

fn print_run(run: &CheckRun, optional: bool) {
    let mark = if run.passed() {
        "✓"
    } else if optional {
        "~"
    } else {
        "✗"
    };
    println!(
        "{mark} {}  {}{}",
        run.name,
        run.status_text(),
        if optional && !run.passed() {
            " (optional: does not block)"
        } else {
            ""
        }
    );
    if !run.passed() && run.spawn_error.is_none() {
        for line in run.failure_excerpt() {
            println!("    {line}");
        }
    }
}

/// The verifier's way: a clean copy, the visible checks, the hidden ones.
fn run_clean(gate: &LoadedGate, verifier: &Verifier) -> anyhow::Result<bool> {
    println!(
        "… in a clean copy of {} (what git holds there, committed or not)",
        gate.root.display()
    );
    let (planned, _, optional) = plan_checks(&gate.spec);
    for check in &planned {
        println!("… {}  {}", check.name, check.command_line());
    }
    let request = VerifyRequest {
        dir: &gate.root,
        spec: &gate.spec,
        base: None,
        visible: true,
        hidden: true,
    };
    let v = verifier
        .verify(&request)
        .map_err(|e| anyhow::anyhow!("no clean copy: {e}"))?;
    for run in &v.runs {
        print_run(run, optional.contains(&run.name));
    }
    if let Some(h) = v.hidden {
        let mark = if h.clean() { "✓" } else { "✗" };
        println!("{mark} hidden checks  {} passed", h.text());
    }
    if let Some(h) = &v.hidden_not_run {
        println!("✗ {}", h.text());
    }
    for change in &v.protected {
        println!("✗ protected file {}: {}", change.verb(), change.path());
    }
    for t in &v.tampering {
        println!("✗ {t}");
    }
    for s in &v.substrate {
        println!("✗ test harness changed: {s}");
    }
    for d in &v.dependencies {
        println!("! new or changed dependencies: {d}");
    }
    if let Some(b) = &v.base_suite {
        println!("{} {}", if b.clean() { "✓" } else { "✗" }, b.text());
    }
    if let Some(p) = &v.revert_probe {
        println!(
            "{} revert probe: {}",
            if p.proved { "✓" } else { "✗" },
            p.detail
        );
    }
    let problem = v.problem.clone();
    // A person ran it and is watching.
    let mut findings = v.into_findings(&gate.spec, false);
    findings.config_problem = gate.problem.clone();
    finish(gate, findings, problem)
}

fn finish(
    gate: &LoadedGate,
    mut findings: GateFindings,
    problem: Option<String>,
) -> anyhow::Result<bool> {
    findings.config_problem = gate.problem.clone();
    findings.policy_sha256 = gate.spec.policy_sha256();
    findings.warnings = gate.spec.warnings.clone();
    if let Some(p) = &problem {
        println!("! {p}");
    }
    for why in findings.incomplete() {
        println!("! {why}");
    }
    let outcome = manual_outcome(&findings);
    let ok = outcome == GateOutcome::Pass;
    record(gate, findings, outcome, problem)?;
    println!(
        "{}",
        match outcome {
            GateOutcome::Pass => "gate passed",
            GateOutcome::Escalate => "gate cannot pass this: escalate",
            GateOutcome::Block => "gate failed",
        }
    );
    Ok(ok)
}

fn record(
    gate: &LoadedGate,
    findings: GateFindings,
    outcome: GateOutcome,
    problem: Option<String>,
) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let clock = Rc::new(SystemClock);
    let ledger = LedgerService::new(Rc::new(SqliteLedger::open(&paths.db)?), clock.clone());
    let text = digest(&findings, outcome, 0, 0, &gate.spec.protected_globs());
    let record = GateRecord {
        job: None,
        session: String::new(),
        event: None,
        root: display(&gate.root),
        origin: gate.origin,
        outcome,
        blocks: 0,
        max_blocks: gate.spec.max_blocks,
        findings,
        digest: text,
        problem,
    };
    ledger.record_gate(&record, None, clock.now())?;
    Ok(())
}

/// Makes the changes an agent that wanted its work to count would make, in
/// scratch copies of the repository, and says whether the gate refused each.
/// The result goes into the ledger. Whether every one was refused.
pub fn selftest(dir: Option<PathBuf>) -> anyhow::Result<bool> {
    let gate = load(dir)?;
    let paths = Paths::default_home()?;
    let verifier = holdout::verifier(&paths, holdout::checker());
    if let Some(p) = &gate.problem {
        println!("! gates.toml could not be used ({p}): the probes are judged with the defaults");
    }
    println!(
        "… making known-bad changes in scratch copies of {}",
        gate.root.display()
    );
    let result = verifier
        .selftest(&gate.root, &gate.spec)
        .map_err(|e| anyhow::anyhow!("no scratch copy: {e}"))?;
    for p in &result.probes {
        println!(
            "{} {}\n    {}",
            if p.caught {
                "✓ refused "
            } else {
                "✗ ALLOWED "
            },
            p.name,
            p.detail
        );
    }
    for (name, why) in &result.skipped {
        println!("- skipped {name}: {why}");
    }
    let passed = result.passed();
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let clock = Rc::new(SystemClock);
    let ledger = LedgerService::new(Rc::new(SqliteLedger::open(&paths.db)?), clock.clone());
    let probes: Vec<(String, bool, String)> = result
        .probes
        .iter()
        .map(|p| (p.name.clone(), p.caught, p.detail.clone()))
        .collect();
    ledger.record_gate_selftest(
        &display(&gate.root),
        &probes,
        &result.skipped,
        &gate.spec.policy_sha256(),
        clock.now(),
    )?;
    println!(
        "{}",
        if passed {
            "gate selftest passed: every known-bad change was refused"
        } else {
            "gate selftest FAILED: the gate let a known-bad change through"
        }
    );
    Ok(passed)
}

fn display(p: &Path) -> String {
    p.display().to_string()
}
