//! `swarmux guard`: the rules added with "always allow", and a check of the
//! Guard's settings; and the wiring of what the Guard XXrotects.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Context;
use clap::Subcommand;
use swarmux_application::ledger::{LedgerService, LedgeredRules};
use swarmux_application::ports::{Clock, RuleStore};
use swarmux_application::{Protection, Settings};
use swarmux_domain::{DAY_MS, StoredRule, iso_minute};
use swarmux_infrastructure::hook::spool_file;
use swarmux_infrastructure::{
    Paths, SqliteLedger, SystemClock, TomlRuleStore, load_settings, rules_file,
};

#[derive(Subcommand)]
pub enum Action {
    /// The allow rules: the ones in config.toml and the ones added with
    /// "always allow", with where they apply and when they stop.
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
    /// Check the `[guard]` settings and the rules file: every line that was
    /// replaced by its default, and any section left from JEV (removed), is
    /// named. Exits 1 when there is one.
    Lint,
}

#[derive(Subcommand)]
pub enum RulesAction {
    /// List every allow rule.
    List,
    /// Remove an added rule by its id (see `list`). Rules in config.toml
    /// are edited there.
    Remove {
        /// The rule's id.
        id: String,
    },
    /// Remove the added rules that have expired.
    Prune,
}

pub fn run(action: Action) -> anyhow::Result<bool> {
    let paths = Paths::default_home()?;
    match action {
        Action::Rules { action } => rules(&paths, action),
        Action::Lint => lint(&paths),
    }
}

fn store(paths: &Paths) -> anyhow::Result<LedgeredRules> {
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        Rc::new(SystemClock),
    ));
    Ok(LedgeredRules::new(
        Rc::new(TomlRuleStore::new(rules_file(&paths.data_dir))),
        ledger,
    ))
}

fn rules(paths: &Paths, action: RulesAction) -> anyhow::Result<bool> {
    let store = store(paths)?;
    let now = SystemClock.now();
    match action {
        RulesAction::List => {
            let settings = load_settings(&paths.config)?;
            let kept = store.list()?;
            if kept.is_empty() && settings.guard.allow.is_empty() {
                println!("no allow rules besides the built-in ones");
            }
            for rule in &settings.guard.allow {
                println!(
                    "config    {rule}   global, never expires (source config; edit config.toml)"
                );
            }
            for r in &kept {
                println!("{}", describe(r, now));
            }
            Ok(true)
        }
        RulesAction::Remove { id } => {
            if store.remove(id.trim())? {
                println!("removed {id}; it is in the ledger as a policy entry");
                Ok(true)
            } else {
                println!("no added rule has the id {id}: `swarmux guard rules list` shows them");
                Ok(false)
            }
        }
        RulesAction::Prune => {
            let gone = store.prune()?;
            if gone.is_empty() {
                println!("no added rule has expired");
            }
            for r in &gone {
                println!("removed {} {} (expired)", r.id, r.text);
            }
            Ok(true)
        }
    }
}

fn describe(r: &StoredRule, now: i64) -> String {
    let at = match &r.root {
        Some(root) => format!("{} {root}", r.scope.as_str()),
        None => r.scope.as_str().to_string(),
    };
    let ends = match r.expires_at {
        None => "never expires".to_string(),
        Some(at) if r.expired(now) => format!("EXPIRED {}", iso_minute(at)),
        Some(at) => format!(
            "expires {} (in {} days)",
            iso_minute(at),
            ((at - now) / DAY_MS).max(0)
        ),
    };
    format!(
        "{:<9} {}   {at}; {ends}; source {}, added {}",
        r.id,
        r.text,
        r.source.as_str(),
        iso_minute(r.created_at)
    )
}

fn lint(paths: &Paths) -> anyhow::Result<bool> {
    let settings = load_settings(&paths.config)?;
    let store = TomlRuleStore::new(rules_file(&paths.data_dir));
    let mut problems = settings.warnings.clone();
    problems.extend(store.problems());
    for p in &problems {
        println!("warn {p}");
    }
    let g = &settings.guard;
    println!(
        "guard.on_unreachable = {} (unattended jobs: {}, other jobs: {})",
        g.on_unreachable.as_str(),
        g.on_unreachable.mode(true).as_str(),
        g.on_unreachable.mode(false).as_str()
    );
    println!(
        "guard.always_scope = {}, always_expiry_days = {}",
        g.always_scope.as_str(),
        g.always_expiry_days
            .map_or("never".to_string(), |d| d.to_string())
    );
    println!(
        "guard.protected_paths = {} built-in + {} of yours",
        g.protected_globs().len() - g.protected_paths.len(),
        g.protected_paths.len()
    );
    println!(
        "guard.unmatched = {} (a call no rule matches; no model is asked)",
        format!("{:?}", g.unmatched).to_lowercase()
    );
    println!("irreversible calls (R4) are allowed only by a typed confirmation");
    if problems.is_empty() {
        println!("ok   no setting was replaced by a default");
    }
    Ok(problems.is_empty())
}

/// What the Guard fences, as absolute paths: the database, the audit key,
/// the configuration, the rules and spool files, the hidden checks, the
/// verifier's copies and the status-line spool. Agents' tools never need them.
pub fn fenced_paths(paths: &Paths) -> Vec<String> {
    let d = &paths.data_dir;
    let mut out: Vec<PathBuf> = Vec::new();
    for db in ["swarmux.db", "usage.db"] {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            out.push(d.join(format!("{db}{suffix}")));
        }
    }
    out.extend([
        d.join("audit.key"),
        paths.config.clone(),
        rules_file(d),
        spool_file(d),
        paths.holdout.clone(),
        paths.verify.clone(),
        d.join("statusline"),
    ]);
    // A configuration folder of its own (a fresh install, or
    // SWARMUX_CONFIG_DIR) is fenced whole: the files a person edits, the
    // organisation's file and the layers are not for an agent to change. The
    // old shared `~/.swarmux` is not: agents work in worktrees below it.
    if paths.config_dir != paths.data_dir {
        out.push(paths.config_dir.clone());
    }
    out.push(paths.managed.clone());
    out.into_iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect()
}

/// What the Guard refuses to its agents, as the settings and the paths say.
pub fn protection(paths: &Paths, settings: &Settings) -> anyhow::Result<Protection> {
    let mut p = Protection::new(
        fenced_paths(paths),
        &paths.home.to_string_lossy().replace('\\', "/"),
    );
    p.exe = std::env::current_exe()
        .ok()
        .map(|e| e.to_string_lossy().replace('\\', "/"));
    p.protected_globs = settings.guard.protected_globs();
    p.resolve = Arc::new(|path: &str| {
        let real = std::fs::canonicalize(path).ok()?;
        let text = real.to_string_lossy().replace('\\', "/");
        Some(text.strip_prefix("//?/").unwrap_or(&text).to_string())
    });
    Ok(p)
}
