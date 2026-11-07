//! `swarmux knowledge` and `swarmux rules`: the rules, facts and decisions
//! kept for a workspace's (or a repository's) agents.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use swarmux_application::ledger::LedgerService;
use swarmux_application::{KnowledgeCodec, KnowledgeService, WorkspaceService};
use swarmux_domain::iso_minute;
use swarmux_domain::knowledge::{
    Action as RuleAction, Anchor, CheckRule, Draft, Enforcer, EnforcerKind, GuardRule, Home, Item,
    ItemId, Kind, Scope, Status, Trust, map,
};
use swarmux_infrastructure::{
    FsAnchorReader, Paths, SqliteLedger, SqliteStore, SystemClock, TomlKnowledge,
};

/// What unknown values are shown as.
const UNKNOWN: &str = "—";

/// Which knowledge: a workspace's, by name, or a repository's, by folder.
/// Neither: the workspace whose root is the current folder, else the
/// current folder as a repository.
#[derive(Args, Clone, Debug, Default)]
pub struct HomeArgs {
    #[arg(long, conflicts_with = "repo")]
    pub workspace: Option<String>,
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

/// An item's fields; on `edit`, only those given change.
#[derive(Args, Clone, Debug, Default)]
pub struct ItemArgs {
    /// rule, fact or decision.
    #[arg(long)]
    pub kind: Option<String>,
    /// One to three sentences.
    #[arg(long)]
    pub text: Option<String>,
    /// Why, in one sentence.
    #[arg(long)]
    pub why: Option<String>,
    /// Only for these repositories of the workspace (repeatable).
    #[arg(long = "in-repo", value_name = "REPO")]
    pub in_repo: Vec<String>,
    /// Only for these paths, globs from the root (repeatable).
    #[arg(long = "path", value_name = "GLOB")]
    pub paths: Vec<String>,
    /// Only for these task classes: work, review, mission (repeatable).
    #[arg(long = "class", value_name = "CLASS")]
    pub classes: Vec<String>,
    /// The code it talks about: `path` or `path#symbol` (repeatable). It
    /// turns stale when that changes.
    #[arg(long = "anchor", value_name = "PATH[#SYMBOL]")]
    pub anchors: Vec<String>,
    /// Part of the core every session is given.
    #[arg(long)]
    pub core: bool,
    /// guard, gate, lint or prompt (rules only; prompt by default).
    #[arg(long)]
    pub enforcer: Option<String>,
    /// guard: tools it watches (`Edit` covers Write and MultiEdit).
    #[arg(long = "tool", value_name = "TOOL")]
    pub tools: Vec<String>,
    /// guard: paths it watches, globs from the root (repeatable).
    #[arg(long = "watch", value_name = "GLOB")]
    pub watch: Vec<String>,
    /// guard: a regular expression over the command or the written text.
    #[arg(long = "match", value_name = "REGEX")]
    pub pattern: Option<String>,
    /// guard: deny, ask or warn.
    #[arg(long)]
    pub action: Option<String>,
    /// guard: what the agent is told besides the rule.
    #[arg(long)]
    pub message: Option<String>,
    /// gate (hidden) or lint (visible): the command, `repo: cmd` for a
    /// repository of the workspace.
    #[arg(long)]
    pub check: Option<String>,
    /// proposed, shadow, enforced or advisory.
    #[arg(long)]
    pub status: Option<String>,
}

#[derive(Subcommand)]
pub enum Action {
    /// List items, grouped by kind.
    List {
        #[command(flatten)]
        home: HomeArgs,
        /// Retired ones too.
        #[arg(long)]
        all: bool,
    },
    /// One item in full.
    Show { id: String },
    /// Add a rule, fact or decision.
    Add {
        #[command(flatten)]
        home: HomeArgs,
        #[command(flatten)]
        item: ItemArgs,
    },
    /// Change an item: only the fields given change. Editing a stale item
    /// confirms it.
    Edit {
        id: String,
        #[command(flatten)]
        item: ItemArgs,
    },
    /// Stop using an item.
    Retire { id: String },
    /// One step up (proposed → shadow → enforced), or to `--to`.
    Promote {
        id: String,
        #[arg(long)]
        to: Option<String>,
    },
    /// The item still holds: hash its anchors again and use it again.
    Confirm { id: String },
    /// Whether an item may reach agents at all.
    Trust {
        id: String,
        #[arg(long)]
        untrusted: bool,
    },
    /// Hash every anchor again; items whose code changed turn stale.
    Recheck,
    /// Fold the stored events again and replace the projection.
    Rebuild,
    /// Write the items as TOML (to keep in git).
    Export {
        #[command(flatten)]
        home: HomeArgs,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Read items from a file `export` wrote (and you edited).
    Import {
        file: PathBuf,
        #[command(flatten)]
        home: HomeArgs,
    },
    /// Draft a workspace's one-page map from its scan and open it in your
    /// editor; its lines become the core every session is given.
    Init {
        workspace: String,
        /// Print the draft instead of opening an editor.
        #[arg(long, conflicts_with = "from")]
        print: bool,
        /// Take the page from this file instead of an editor.
        #[arg(long)]
        from: Option<PathBuf>,
    },
}

struct Ctx {
    service: KnowledgeService,
    workspaces: WorkspaceService,
    paths: Paths,
}

fn open() -> anyhow::Result<Ctx> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let clock = Rc::new(SystemClock);
    let ledger = Rc::new(LedgerService::new(
        Rc::new(SqliteLedger::open(&paths.db)?),
        clock.clone(),
    ));
    Ok(Ctx {
        service: service(store.clone(), ledger)?,
        workspaces: crate::workspace::service(store),
        paths,
    })
}

/// The knowledge service over the database, recording into `ledger`.
pub fn service(
    store: Rc<SqliteStore>,
    ledger: Rc<LedgerService>,
) -> anyhow::Result<KnowledgeService> {
    Ok(KnowledgeService::new(store.clone(), Rc::new(SystemClock))?
        .with_ledger(ledger)
        .with_reader(Rc::new(FsAnchorReader))
        .with_workspaces(store))
}

fn home_of(ctx: &Ctx, args: &HomeArgs) -> anyhow::Result<Home> {
    if let Some(name) = &args.workspace {
        return Ok(Home::Workspace(ctx.workspaces.find(name)?.id));
    }
    let dir = match &args.repo {
        Some(p) => std::path::absolute(p)?,
        None => std::env::current_dir()?,
    };
    if args.repo.is_none() {
        let here = dir.to_string_lossy().replace('\\', "/");
        let found = ctx.workspaces.list()?.into_iter().find(|w| {
            let root = w.root.to_string_lossy().replace('\\', "/");
            root.trim_end_matches('/')
                .eq_ignore_ascii_case(here.trim_end_matches('/'))
        });
        if let Some(w) = found {
            return Ok(Home::Workspace(w.id));
        }
    }
    Ok(Home::repo(&dir.to_string_lossy()))
}

fn home_name(ctx: &Ctx, home: &Home) -> String {
    match home {
        Home::Workspace(id) => ctx
            .workspaces
            .get(*id)
            .ok()
            .flatten()
            .map_or_else(|| home.key(), |w| format!("workspace {}", w.name)),
        Home::Repo(path) => format!("repository {path}"),
    }
}

fn id_of(s: &str) -> anyhow::Result<ItemId> {
    ItemId::parse(s).with_context(|| format!("{s:?} is not an item id (R3, F1, D2)"))
}

pub fn run(action: Action) -> anyhow::Result<()> {
    let ctx = open()?;
    let svc = &ctx.service;
    match action {
        Action::List { home, all } => {
            let home = home_of(&ctx, &home)?;
            list(&ctx, &home, all);
        }
        Action::Show { id } => {
            let item = svc
                .get(&id_of(&id)?)
                .with_context(|| format!("no item {id}"))?;
            print!("{}", show(&ctx, &item));
        }
        Action::Add { home, item } => {
            let home = home_of(&ctx, &home)?;
            let kind = item
                .kind
                .as_deref()
                .context("--kind is needed: rule, fact or decision")?;
            let mut draft = Draft::new(home, parse_kind(kind)?, "");
            apply(&mut draft, &item)?;
            let added = svc.add(draft)?;
            println!(
                "added {} ({}, {})",
                added.id,
                added.kind.as_str(),
                added.status.as_str()
            );
            if added.status == Status::Shadow {
                println!(
                    "it logs what it would do; `swarmux knowledge promote {}` makes it act",
                    added.id
                );
            }
        }
        Action::Edit { id, item } => {
            let id = id_of(&id)?;
            let old = svc.get(&id).with_context(|| format!("no item {id}"))?;
            let mut draft = draft_of(&old);
            if item.kind.is_some() {
                bail!("an item's kind can't change; retire it and add another");
            }
            apply(&mut draft, &item)?;
            let item = svc.edit(&id, draft)?;
            println!("{} edited ({})", item.id, item.status.as_str());
        }
        Action::Retire { id } => {
            let item = svc.retire(&id_of(&id)?)?;
            println!("{} retired", item.id);
        }
        Action::Promote { id, to } => {
            let id = id_of(&id)?;
            let item = match to {
                Some(to) => svc.set_status(&id, parse_status(&to)?, "promoted")?,
                None => svc.promote(&id)?,
            };
            println!("{} is {}", item.id, item.status.as_str());
        }
        Action::Confirm { id } => {
            let item = svc.confirm(&id_of(&id)?)?;
            println!("{} confirmed ({})", item.id, item.status.as_str());
        }
        Action::Trust { id, untrusted } => {
            let trust = if untrusted {
                Trust::Untrusted
            } else {
                Trust::Verified
            };
            let item = svc.trust(&id_of(&id)?, trust)?;
            println!("{} is {}", item.id, item.trust.as_str());
        }
        Action::Recheck => {
            let stale = svc.recheck_all()?;
            if stale.is_empty() {
                println!("every anchor is as it was");
            } else {
                let ids: Vec<String> = stale.iter().map(ToString::to_string).collect();
                println!(
                    "stale now: {} (withheld from agents until confirmed or edited)",
                    ids.join(", ")
                );
            }
        }
        Action::Rebuild => {
            let r = svc.rebuild()?;
            println!(
                "{} events folded into {} items; {} differed from the stored projection",
                r.events,
                r.items,
                r.differed.len()
            );
            for id in r.differed {
                println!("  {id}");
            }
        }
        Action::Export { home, out } => {
            let home = home_of(&ctx, &home)?;
            let text = TomlKnowledge.encode(&svc.export(&home));
            match out {
                Some(path) => {
                    std::fs::write(&path, text)
                        .with_context(|| format!("cannot write {}", path.display()))?;
                    println!("wrote {}", path.display());
                }
                None => print!("{text}"),
            }
        }
        Action::Import { file, home } => {
            let home = home_of(&ctx, &home)?;
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let items = TomlKnowledge
                .decode(&text, &home)
                .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
            let done = svc.import(items)?;
            println!(
                "{} added, {} changed, {} unchanged",
                done.added, done.changed, done.unchanged
            );
        }
        Action::Init {
            workspace,
            print,
            from,
        } => init(&ctx, &workspace, print, from.as_deref())?,
    }
    Ok(())
}

/// Hashes the anchors of a workspace's items again after a rescan.
pub fn recheck_workspace(id: swarmux_domain::WorkspaceId) -> anyhow::Result<()> {
    let ctx = open()?;
    let stale = ctx.service.recheck(&Home::Workspace(id))?;
    if !stale.is_empty() {
        let ids: Vec<String> = stale.iter().map(ToString::to_string).collect();
        println!(
            "knowledge stale now: {} (confirm or edit them: `swarmux knowledge list`)",
            ids.join(", ")
        );
    }
    Ok(())
}

/// `swarmux rules`: the registry at a glance and every rule.
pub fn rules(home: HomeArgs) -> anyhow::Result<()> {
    let ctx = open()?;
    let home = home_of(&ctx, &home)?;
    let registry = ctx.service.registry(Some(&home));
    println!("{}: {registry}", home_name(&ctx, &home));
    if registry.shadow + registry.stale + registry.proposed > 0 {
        println!(
            "  {} in shadow · {} stale · {} proposed",
            registry.shadow, registry.stale, registry.proposed
        );
    }
    for i in ctx
        .service
        .of_home(&home)
        .into_iter()
        .filter(|i| i.kind == Kind::Rule && i.status != Status::Retired)
    {
        println!(
            "{:<5} {:<9} {:<7} hits {:<4} shadow {:<4} {}",
            i.id,
            i.status.as_str(),
            i.enforcer_kind().map_or(UNKNOWN, EnforcerKind::as_str),
            i.counters.hits,
            i.counters.shadow_hits,
            i.text
        );
    }
    if registry.advisory_only > 0 {
        println!(
            "advisory-only rules are text an agent may ignore: give them a guard, gate or lint enforcer"
        );
    }
    Ok(())
}

fn list(ctx: &Ctx, home: &Home, all: bool) {
    let items: Vec<Item> = ctx
        .service
        .of_home(home)
        .into_iter()
        .filter(|i| all || i.status != Status::Retired)
        .collect();
    println!(
        "{}: {}",
        home_name(ctx, home),
        ctx.service.registry(Some(home))
    );
    if items.is_empty() {
        println!("no items: add one with `swarmux knowledge add --kind rule --text \"...\"`");
        return;
    }
    for kind in [Kind::Rule, Kind::Fact, Kind::Decision] {
        let group: Vec<&Item> = items.iter().filter(|i| i.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        println!("{}s", kind.as_str());
        for i in group {
            let core = if i.core { " core" } else { "" };
            let trust = if i.trust == Trust::Untrusted {
                " untrusted"
            } else {
                ""
            };
            println!(
                "  {:<5} {:<9}{core}{trust}  {}",
                i.id,
                i.status.as_str(),
                i.text
            );
        }
    }
}

fn show(ctx: &Ctx, i: &Item) -> String {
    let list = |v: &[String]| {
        if v.is_empty() {
            "all".to_string()
        } else {
            v.join(", ")
        }
    };
    let mut out = format!(
        "{} {} · {} · {} · {}{}\n  {}\n",
        i.id,
        i.kind.as_str(),
        i.status.as_str(),
        i.provenance.source.as_str(),
        i.trust.as_str(),
        if i.core { " · core" } else { "" },
        i.text
    );
    out.push_str(&format!(
        "  why: {}\n",
        if i.why.is_empty() { UNKNOWN } else { &i.why }
    ));
    out.push_str(&format!("  home: {}\n", home_name(ctx, &i.home)));
    out.push_str(&format!(
        "  scope: repos {} · paths {} · classes {}\n",
        list(&i.scope.repos),
        list(&i.scope.paths),
        list(&i.scope.classes)
    ));
    for a in &i.anchors {
        out.push_str(&format!(
            "  anchor: {} ({})\n",
            a.spec(),
            if a.hash.is_empty() {
                UNKNOWN
            } else {
                &a.hash[..a.hash.len().min(12)]
            }
        ));
    }
    match &i.enforcer {
        Some(Enforcer::Guard(g)) => out.push_str(&format!(
            "  enforcer: guard {} · tools {} · watches {} · match {}\n",
            g.action.as_str(),
            list(&g.tools),
            list(&g.paths),
            g.pattern.as_deref().unwrap_or(UNKNOWN)
        )),
        Some(Enforcer::Gate(c)) => {
            out.push_str(&format!("  enforcer: gate (hidden) · {}\n", c.command));
        }
        Some(Enforcer::Lint(c)) => out.push_str(&format!("  enforcer: lint · {}\n", c.command)),
        Some(Enforcer::Prompt) => out.push_str("  enforcer: prompt (advisory text)\n"),
        None => {}
    }
    let check = match i.last_check {
        Some(true) => "passed",
        Some(false) => "failed",
        None => UNKNOWN,
    };
    out.push_str(&format!(
        "  hits {} · shadow hits {} · last used {} · last check {check}\n",
        i.counters.hits,
        i.counters.shadow_hits,
        i.counters.last_used.map_or(UNKNOWN.to_string(), iso_minute)
    ));
    let events: Vec<String> = i
        .provenance
        .events
        .iter()
        .map(|s| format!("#{s}"))
        .collect();
    out.push_str(&format!(
        "  ledger: {}\n",
        if events.is_empty() {
            UNKNOWN.to_string()
        } else {
            events.join(" ")
        }
    ));
    out
}

fn parse_kind(s: &str) -> anyhow::Result<Kind> {
    match Kind::parse(s) {
        Some(k) if !k.is_reserved() => Ok(k),
        _ => bail!("{s:?} is not a kind: rule, fact or decision"),
    }
}

fn parse_status(s: &str) -> anyhow::Result<Status> {
    Status::parse(s).with_context(|| {
        format!("{s:?} is not a status: proposed, shadow, enforced, advisory, retired")
    })
}

/// The draft that recreates `item` (for an edit).
fn draft_of(item: &Item) -> Draft {
    Draft {
        home: item.home.clone(),
        kind: item.kind,
        text: item.text.clone(),
        why: item.why.clone(),
        scope: item.scope.clone(),
        anchors: item.anchors.clone(),
        core: item.core,
        enforcer: item.enforcer.clone(),
        status: None,
        source: item.provenance.source,
    }
}

/// `args` laid over `draft`: only what was given.
fn apply(draft: &mut Draft, args: &ItemArgs) -> anyhow::Result<()> {
    if let Some(t) = &args.text {
        draft.text = t.clone();
    }
    if let Some(w) = &args.why {
        draft.why = w.clone();
    }
    let scope = Scope {
        repos: args.in_repo.clone(),
        paths: args.paths.clone(),
        classes: args.classes.clone(),
    };
    if !scope.repos.is_empty() {
        draft.scope.repos = scope.repos;
    }
    if !scope.paths.is_empty() {
        draft.scope.paths = scope.paths;
    }
    if !scope.classes.is_empty() {
        draft.scope.classes = scope.classes;
    }
    if !args.anchors.is_empty() {
        draft.anchors = args.anchors.iter().map(|a| Anchor::parse(a)).collect();
    }
    if args.core {
        draft.core = true;
    }
    if let Some(s) = &args.status {
        draft.status = Some(parse_status(s)?);
    }
    let kind = match &args.enforcer {
        Some(e) => Some(
            EnforcerKind::parse(e)
                .with_context(|| format!("{e:?} is not an enforcer: guard, gate, lint, prompt"))?,
        ),
        None => draft.enforcer.as_ref().map(Enforcer::kind),
    };
    let guard_given = !args.tools.is_empty()
        || !args.watch.is_empty()
        || args.pattern.is_some()
        || args.action.is_some()
        || args.message.is_some();
    draft.enforcer = match kind {
        Some(EnforcerKind::Guard) => {
            let mut g = match &draft.enforcer {
                Some(Enforcer::Guard(g)) => g.clone(),
                _ => GuardRule {
                    tools: Vec::new(),
                    paths: Vec::new(),
                    pattern: None,
                    action: RuleAction::Ask,
                    message: String::new(),
                },
            };
            if !args.tools.is_empty() {
                g.tools = args.tools.clone();
            }
            if !args.watch.is_empty() {
                g.paths = args.watch.clone();
            }
            if let Some(p) = &args.pattern {
                g.pattern = Some(p.clone()).filter(|p| !p.is_empty());
            }
            if let Some(a) = &args.action {
                g.action = RuleAction::parse(a)
                    .with_context(|| format!("{a:?} is not an action: deny, ask, warn"))?;
            }
            if let Some(m) = &args.message {
                g.message = m.clone();
            }
            Some(Enforcer::Guard(g))
        }
        Some(k @ (EnforcerKind::Gate | EnforcerKind::Lint)) => {
            let command = match (&args.check, &draft.enforcer) {
                (Some(c), _) => c.clone(),
                (None, Some(Enforcer::Gate(c) | Enforcer::Lint(c))) => c.command.clone(),
                _ => bail!("a {} rule needs --check \"command\"", k.as_str()),
            };
            let rule = CheckRule { command };
            Some(if k == EnforcerKind::Gate {
                Enforcer::Gate(rule)
            } else {
                Enforcer::Lint(rule)
            })
        }
        Some(EnforcerKind::Prompt) => Some(Enforcer::Prompt),
        None => None,
    };
    if guard_given && !matches!(draft.enforcer, Some(Enforcer::Guard(_))) {
        bail!("--tool, --watch, --match, --action and --message are for --enforcer guard");
    }
    if draft.kind == Kind::Rule && draft.enforcer.is_none() {
        draft.enforcer = Some(Enforcer::Prompt);
    }
    Ok(())
}

/// `swarmux knowledge init`: the map drafted, edited, saved as the core.
fn init(ctx: &Ctx, name: &str, print: bool, from: Option<&Path>) -> anyhow::Result<()> {
    let w = ctx.workspaces.find(name)?;
    let home = Home::Workspace(w.id);
    let core: Vec<String> = ctx
        .service
        .core(&home)
        .into_iter()
        .map(|i| i.text)
        .collect();
    let page = map::draft(&w, &core);
    if print {
        print!("{page}");
        return Ok(());
    }
    let edited = match from {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?,
        None => {
            let dir = ctx.paths.data_dir.join("workspaces").join(w.id.to_string());
            std::fs::create_dir_all(&dir)?;
            let file = dir.join("map.md");
            std::fs::write(&file, &page)?;
            edit_in_editor(&file)?;
            std::fs::read_to_string(&file)?
        }
    };
    let facts = map::facts(&edited);
    if facts.is_empty() {
        bail!("the map has no lines left; nothing was saved");
    }
    let saved = ctx.service.save_map(&home, &facts)?;
    println!(
        "the map of {} has {} lines: {} new, {} kept, {} dropped. Every session of it starts with them.",
        w.name,
        facts.len(),
        saved.added,
        saved.kept,
        saved.retired
    );
    Ok(())
}

/// Opens `file` in `$VISUAL`, `$EDITOR`, or the system's plain editor,
/// and waits for it to close.
fn edit_in_editor(file: &Path) -> anyhow::Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "vi".into()
            }
        });
    let mut words = editor.split_whitespace();
    let program = words.next().context("no editor")?;
    let status = std::process::Command::new(program)
        .args(words)
        .arg(file)
        .status()
        .with_context(|| format!("cannot start the editor {editor:?} (set EDITOR)"))?;
    if !status.success() {
        bail!("the editor exited with {status}; nothing was saved");
    }
    Ok(())
}
