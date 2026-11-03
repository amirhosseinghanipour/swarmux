//! `swarmux gate`: shows a repository's done-gate and runs its checks.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Context;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::Clock;
use swarmux_application::{
    GateRecord, GateSource, LoadedGate, Verifier, VerifyRequest, run_checks,
};
use swarmux_domain::gate::{CheckRun, GateFindings, GateOutcome, digest, judge};
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
    }
    println!("max blocks  {}", spec.max_blocks);
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
    if spec.checks.is_empty() {
        println!("checks      none");
    }
    for c in &spec.checks {
        println!(
            "check       {:<14} {}  (timeout {} s)",
            c.name,
            c.command_line(),
            c.timeout_secs
        );
    }
    Ok(())
}

/// Runs the checks of `dir`'s gate, prints each result and records the run
/// in the ledger. Returns whether every check passed.
///
/// In a git repository the checks run in a clean copy of the work, with
/// the hidden checks after them and the tamper scan, as the gate runs
/// them when an agent stops; `in_place` runs them in the folder itself.
pub fn run(dir: Option<PathBuf>, in_place: bool) -> anyhow::Result<bool> {
    let gate = load(dir)?;
    if let Some(p) = &gate.problem {
        println!("✗ {p}");
        return Ok(false);
    }
    let paths = Paths::default_home()?;
    let verifier = holdout::verifier(&paths, holdout::checker());
    let clean = !in_place && gate.spec.verifier.clean;
    if clean && verifier.applies(&gate.root, &gate.spec, true) {
        return run_clean(&gate, &verifier);
    }
    if gate.spec.checks.is_empty() {
        println!(
            "no checks for {}: none detected and no {GATES_FILE}",
            gate.root.display()
        );
        return Ok(true);
    }
    let checker = ProcessChecker::new();
    let mut runs = Vec::new();
    for check in &gate.spec.checks {
        println!("… {}  {}", check.name, check.command_line());
        let run = run_checks(&checker, std::slice::from_ref(check), &gate.root)
            .into_iter()
            .next()
            .context("the check produced no result")?;
        print_run(&run);
        runs.push(run);
    }
    let findings = GateFindings {
        runs,
        ..GateFindings::default()
    };
    finish(&gate, findings, None)
}

fn print_run(run: &CheckRun) {
    let mark = if run.passed() { "✓" } else { "✗" };
    println!("{mark} {}  {}", run.name, run.status_text());
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
    for check in &gate.spec.checks {
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
        print_run(run);
    }
    if let Some(h) = v.hidden {
        let mark = if h.clean() { "✓" } else { "✗" };
        println!("{mark} hidden checks  {} passed", h.text());
    }
    for change in &v.protected {
        println!("✗ protected file {}: {}", change.verb(), change.path());
    }
    for t in &v.tampering {
        println!("✗ {t}");
    }
    let findings = GateFindings {
        runs: v.runs,
        pins: v.protected,
        hidden: v.hidden,
        tampering: v.tampering,
        ..GateFindings::default()
    };
    finish(gate, findings, v.problem)
}

fn finish(
    gate: &LoadedGate,
    findings: GateFindings,
    problem: Option<String>,
) -> anyhow::Result<bool> {
    if let Some(p) = &problem {
        println!("! {p}");
    }
    let outcome = judge(&findings, 0, 0);
    let ok = outcome == GateOutcome::Pass;
    record(gate, findings, outcome, problem)?;
    println!("{}", if ok { "gate passed" } else { "gate failed" });
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

fn display(p: &Path) -> String {
    p.display().to_string()
}
