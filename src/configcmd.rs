//! `swarmux config`: what the configuration is, where each value came from,
//! what is wrong with it, and where the files are.

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Subcommand;
use swarmux_domain::layers::{MergeKind, Tier};
use swarmux_infrastructure::Paths;
use swarmux_infrastructure::config_layers::{
    Entry, Finding, Inputs, Level, Project, Resolved, SCHEMA_VERSION, process_inputs, resolve,
};
use swarmux_infrastructure::config_registry::Registry;
use swarmux_infrastructure::paths::Why;

#[derive(Subcommand)]
pub enum Action {
    /// Every setting with its value and where the value came from. Keys that
    /// were not set show their default.
    List {
        /// Only what some file, variable or flag set.
        #[arg(long)]
        changed: bool,
        /// Only values that came from this tier: core, org, user, workspace,
        /// project, pack, mission, run or managed.
        #[arg(long)]
        tier: Option<String>,
        #[command(flatten)]
        scope: Scope,
    },
    /// The value of one setting, as TOML. Exits 1 for a key that is not a
    /// setting or has no value.
    Get {
        key: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// One setting in full: its value, the tier and file it came from, what
    /// it overrode, whether a lock applies, and the rule that merges it.
    Explain {
        key: String,
        #[command(flatten)]
        scope: Scope,
    },
    /// Every problem in the configuration files, one per line: lines that
    /// could not be read, values that were replaced, keys swarmux does not
    /// know. Exits 1 when there is an error; with --strict, a warning too.
    Lint {
        #[arg(long)]
        strict: bool,
        #[command(flatten)]
        scope: Scope,
    },
    /// The JSON Schemas of the files swarmux reads (config.toml, gates.toml,
    /// mission contracts, the knowledge file, themes and keys). With --out
    /// writes them into a folder; without, prints one.
    Schema {
        /// Write every schema into this folder.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Which one to print without --out: config, managed, gates, mission,
        /// knowledge, keys or theme.
        #[arg(long, default_value = "config")]
        name: String,
    },
    /// The configuration as TOML: the effective one, or what one tier sets.
    Export {
        /// Only what this tier sets (user, workspace, project, managed ...).
        #[arg(long)]
        tier: Option<String>,
        #[command(flatten)]
        scope: Scope,
    },
    /// Where the configuration and the data are, and why.
    Paths {
        #[command(flatten)]
        scope: Scope,
    },
    /// Moves an existing ~/.swarmux to the config and state folders of this
    /// platform. Optional: ~/.swarmux keeps working where it is. Shows the
    /// plan; --yes does it. Needs the background service stopped.
    MigratePaths {
        #[arg(long)]
        yes: bool,
    },
}

/// Which layers beyond the user's to read.
#[derive(clap::Args, Clone)]
pub struct Scope {
    /// Read this repository's .swarmux/config.toml as the project tier
    /// (default: the current folder's repository, when it has one).
    #[arg(long, value_name = "FOLDER")]
    repo: Option<PathBuf>,
    /// Read this workspace's file as the workspace tier (or set
    /// SWARMUX_WORKSPACE).
    #[arg(long, value_name = "NAME")]
    workspace: Option<String>,
}

fn inputs(paths: &Paths, scope: &Scope) -> Inputs {
    let mut inputs = process_inputs(paths, scope.workspace.as_deref());
    let repo = scope
        .repo
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .filter(|d| d.join(".swarmux").join("config.toml").is_file());
    if let Some(root) = repo {
        let trusted = crate::trust::is_trusted(&root);
        inputs.project = Some(Project { root, trusted });
    }
    inputs
}

pub fn run(action: Action) -> anyhow::Result<bool> {
    let paths = Paths::default_home()?;
    let reg = Registry::core();
    match action {
        Action::List {
            changed,
            tier,
            scope,
        } => {
            let r = resolve(&reg, &inputs(&paths, &scope));
            list(&r, changed, tier.as_deref())?;
            Ok(true)
        }
        Action::Get { key, scope } => {
            let r = resolve(&reg, &inputs(&paths, &scope));
            let entry = known(&reg, &r, &key)?;
            match &entry.value {
                Some(v) => {
                    println!("{v}");
                    Ok(true)
                }
                None => {
                    eprintln!("{key} has no value: nothing sets it and it has no default");
                    Ok(false)
                }
            }
        }
        Action::Explain { key, scope } => {
            let r = resolve(&reg, &inputs(&paths, &scope));
            let entry = known(&reg, &r, &key)?;
            explain(&reg, entry);
            Ok(true)
        }
        Action::Lint { strict, scope } => {
            let r = resolve(&reg, &inputs(&paths, &scope));
            Ok(lint(&r, strict))
        }
        Action::Schema { out, name } => {
            schema(out.as_deref(), &name)?;
            Ok(true)
        }
        Action::Export { tier, scope } => {
            let r = resolve(&reg, &inputs(&paths, &scope));
            export(&r, tier.as_deref())?;
            Ok(true)
        }
        Action::Paths { scope } => {
            show_paths(&paths, &scope);
            Ok(true)
        }
        Action::MigratePaths { yes } => migrate::run(&paths, yes),
    }
}

fn known<'a>(reg: &Registry, r: &'a Resolved, key: &str) -> anyhow::Result<&'a Entry> {
    match r.get(key) {
        Some(e) => Ok(e),
        None => {
            let near: Vec<String> = reg
                .keys()
                .map(|k| k.path.clone())
                .filter(|p| p.contains(key) || key.contains(p.as_str()))
                .take(5)
                .collect();
            anyhow::bail!(
                "{key} is not a setting swarmux knows{}",
                if near.is_empty() {
                    " (`swarmux config list` shows them)".to_string()
                } else {
                    format!("; did you mean {}?", near.join(", "))
                }
            )
        }
    }
}

