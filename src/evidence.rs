//! Evidence bundles: the service the app runs with, and the commands
//! `swarmux verify`, `swarmux brief` and `swarmux labels`.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::Clock;
use swarmux_application::{
    CheckpointService, EvidenceService, HistoryService, MissionStore, Rerunner, build_cards,
};
use swarmux_domain::evidence::{Bundle, RunOutcome, brief};
use swarmux_domain::iso_minute;
use swarmux_infrastructure::evidence::read_bundle;
use swarmux_infrastructure::{
    EvidenceFiles, FsHoldoutStore, GhOutcomes, GitCheckouts, GitVcs, Paths, ProcessChecker,
    SqliteLedger, SqliteStore, SystemClock,
};

/// How long `labels` waits for GitHub.
const LABELS_WAIT: Duration = Duration::from_secs(90);
/// Hours the brief covers unless told otherwise.
pub const BRIEF_HOURS: i64 = 24;

/// Runs a bundle's checks again in a clean checkout, and the
/// repository's hidden checks after them.
fn rerunner() -> Rerunner {
    let rerunner = Rerunner::new(
        Arc::new(ProcessChecker::new()),
        Arc::new(GitCheckouts::default()),
    );
    match Paths::default_home() {
        Ok(paths) => rerunner.with_holdout(Arc::new(FsHoldoutStore::new(paths.holdout))),
        // No home folder: there are no hidden checks to run either.
        Err(_) => rerunner,
    }
}

/// Wires evidence to git, `gh`, the database and the ledger.
pub fn service(
    store: Rc<SqliteStore>,
    ledger: Rc<LedgerService>,
    clock: Rc<dyn Clock>,
    checkpoints: Option<Rc<CheckpointService>>,
) -> Rc<EvidenceService> {
    let mut service = EvidenceService::new(
        Rc::new(EvidenceFiles::new(store.clone())),
        ledger,
        store,
        Rc::new(GitVcs::default()),
        clock,
    )
    .with_rerunner(rerunner())
    .with_probe(Rc::new(GhOutcomes::default()));
    if let Some(c) = checkpoints {
        service = service.with_checkpoints(c);
    }
    Rc::new(service)
}

struct Opened {
    store: Rc<SqliteStore>,
    service: Rc<EvidenceService>,
    clock: Rc<SystemClock>,
}

fn open() -> anyhow::Result<Opened> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    let service = service(store.clone(), ledger, clock.clone(), None);
    Ok(Opened {
        store,
        service,
        clock,
    })
}

/// The bundle `target` names: a bundle file, or a mission's slug.
fn find(o: &Opened, target: &str) -> anyhow::Result<Bundle> {
    let path = Path::new(target);
    if path.is_file() {
        return Ok(read_bundle(path)?);
    }
    match o.service.get(target)? {
        Some(stored) => Ok(stored.bundle),
        None => anyhow::bail!(
            "no bundle for {target:?}: give the slug of a delivered mission or the path of a bundle file"
        ),
    }
}

/// Runs the bundle's checks again at its head in a clean checkout and
/// prints what held. Returns whether everything matched.
pub fn verify(target: &str) -> anyhow::Result<bool> {
    let o = open()?;
    let bundle = find(&o, target)?;
    println!(
        "mission  {}\nhead     {}\nchecks   {}",
        bundle.contract.slug,
        bundle.head_sha,
        bundle.checks.len()
    );
    println!("re-running at head in a clean checkout …");
    let report = o.service.verify_now(&bundle)?;
    for i in &report.items {
        if i.ok {
            println!("  ok       {}: {}", i.subject, i.found);
        } else {
            println!(
                "  MISMATCH {}: expected {}, found {}",
                i.subject, i.expected, i.found
            );
        }
    }
    println!("{}", report.verdict());
    Ok(report.matches())
}

/// Prints what happened in the last `hours` hours.
pub fn print_brief(hours: i64) -> anyhow::Result<()> {
    let o = open()?;
    let now = o.clock.now();
    let since = now - hours.max(1) * 3_600_000;
    let rows = HistoryService::new(o.store.clone()).list("")?;
    let rows: Vec<_> = rows.into_iter().filter(|r| r.review_of.is_none()).collect();
    let missions = o.store.list()?;
    let stored = o.service.list()?;
    let cards = build_cards(&rows, &missions, &stored);
    let b = brief(&cards, since, now);
    println!("since {} UTC: {}", iso_minute(since), b.headline());
    if b.needs_you.is_empty() {
        println!("nothing needs you");
    } else {
        println!("needs you:");
        for t in &b.needs_you {
            println!("  {t}");
        }
    }
    for c in cards.iter().filter(|c| c.at >= since && c.at <= now) {
        let evidence = c.evidence.as_ref().map_or_else(
            || "no bundle".to_string(),
            |e| format!("{}/{} {}", e.checks_passed, e.checks_total, e.digest),
        );
        let label = c
            .labels
            .last()
            .map_or_else(String::new, |l| format!("  {} #{}", l.kind.as_str(), l.pr));
        println!(
            "  {}  {:<9}  ${:>6.2}  {}  {}{label}",
            iso_minute(c.at),
            c.outcome.as_str(),
            c.cost_usd,
            evidence,
            c.title
        );
    }
    let delivered = cards
        .iter()
        .filter(|c| c.outcome == RunOutcome::Delivered)
        .count();
    println!("{delivered} delivered in all");
    Ok(())
}

/// How many runs ended delivered, all time.
pub fn delivered_count() -> anyhow::Result<usize> {
    let o = open()?;
    let rows = HistoryService::new(o.store.clone()).list("")?;
    let missions = o.store.list()?;
    let stored = o.service.list()?;
    Ok(build_cards(&rows, &missions, &stored)
        .iter()
        .filter(|c| c.outcome == RunOutcome::Delivered)
        .count())
}

/// Asks GitHub what became of each delivered branch and prints the labels.
pub fn labels() -> anyhow::Result<()> {
    let o = open()?;
    let asked = o.service.request_labels()?;
    println!("asked about {asked} branch(es)");
    let end = Instant::now() + LABELS_WAIT;
    let mut changed = Vec::new();
    while o.service.probing_labels() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        changed.extend(o.service.pump_labels());
    }
    changed.extend(o.service.pump_labels());
    for slug in &changed {
        if let Some(s) = o.service.get(slug)?
            && let Some(l) = s.labels.last()
        {
            println!("  {slug}: {} #{}", l.kind.as_str(), l.pr);
        }
    }
    if let Some(why) = o.service.label_problem() {
        println!("trouble: {why}");
    }
    println!("{} label(s) changed", changed.len());
    Ok(())
}
