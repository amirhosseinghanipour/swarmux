//! `swarmux trust`: which repositories may run the commands they ship.
//!
//! A repository with `.swarmux/gates.toml` (or checks swarmux guesses from
//! its files) runs none of them until the owner grants trust here. A grant
//! covers exactly the files shown; a changed file asks again. The grant is a
//! ledger entry.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::Clock;
use swarmux_application::trust::{TrustGuard, TrustReader, TrustService};
use swarmux_application::{GateSource, LoadedGate};
use swarmux_domain::trust::{Supplied, TrustEvent, TrustState};
use swarmux_infrastructure::trust::LedgerTrustReader;
use swarmux_infrastructure::{FsGateSource, Paths, SqliteLedger, SystemClock};

/// The gate of the repository `dir` lies in, as it is loaded, and what the
/// repository supplies.
fn supplied_of(dir: &Path) -> anyhow::Result<(LoadedGate, Supplied)> {
    anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
    let source = FsGateSource::new();
    let gate = source.load(dir);
    let supplied = source.supplied(&gate);
    Ok((gate, supplied))
}

fn service(paths: &Paths) -> anyhow::Result<TrustService> {
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        Rc::new(SystemClock),
    ));
    Ok(TrustService::new(
        ledger,
        Arc::new(LedgerTrustReader::new(&paths.db)),
    ))
}

/// The gate of `dir` with the trust rule applied, for the commands that
/// run checks by hand (`swarmux gate run`, `gate selftest`): the same rule
/// as for an agent's stop, so a cloned repository's checks do not run
/// because its owner typed a command in it.
pub fn trusted_gate(dir: &Path) -> anyhow::Result<LoadedGate> {
    let paths = Paths::default_home()?;
    let source = FsGateSource::new();
    let mut gate = source.load(dir);
    let guard = TrustGuard::new(
        Arc::new(LedgerTrustReader::new(&paths.db)),
        Arc::new(|| SystemClock.now()),
    );
    let state = guard.apply(&source, &mut gate, false);
    if !state.allows() {
        eprintln!(
            "swarmux: {} is {}; its checks did not run. `swarmux trust show {}` says what they are, `swarmux trust grant {}` allows them.",
            gate.root.display(),
            state.as_str(),
            gate.root.display(),
            gate.root.display()
        );
    }
    Ok(gate)
}

/// One line of `trust list` and `trust show`.
fn say_state(state: &TrustState) -> String {
    match state {
        TrustState::Changed(files) => format!("{} ({})", state.as_str(), files.join(", ")),
        _ => state.as_str().to_string(),
    }
}

/// `swarmux trust show [repo]`: what the repository wants to run and where it stands.
pub fn show(repo: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = match repo {
        Some(d) => d,
        None => std::env::current_dir().context("cannot read the current folder")?,
    };
    let paths = Paths::default_home()?;
    let (gate, supplied) = supplied_of(&dir)?;
    let trust = service(&paths)?;
    println!("repository  {}", gate.root.display());
    if supplied.is_empty() {
        println!(
            "trust       nothing here needs it (no commands shipped, no keys that loosen the gate)"
        );
        return Ok(());
    }
    println!("id          {}", supplied.repo_id);
    println!("trust       {}", say_state(&trust.state_of(&supplied)));
    println!("wants       ");
    for line in supplied.describe().lines() {
        println!("{line}");
    }
    println!("files       a grant covers exactly these (a changed byte asks again):");
    for e in &supplied.entries {
        println!("  {}  {}", &e.sha256[..12.min(e.sha256.len())], e.path);
    }
    Ok(())
}