/// Where a value came from, as one short phrase.
fn origin(e: &Entry) -> String {
    if e.is_default {
        return "default".into();
    }
    let line = e.line.map_or(String::new(), |n| format!(":{n}"));
    match e.tier {
        Tier::Run => e.source.clone(),
        _ => format!("{}{line}", e.source),
    }
}

fn list(r: &Resolved, changed: bool, tier: Option<&str>) -> anyhow::Result<()> {
    let want = match tier {
        Some(t) => Some(Tier::parse(t).with_context(|| {
            format!("--tier is core, org, user, workspace, project, pack, mission, run or managed, not {t:?}")
        })?),
        None => None,
    };
    let width = r
        .entries
        .values()
        .filter(|e| e.value.is_some())
        .map(|e| e.path.len())
        .max()
        .unwrap_or(20);
    for e in r.entries.values() {
        if changed && e.is_default {
            continue;
        }
        if want.is_some_and(|t| e.tier != t) {
            continue;
        }
        let Some(value) = &e.value else { continue };
        let mut shown = value.to_string();
        if shown.chars().count() > 60 {
            shown = format!("{}…", shown.chars().take(59).collect::<String>());
        }
        let mark = match (&e.locked_by, e.merge) {
            (Some(_), _) => "  locked",
            (None, MergeKind::Lock) => "  lock",
            _ => "",
        };
        println!(
            "{:<width$} = {:<24} # {:<9} {}{}",
            e.path,
            shown,
            e.tier.as_str(),
            origin(e),
            mark
        );
    }
    Ok(())
}

