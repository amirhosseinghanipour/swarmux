//! Worktrees and checkpoints: the services the app runs with, and
//! `swarmux worktree`.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use clap::ValueEnum;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::{Clock, SessionRepository};
use swarmux_application::{
    CheckpointService, HandoffBoard, SessionService, Settings, WorktreeService, WorktreeView,
};
use swarmux_domain::{Checkpoint, JobId, WorktreeRecord, iso_minute};
use swarmux_infrastructure::{GitWorktrees, Paths, SqliteLedger, SqliteStore, SystemClock};

/// The two services every part of the app shares.
pub struct Services {
    pub worktrees: Rc<WorktreeService>,
    pub checkpoints: Rc<CheckpointService>,
}

/// Wires worktrees and checkpoints to git, the database and the ledger.
pub fn services(
    paths: &Paths,
    settings: &Settings,
    store: Rc<SqliteStore>,
    ledger: Rc<LedgerService>,
    clock: Rc<dyn Clock>,
    board: Arc<HandoffBoard>,
) -> Services {
    let worktrees = Rc::new(
        WorktreeService::new(
            Rc::new(GitWorktrees::default()),
            store.clone(),
            store.clone(),
            clock.clone(),
            paths.data_dir.join("worktrees"),
        )
        .with_settings(&settings.worktree)
        .with_ledger(ledger.clone())
        .with_handoffs(board.clone()),
    );
    let checkpoints = Rc::new(
        CheckpointService::new(worktrees.clone(), store, ledger, clock).with_handoffs(board),
    );
    Services {
        worktrees,
        checkpoints,
    }
}

/// What a session's worktree mode can be set to.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Mode {
    /// Always give each job a worktree.
    On,
    /// Never.
    Off,
    /// On in a git repository, off elsewhere (the default).
    Auto,
}

struct Open {
    store: Rc<SqliteStore>,
    services: Services,
    settings: Settings,
}

fn open() -> anyhow::Result<Open> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let settings = swarmux_infrastructure::load_settings(&paths.config)?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock: Rc<dyn Clock> = Rc::new(SystemClock);
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    let services = services(
        &paths,
        &settings,
        store.clone(),
        ledger,
        clock,
        HandoffBoard::new(),
    );
    Ok(Open {
        store,
        services,
        settings,
    })
}

/// Prints the worktrees that have not been reclaimed.
pub fn list() -> anyhow::Result<()> {
    let open = open()?;
    let rows = open.services.worktrees.list()?;
    if rows.is_empty() {
        println!("no worktrees");
        return Ok(());
    }
    for row in &rows {
        println!("{}", line(row));
    }
    println!(
        "{} worktrees; stale ones are reclaimed after {} h (swarmux worktree gc)",
        rows.len(),
        open.settings.worktree.stale_hours
    );
    Ok(())
}

fn line(v: &WorktreeView) -> String {
    let r: &WorktreeRecord = &v.record;
    let state = if v.missing {
        "missing"
    } else if v.stale {
        "stale"
    } else {
        "in use"
    };
    format!(
        "#{:<5} {:<8} {:<7} ports {}+{:<3} {:>2} checkpoints  {}  {}",
        r.job.0,
        v.job_status.as_deref().unwrap_or("?"),
        state,
        r.port_base,
        r.port_count,
        v.checkpoints,
        r.branch,
        r.path.display()
    )
}

/// Reclaims stale worktrees (or, with `dry_run`, says which).
pub fn gc(dry_run: bool) -> anyhow::Result<()> {
    let open = open()?;
    let report = open.services.worktrees.gc(dry_run)?;
    let verb = if dry_run {
        "would reclaim"
    } else {
        "reclaimed"
    };
    for r in &report.reclaimed {
        println!("{verb} #{} {} ({})", r.job.0, r.path.display(), r.branch);
    }
    for r in &report.forgotten {
        println!(
            "forgot  #{} {} (folder already gone)",
            r.job.0,
            r.path.display()
        );
    }
    for e in &report.errors {
        eprintln!("error: {e}");
    }
    println!(
        "{} {}, {} forgotten, {} kept",
        report.reclaimed.len(),
        if dry_run { "to reclaim" } else { "reclaimed" },
        report.forgotten.len(),
        report.kept
    );
    anyhow::ensure!(
        report.errors.is_empty(),
        "some worktrees could not be reclaimed"
    );
    Ok(())
}

/// Prints the checkpoints of one job, or the newest ones.
pub fn checkpoints(job: Option<i64>, limit: usize) -> anyhow::Result<()> {
    let open = open()?;
    let list = match job {
        Some(id) => open.services.checkpoints.list(JobId(id))?,
        None => open.services.checkpoints.recent(limit)?,
    };
    if list.is_empty() {
        println!("no checkpoints");
    }
    for c in &list {
        println!("{}", checkpoint_line(c));
    }
    Ok(())
}

fn checkpoint_line(c: &Checkpoint) -> String {
    format!(
        "#{:<5} {}  {:<11} commit {}  session {}  ledger {}",
        c.job.0,
        iso_minute(c.at),
        c.reason.as_str(),
        &c.commit[..c.commit.len().min(10)],
        c.session_id,
        c.ledger_seq.map_or("-".to_string(), |s| format!("#{s}"))
    )
}

/// Sets worktree mode for a session, given by name or id.
pub fn mode(session: &str, mode: Mode) -> anyhow::Result<()> {
    let open = open()?;
    let all = SessionRepository::list_active(&*open.store)?;
    let found = all
        .iter()
        .find(|s| s.id.to_string() == session)
        .or_else(|| all.iter().find(|s| s.name == session))
        .with_context(|| format!("no session called {session}"))?;
    let value = match mode {
        Mode::On => Some(true),
        Mode::Off => Some(false),
        Mode::Auto => None,
    };
    SessionService::new(open.store.clone(), Rc::new(SystemClock)).set_worktree(found.id, value)?;
    println!(
        "{}: worktrees {}",
        found.name,
        match value {
            Some(true) => "on",
            Some(false) => "off",
            None => "auto (on inside a git repository)",
        }
    );
    Ok(())
}
