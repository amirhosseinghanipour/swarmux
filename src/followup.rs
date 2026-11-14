//! Follow-through wiring and the `swarmux followup` commands.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Context;
use clap::Subcommand;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::{Clock, JobRepository, SessionRepository};
use swarmux_application::{FollowUpService, Reply, Settings};
use swarmux_domain::JobId;
use swarmux_domain::followup::{FollowStatus, FollowUp, OwnerBrief};
use swarmux_infrastructure::{
    HaikuExtractor, Paths, SqliteLedger, SqliteStore, SystemClock, load_effective_settings,
    utc_offset_minutes,
};

#[derive(Subcommand)]
pub enum FollowAction {
    /// What is waiting for you (or, with --all, everything kept).
    List {
        #[arg(long)]
        all: bool,
    },
    /// Approve a follow-up: it becomes an intent, ready to turn into a mission.
    Approve { id: i64 },
    /// Decline a follow-up; it will not be raised again.
    Decline { id: i64 },
    /// Put a follow-up off; it comes back after `defer_hours`.
    Defer { id: i64 },
    /// The owner brief now: decision cards, with a reason you might say no.
    Brief,
    /// The standing orders from config.toml.
    Orders,
    /// Approved follow-ups that are not yet missions.
    Intents,
}

/// Builds the service the app and the commands use. `extractor` is given
/// only where a model may be asked (the running app).
pub fn build(
    settings: &Settings,
    store: Rc<SqliteStore>,
    ledger: Rc<LedgerService>,
    clock: Rc<dyn Clock>,
    extractor: Option<HaikuExtractor>,
) -> FollowUpService {
    let repos = store.clone();
    let repo_of: Rc<dyn Fn(JobId) -> String> = Rc::new(move |job| {
        JobRepository::get(&*repos, job)
            .ok()
            .flatten()
            .and_then(|j| SessionRepository::get(&*repos, j.session_id).ok().flatten())
            .and_then(|s| s.cwd.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    });
    let mut service = FollowUpService::new(store.clone(), store, clock, settings.followups.clone())
        .with_ledger(ledger)
        .with_utc_offset(utc_offset_minutes)
        .with_repo_of(repo_of);
    if let Some(x) = extractor {
        service = service.with_extractor(Rc::new(x));
    }
    service
}

fn open() -> anyhow::Result<FollowUpService> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let settings = load_effective_settings(&paths);
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    Ok(build(&settings, store, ledger, clock, None))
}

fn line(f: &FollowUp) -> String {
    format!(
        "#{:<4} {:<13} {:<8} {}{}",
        f.id,
        f.status.as_str(),
        f.kind.as_str(),
        f.text,
        if f.repo.is_empty() {
            String::new()
        } else {
            format!("  ({})", f.repo)
        }
    )
}

pub fn run(action: FollowAction) -> anyhow::Result<()> {
    let svc = open()?;
    match action {
        FollowAction::List { all } => {
            let rows: Vec<FollowUp> = if all { svc.all()? } else { svc.waiting()? };
            if rows.is_empty() {
                println!(
                    "{}",
                    if all {
                        "no follow-ups"
                    } else {
                        "nothing is waiting for you"
                    }
                );
            }
            for f in rows {
                println!("{}", line(&f));
            }
        }
        FollowAction::Approve { id } => answer(&svc, id, Reply::Approve)?,
        FollowAction::Decline { id } => answer(&svc, id, Reply::Decline)?,
        FollowAction::Defer { id } => answer(&svc, id, Reply::Defer)?,
        FollowAction::Brief => print_brief(&svc, None)?,
        FollowAction::Orders => {
            if svc.orders().is_empty() {
                println!("no standing orders: add [[followups.order]] to config.toml");
            }
            for o in svc.orders() {
                println!(
                    "{}: {} kinds={:?} repos={:?} paths={:?} words={:?} max_cost={:?}{}",
                    o.id,
                    o.action.as_str(),
                    o.kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
                    o.repos,
                    o.paths,
                    o.keywords,
                    o.max_cost_usd,
                    if o.allow_risky { " allow_risky" } else { "" }
                );
            }
        }
        FollowAction::Intents => {
            let rows: Vec<_> = svc
                .intents()?
                .into_iter()
                .filter(|i| i.mission.is_none())
                .collect();
            if rows.is_empty() {
                println!("no approved follow-ups are waiting to become missions");
            }
            for i in rows {
                println!("#{} (follow-up #{}) {}", i.id, i.followup, i.text);
            }
        }
    }
    Ok(())
}

fn answer(svc: &FollowUpService, id: i64, reply: Reply) -> anyhow::Result<()> {
    let intent = svc.reply(id, reply)?;
    let f = svc.get(id)?.context("the follow-up vanished")?;
    println!("{}", line(&f));
    if intent.is_some() {
        println!("kept as an intent: open swarmux and press 9 then m to draft a mission");
    }
    if f.status == FollowStatus::Deferred {
        println!("it comes back in {} hours", svc.settings().defer_hours);
    }
    Ok(())
}

/// Prints the brief's cards. `counts` are the finished and met numbers the
/// caller already has; `None` leaves them at zero.
pub fn print_brief(svc: &FollowUpService, counts: Option<(usize, usize)>) -> anyhow::Result<()> {
    let mut brief: OwnerBrief = svc.brief_now(&[], &HashMap::new())?;
    if let Some((finished, met)) = counts {
        brief.finished = finished;
        brief.met_checks = met;
    }
    if counts.is_some() {
        // The caller printed the headline; the cards and the note follow.
        let text = brief.text();
        let rest = text.split_once('\n').map_or("", |(_, r)| r);
        if brief.is_empty() {
            println!("no follow-ups need you");
        } else {
            println!("follow-ups:{rest}");
        }
    } else {
        println!("{}", brief.text());
    }
    Ok(())
}

/// The follow-ups section of `swarmux brief`.
pub fn brief_section(finished: usize, met: usize) -> anyhow::Result<()> {
    let svc = open()?;
    if !svc.settings().enabled {
        return Ok(());
    }
    print_brief(&svc, Some((finished, met)))
}