fn explain(reg: &Registry, e: &Entry) {
    let def = reg.key(&e.path);
    match &e.value {
        Some(v) => println!("{} = {v}", e.path),
        None => println!("{} (not set, no default)", e.path),
    }
    if let Some(d) = &def {
        println!("  what      {}", d.doc);
    }
    println!(
        "  tier      {}{}",
        e.tier.as_str(),
        if e.is_default {
            " (nothing sets it)".to_string()
        } else {
            format!(" ({})", origin(e))
        }
    );
    let merge = match e.merge {
        MergeKind::Scalar => "scalar: the nearest tier wins",
        MergeKind::List => "list: the union of every tier's list",
        MergeKind::Lock => "lock: the strictest value of any tier wins",
        MergeKind::Allowlist => "allowlist: taken whole from the highest tier that sets it",
    };
    println!("  merge     {merge}");
    println!("  effect    {}", e.effect.as_str());
    match &e.locked_by {
        Some(file) => println!("  locked    yes, by {file}: nothing above it can change the value"),
        None => println!("  locked    no"),
    }
    if let Some(d) = &def {
        let tiers: Vec<&str> = d.tiers.list().iter().map(|t| t.as_str()).collect();
        println!("  tiers     may be set in: {}", tiers.join(", "));
        println!(
            "  env       {}",
            if d.env {
                format!(
                    "SWARMUX_{}__{}",
                    d.section().to_ascii_uppercase(),
                    d.name().to_ascii_uppercase()
                )
            } else {
                "cannot be set from the environment".to_string()
            }
        );
    }
    if e.chain.is_empty() {
        return;
    }
    println!("  chain     lowest tier first; the winner is marked");
    for c in &e.chain {
        let win = e.value.as_ref() == Some(&c.value) && c.tier == e.tier && c.source == e.source;
        let line = c.line.map_or(String::new(), |n| format!(":{n}"));
        println!(
            "    {} {:<9} {}  ({}{line})",
            if win { "→" } else { " " },
            c.tier.as_str(),
            c.value,
            c.source
        );
    }
    if e.merge == MergeKind::List && e.chain.len() > 1 {
        println!("    the lists are joined, in this order, without repeats");
    }
    for ig in &e.ignored {
        println!(
            "    ✗ {:<9} {}  ({}): not used, {}",
            ig.contribution.tier.as_str(),
            ig.contribution.value,
            ig.contribution.source,
            ig.why
        );
    }
}

fn say(f: &Finding) -> String {
    let mark = match f.level {
        Level::Error => "✗",
        Level::Warn => "!",
        Level::Info => "·",
    };
    format!("{mark} {f}")
}

fn lint(r: &Resolved, strict: bool) -> bool {
    for l in &r.layers {
        println!(
            "{} {} {}",
            if l.found { "read " } else { "no file" },
            l.tier.as_str(),
            l.source
        );
    }
    if r.findings.is_empty() {
        println!("✓ the configuration reads without a problem");
    }
    for f in &r.findings {
        println!("{}", say(f));
    }
    let failing = r.failing(strict);
    if !r.findings.is_empty() {
        println!(
            "{} finding(s); the app starts anyway, with the default (or the restrictive reading) for each line that is wrong",
            r.findings.len()
        );
    }
    failing.is_empty()
}

fn export(r: &Resolved, tier: Option<&str>) -> anyhow::Result<()> {
    let want = match tier {
        Some(t) => Some(Tier::parse(t).with_context(|| format!("--tier: unknown tier {t:?}"))?),
        None => None,
    };
    println!("schema_version = {SCHEMA_VERSION}");
    let body = r.export(want).map_err(anyhow::Error::msg)?;
    if !body.trim().is_empty() {
        println!();
        print!("{body}");
    }
    Ok(())
}

fn schema(out: Option<&Path>, name: &str) -> anyhow::Result<()> {
    let all = swarmux_infrastructure::config_schema::all(&Registry::core());
    match out {
        Some(dir) => {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
            for (file, doc) in &all {
                let path = dir.join(file);
                std::fs::write(&path, doc)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                println!("{}", path.display());
            }
        }
        None => {
            let file = format!("{name}.schema.json");
            let doc = all
                .iter()
                .find(|(f, _)| *f == file)
                .map(|(_, d)| d)
                .with_context(|| {
                    format!("no schema named {name:?}: config, managed, gates, mission, knowledge, keys or theme")
                })?;
            print!("{doc}");
        }
    }
    Ok(())
}