/// `swarmux trust list`: every repository with a grant or a revocation.
pub fn list() -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let trust = service(&paths)?;
    let records = trust.records();
    if records.is_empty() {
        println!("no repository has been trusted.");
        println!(
            "A repository that ships .swarmux/gates.toml runs none of its checks until you run `swarmux trust grant <folder>`."
        );
        return Ok(());
    }
    for event in records {
        match event {
            TrustEvent::Granted(r) => {
                // Compare with the files as they are now, when the folder is still there.
                let now = Path::new(&r.root)
                    .is_dir()
                    .then(|| supplied_of(Path::new(&r.root)).ok())
                    .flatten()
                    .map(|(_, s)| trust.state_of(&s));
                let state = now.map_or_else(
                    || "granted (folder not found)".to_string(),
                    |s| say_state(&s),
                );
                println!("{:<40} {}  {}", r.repo_id, r.root, state);
            }
            TrustEvent::Revoked { repo_id, .. } => {
                println!("{repo_id:<40} revoked");
            }
        }
    }
    Ok(())
}

/// `swarmux trust grant <repo>`.
pub fn grant(repo: PathBuf) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let (gate, supplied) = supplied_of(&repo)?;
    if supplied.is_empty() {
        println!(
            "{}: nothing here needs trust (no commands shipped, no keys that loosen the gate).",
            gate.root.display()
        );
        return Ok(());
    }
    let trust = service(&paths)?;
    if trust.state_of(&supplied) == TrustState::Trusted {
        println!(
            "{} is already trusted for these files.",
            gate.root.display()
        );
        return Ok(());
    }
    let record = trust.grant(&supplied)?;
    println!("trusted {}", record.root);
    for line in supplied.describe().lines() {
        println!("{line}");
    }
    println!(
        "covers {} file(s); a change to any of them asks again. `swarmux trust revoke {}` takes it back.",
        record.entries.len(),
        record.root
    );
    Ok(())
}

/// `swarmux trust revoke <repo>`: a folder, or an id from `trust list`.
pub fn revoke(repo: String) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let trust = service(&paths)?;
    let (id, root) = match Path::new(&repo).is_dir() {
        true => {
            let (gate, supplied) = supplied_of(Path::new(&repo))?;
            let id = if supplied.repo_id.is_empty() {
                swarmux_infrastructure::trust::repo_id_of(&gate.root)
            } else {
                supplied.repo_id
            };
            (id, gate.root.display().to_string())
        }
        false => (repo.clone(), String::new()),
    };
    let known = trust
        .records()
        .into_iter()
        .any(|e| e.repo_id() == id && matches!(e, TrustEvent::Granted(_)));
    anyhow::ensure!(
        known,
        "{repo} has no grant to revoke (`swarmux trust list` shows them)"
    );
    trust.revoke(&id, &root)?;
    println!("revoked {id}: it runs none of its checks until you grant it again.");
    Ok(())
}

/// What `swarmux doctor` says about repositories waiting for trust: the
/// grants it knows of, and the current folder's state.
pub fn doctor(paths: &Paths) {
    let reader = LedgerTrustReader::new(&paths.db);
    let events = reader.events();
    let granted = events
        .iter()
        .filter(|e| matches!(e, TrustEvent::Granted(_)))
        .map(TrustEvent::repo_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    println!(
        "ok   repository trust: {granted} repositor{} granted (`swarmux trust list`)",
        if granted == 1 { "y" } else { "ies" }
    );
    if let Ok(dir) = std::env::current_dir()
        && let Ok((gate, supplied)) = supplied_of(&dir)
        && !supplied.is_empty()
    {
        let state = trust_state_of(&reader, &supplied);
        if state.allows() {
            println!("ok   {} is trusted", gate.root.display());
        } else {
            println!(
                "warn {} is {}: its checks do not run. It wants:",
                gate.root.display(),
                say_state(&state)
            );
            for line in supplied.describe().lines() {
                println!("       {line}");
            }
            println!(
                "       `swarmux trust grant {}` allows them",
                gate.root.display()
            );
        }
    }
}

fn trust_state_of(reader: &dyn TrustReader, supplied: &Supplied) -> TrustState {
    let events = reader.events();
    swarmux_domain::trust::trust_state(
        supplied,
        swarmux_application::trust::latest(&events, &supplied.repo_id).as_ref(),
    )
}