fn exists(p: &Path) -> &'static str {
    if p.exists() { "found" } else { "not found" }
}

fn why(w: &Why) -> String {
    w.text()
}

fn show_paths(paths: &Paths, scope: &Scope) {
    println!("home          {}", paths.home.display());
    println!(
        "config dir    {}\n              {}",
        paths.config_dir.display(),
        why(&paths.config_why)
    );
    println!(
        "config file   {}  ({})",
        paths.config.display(),
        exists(&paths.config)
    );
    println!(
        "state dir     {}\n              {}",
        paths.data_dir.display(),
        why(&paths.state_why)
    );
    println!("              holds the database, the ledger, the audit key, hidden checks,");
    println!("              the verifier's copies, spools and caches");
    println!("database      {}", paths.db.display());
    println!(
        "managed file  {}  ({})\n              {}",
        paths.managed.display(),
        exists(&paths.managed),
        why(&paths.managed_why)
    );
    let workspaces = paths.config_dir.join("workspaces");
    println!(
        "workspaces    {}/<name>.toml  (read with --workspace <name> or SWARMUX_WORKSPACE)",
        workspaces.display()
    );
    let repo = scope.repo.clone().or_else(|| std::env::current_dir().ok());
    if let Some(repo) = repo {
        let p = repo.join(".swarmux").join("config.toml");
        println!("project       {}  ({})", p.display(), exists(&p));
    }
    // Two places that both look like a config folder: say which one counts.
    let legacy = paths.home.join(".swarmux");
    if paths.config_dir == legacy {
        let other = paths
            .home
            .join(".config")
            .join("swarmux")
            .join("config.toml");
        if other.is_file() {
            println!(
                "note          {} exists too and is not read: ~/.swarmux exists, so it wins. Run `swarmux config migrate-paths`, or set SWARMUX_CONFIG_DIR.",
                other.display()
            );
        }
    }
    println!();
    println!("order         SWARMUX_CONFIG_DIR / SWARMUX_STATE_DIR, then ~/.swarmux if it exists,");
    println!(
        "              then the platform's folders (XDG on Linux, Application Support on macOS,"
    );
    println!("              APPDATA and LOCALAPPDATA on Windows)");
}

/// What `swarmux doctor` adds about the configuration.
pub fn doctor(paths: &Paths, r: &Resolved) {
    println!(
        "ok   config folder {} ({}), state folder {} ({})",
        paths.config_dir.display(),
        paths.config_why.text(),
        paths.data_dir.display(),
        paths.state_why.text()
    );
    for l in &r.layers {
        if l.tier == Tier::Managed && l.found {
            println!(
                "ok   managed file {} locks {} key(s) (a client-side control, not a security boundary)",
                l.source,
                r.locked.len()
            );
        }
    }
    let errors = r
        .findings
        .iter()
        .filter(|f| f.level == Level::Error)
        .count();
    let warns = r.findings.iter().filter(|f| f.level == Level::Warn).count();
    if errors + warns > 0 {
        println!(
            "warn config: {errors} error(s), {warns} warning(s): `swarmux config lint` lists them; the app starts anyway"
        );
    }
}

/// `swarmux config migrate-paths`.
mod migrate {
    use super::*;
    use std::rc::Rc;
    use swarmux_application::ledger::LedgerService;
    use swarmux_infrastructure::{SqliteLedger, SystemClock};

    pub fn run(paths: &Paths, yes: bool) -> anyhow::Result<bool> {
        let legacy = paths.home.join(".swarmux");
        if !legacy.is_dir() || paths.config_dir != legacy {
            println!(
                "nothing to move: ~/.swarmux is not the folder in use ({}).",
                paths.config_dir.display()
            );
            return Ok(true);
        }
        // `resolve` picks ~/.swarmux while it exists; what the platform
        // convention says is what a fresh home would get.
        let (config_to, state_to) = target_folders(&paths.home);
        println!("from       {}", legacy.display());
        println!("config to  {}", config_to.display());
        println!("state to   {}", state_to.display());
        let worktrees = legacy.join("worktrees");
        if worktrees.is_dir()
            && std::fs::read_dir(&worktrees)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
            println!(
                "\nnot moved: {} holds worktrees, which git knows by their path. Finish or clean them (`swarmux worktree list`), then run this again.",
                worktrees.display()
            );
            return Ok(false);
        }
        if swarmux_infrastructure::daemon::is_held(&legacy) {
            println!(
                "\nnot moved: the background service is running. `swarmux daemon stop`, then run this again."
            );
            return Ok(false);
        }
        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(&legacy)? {
            let entry = entry?;
            let name = entry.file_name();
            let to_config = name == "config.toml" || name == "workspaces-config";
            let target = if to_config { &config_to } else { &state_to };
            moves.push((entry.path(), target.join(&name)));
        }
        println!();
        for (from, to) in &moves {
            println!("  {}  →  {}", from.display(), to.display());
        }
        if !yes {
            println!("\nthis was the plan; nothing moved. Run again with --yes to do it.");
            return Ok(true);
        }
        std::fs::create_dir_all(&config_to)?;
        std::fs::create_dir_all(&state_to)?;
        let mut done = Vec::new();
        for (from, to) in &moves {
            if to.exists() {
                anyhow::bail!("{} already exists; nothing more was moved", to.display());
            }
            move_path(from, to)?;
            done.push(to.display().to_string());
        }
        // The old folder is empty now; leaving it would make it win again.
        let _ = std::fs::remove_dir(&legacy);
        // The record goes into the ledger where it is now.
        let db = state_to.join("swarmux.db");
        if db.is_file() {
            let ledger =
                LedgerService::new(Rc::new(SqliteLedger::open(&db)?), Rc::new(SystemClock));
            ledger.record_paths_migrated(
                &legacy.display().to_string(),
                &config_to.display().to_string(),
                &state_to.display().to_string(),
                &done,
                ledger.now(),
            )?;
        }
        println!(
            "\nmoved {} item(s). Config is now in {}, state in {}.",
            done.len(),
            config_to.display(),
            state_to.display()
        );
        Ok(true)
    }

    fn target_folders(home: &Path) -> (PathBuf, PathBuf) {
        // What a home without ~/.swarmux resolves to.
        let (config, state, _) = swarmux_infrastructure::paths::locate(
            &|name| std::env::var_os(name),
            home,
            swarmux_infrastructure::paths::Platform::current(),
            &|p| p != home.join(".swarmux") && p.is_dir(),
        );
        (config.dir, state.dir)
    }

    fn move_path(from: &Path, to: &Path) -> anyhow::Result<()> {
        if std::fs::rename(from, to).is_ok() {
            return Ok(());
        }
        // Another file system: copy, then remove.
        if from.is_dir() {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                move_path(&entry.path(), &to.join(entry.file_name()))?;
            }
            std::fs::remove_dir(from)?;
        } else {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)?;
        }
        Ok(())
    }
}

/// Prints the lines of any configuration file that could not be read as
/// TOML, for the older `lint` commands (`keys lint`, `guard lint`, `deck
/// lint`), which must not call a file they could not read clean. Whether
/// there were any.
pub fn report_syntax(paths: &Paths) -> bool {
    let r = resolve(&Registry::core(), &process_inputs(paths, None));
    let mut any = false;
    for f in r
        .findings
        .iter()
        .filter(|f| f.kind == swarmux_infrastructure::config_layers::Kind::Syntax)
    {
        println!("{}", say(f));
        any = true;
    }
    any
}
