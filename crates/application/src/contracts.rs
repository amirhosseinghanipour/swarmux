//! Cross-repo contracts: what each repository of a workspace provides and
//! who consumes it, kept up to date by reading only the files that changed.
//!
//! The domain (`swarmux_domain::contracts`) reads one file at a time and
//! folds the facts into an [`Index`]. This module is the rest: the ports
//! ([`ContractStore`] keeps what each file yielded, [`SourceTree`] lists
//! and reads files), the incremental [`scan_tree`], the
//! [`ContractService`] the app and the command line use, and the
//! [`ContractHub`] the hook transport and the done-gate read from other
//! threads (delivery of "consumed by …" notes, and the cross-repo
//! consistency check of a mission's gate).
//!
//! Nothing here asks a model anything: every answer comes from the sources.

mod hub;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use swarmux_domain::contracts::{self, FileFacts, Index, Provided, Report, Site};
use swarmux_domain::knowledge::content_hash;
use swarmux_domain::ledger::Payload;
use swarmux_domain::mission::Mission;
use swarmux_domain::{
    AttentionId, AttentionItem, AttentionKind, JobId, Likelihood, Millis, Priority, Workspace,
    WorkspaceId,
};

use crate::jobs::{JobStart, JobStarts};
use crate::ledger::LedgerService;
use crate::missions::MissionWatch;
use crate::ports::Clock;
use crate::workspace::WorkspaceStore;
use crate::{AppError, AppResult};

pub use hub::{ContractGate, ContractHub, ContractJudgement, PullContext};

/// What one file yielded last time it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub repo: String,
    /// Relative to the repository, `/`.
    pub path: String,
    pub size: u64,
    /// Modified time, in milliseconds; a cheap hint that skips reading.
    pub mtime: i64,
    /// The content hash: when it is unchanged the facts are too.
    pub hash: String,
    pub facts: Vec<contracts::Fact>,
}

type Cache = HashMap<(String, String), FileRecord>;

/// Where what each file yielded is kept between runs.
pub trait ContractStore {
    fn records(&self, workspace: WorkspaceId) -> AppResult<Vec<FileRecord>>;
    /// Stores `upserts` and forgets the `(repo, path)`s in `removed`.
    fn apply(
        &self,
        workspace: WorkspaceId,
        upserts: &[FileRecord],
        removed: &[(String, String)],
    ) -> AppResult<()>;
}

/// A file a listing found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
}

/// Lists and reads the files of a repository folder.
pub trait SourceTree: Send + Sync {
    /// The files below `dir` whose repository-relative `/` path `wanted`
    /// accepts. Build output, dependency and VCS folders are skipped.
    fn files(&self, dir: &Path, wanted: &dyn Fn(&str) -> bool) -> Vec<SourceFile>;
    /// The text of `rel` below `dir`; `None` when it can't be read.
    fn read(&self, dir: &Path, rel: &str) -> Option<String>;
}

/// What a scan did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Files an extractor reads that the tree has.
    pub files: usize,
    /// Files opened (the others were unchanged by size and time).
    pub read: usize,
    /// Files whose content changed, so an extractor ran again.
    pub extracted: usize,
    /// Files that are gone.
    pub removed: usize,
}

/// The result of reading a tree.
pub struct Scan {
    pub index: Index,
    /// Every file's record, as of now.
    pub records: Vec<FileRecord>,
    /// Those that differ from the cache.
    pub upserts: Vec<FileRecord>,
    pub removed: Vec<(String, String)>,
    pub stats: ScanStats,
}

/// Reads the files of `repos` (name, folder below `tree`) that an
/// extractor wants, running an extractor only where the content hash
/// differs from `cache`'s. Files whose size and time are unchanged are not
/// even opened.
pub fn scan_tree(
    source: &dyn SourceTree,
    tree: &Path,
    repos: &[(String, String)],
    cache: &Cache,
) -> Scan {
    let mut stats = ScanStats::default();
    let mut records: Vec<FileRecord> = Vec::new();
    let mut upserts = Vec::new();
    for (repo, folder) in repos {
        let dir = if folder == "." {
            tree.to_path_buf()
        } else {
            folder.split('/').fold(tree.to_path_buf(), |p, c| p.join(c))
        };
        // A repository at the root does not own the other repositories.
        let nested = |rel: &str| {
            folder == "."
                && repos
                    .iter()
                    .any(|(_, f)| f != "." && rel.starts_with(&format!("{f}/")))
        };
        for f in source.files(&dir, &contracts::wanted) {
            if nested(&f.rel) {
                continue;
            }
            stats.files += 1;
            let key = (repo.clone(), f.rel.clone());
            let old = cache.get(&key);
            if let Some(o) = old.filter(|o| o.size == f.size && o.mtime == f.mtime) {
                records.push(o.clone());
                continue;
            }
            let Some(text) = source.read(&dir, &f.rel) else {
                continue;
            };
            stats.read += 1;
            let hash = content_hash(Some(&text), None);
            let facts = match old {
                Some(o) if o.hash == hash => o.facts.clone(),
                _ => {
                    stats.extracted += 1;
                    contracts::extract(&f.rel, &text)
                }
            };
            let record = FileRecord {
                repo: repo.clone(),
                path: f.rel,
                size: f.size,
                mtime: f.mtime,
                hash,
                facts,
            };
            if old != Some(&record) {
                upserts.push(record.clone());
            }
            records.push(record);
        }
    }
    let seen: BTreeSet<(&str, &str)> = records
        .iter()
        .map(|r| (r.repo.as_str(), r.path.as_str()))
        .collect();
    let scanned: BTreeSet<&str> = repos.iter().map(|(r, _)| r.as_str()).collect();
    let removed: Vec<(String, String)> = cache
        .keys()
        .filter(|(r, p)| scanned.contains(r.as_str()) && !seen.contains(&(r.as_str(), p.as_str())))
        .cloned()
        .collect();
    stats.removed = removed.len();
    let index = index_of(&records);
    Scan {
        index,
        records,
        upserts,
        removed,
        stats,
    }
}

/// The index over `records`.
pub fn index_of(records: &[FileRecord]) -> Index {
    let files: Vec<FileFacts<'_>> = records
        .iter()
        .map(|r| FileFacts {
            repo: &r.repo,
            path: &r.path,
            hash: &r.hash,
            facts: &r.facts,
        })
        .collect();
    Index::build(&files)
}

/// What a rescan of a workspace found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rescanned {
    pub stats: ScanStats,
    pub providers: usize,
    pub consumers: usize,
    /// Provided symbols that are gone or changed since the last scan, as
    /// `table orders (backend/…:3)`.
    pub changed: Vec<String>,
}

/// One provided symbol and how many places use it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractRow {
    pub provided: Provided,
    pub consumers: usize,
}

/// The places a symbol is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Consumed {
    pub provided: Provided,
    pub shape: String,
    pub sites: Vec<Site>,
}

/// A ledger-worthy fact about contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractEvent {
    /// A mission job's gate compared the contracts.
    Checked {
        job: JobId,
        changed: usize,
        consumers: usize,
        repos: Vec<String>,
        dangling: Vec<String>,
        outside: Vec<String>,
        passed: bool,
    },
    /// A note reached an agent.
    Injected {
        job: JobId,
        keys: Vec<String>,
        tokens: usize,
    },
}

/// A change that breaks repositories outside its mission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escalation {
    pub job: JobId,
    /// `app/lib/orders.dart:42`, …
    pub places: Vec<String>,
    pub repos: Vec<String>,
}

/// The contract use cases.
pub struct ContractService {
    store: Rc<dyn ContractStore>,
    workspaces: Rc<dyn WorkspaceStore>,
    source: Arc<dyn SourceTree>,
    clock: Rc<dyn Clock>,
    ledger: Option<Rc<LedgerService>>,
    hub: Arc<ContractHub>,
    /// When an escalation first showed.
    seen: RefCell<HashMap<JobId, Millis>>,
}

impl ContractService {
    pub fn new(
        store: Rc<dyn ContractStore>,
        workspaces: Rc<dyn WorkspaceStore>,
        source: Arc<dyn SourceTree>,
        clock: Rc<dyn Clock>,
    ) -> ContractService {
        ContractService {
            store,
            workspaces,
            hub: ContractHub::new(source.clone()),
            source,
            clock,
            ledger: None,
            seen: RefCell::default(),
        }
    }

    /// Records checks and deliveries in the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> ContractService {
        self.ledger = Some(ledger);
        self
    }

    /// What the hook transport and the gate read.
    pub fn hub(&self) -> Arc<ContractHub> {
        self.hub.clone()
    }

    fn repos_of(w: &Workspace) -> Vec<(String, String)> {
        w.repos
            .iter()
            .map(|r| (r.name.clone(), r.path.clone()))
            .collect()
    }

    fn cache_of(&self, w: WorkspaceId) -> AppResult<Cache> {
        Ok(self
            .store
            .records(w)?
            .into_iter()
            .map(|r| ((r.repo.clone(), r.path.clone()), r))
            .collect())
    }

    /// Reads the workspace's files that changed since the last scan and
    /// stores what they yielded. Cheap when little changed.
    pub fn rescan(&self, workspace: &Workspace) -> AppResult<Rescanned> {
        let before = self.index(workspace.id)?;
        let cache = self.cache_of(workspace.id)?;
        let scan = scan_tree(
            &*self.source,
            &workspace.root,
            &Self::repos_of(workspace),
            &cache,
        );
        if !scan.upserts.is_empty() || !scan.removed.is_empty() {
            self.store
                .apply(workspace.id, &scan.upserts, &scan.removed)?;
        }
        let changed = Report::of(&before, &scan.index)
            .impacts
            .iter()
            .map(|i| format!("{} ({})", i.before.sym.label(), i.before.place()))
            .collect();
        let out = Rescanned {
            stats: scan.stats,
            providers: scan.index.provided().count(),
            consumers: scan.index.sites().len(),
            changed,
        };
        let fresh: Cache = scan
            .records
            .iter()
            .cloned()
            .map(|r| ((r.repo.clone(), r.path.clone()), r))
            .collect();
        self.hub.set_cache(workspace.id, Arc::new(fresh));
        self.hub.set_index(workspace.id, Arc::new(scan.index));
        Ok(out)
    }

    /// [`rescan`](Self::rescan) for every workspace.
    pub fn rescan_all(&self) -> AppResult<Vec<(String, Rescanned)>> {
        let mut out = Vec::new();
        for w in self.workspaces.list()? {
            out.push((w.name.clone(), self.rescan(&w)?));
        }
        Ok(out)
    }

    /// The workspace's index as last scanned (read from the store the
    /// first time).
    pub fn index(&self, workspace: WorkspaceId) -> AppResult<Arc<Index>> {
        if let Some(ix) = self.hub.index(workspace) {
            return Ok(ix);
        }
        let records = self.store.records(workspace)?;
        let index = Arc::new(index_of(&records));
        let cache: Cache = records
            .into_iter()
            .map(|r| ((r.repo.clone(), r.path.clone()), r))
            .collect();
        self.hub.set_cache(workspace, Arc::new(cache));
        self.hub.set_index(workspace, index.clone());
        Ok(index)
    }

    /// Every provided symbol of the workspace with its number of
    /// consumers, most consumed first.
    pub fn rows(&self, workspace: WorkspaceId) -> AppResult<Vec<ContractRow>> {
        let ix = self.index(workspace)?;
        let mut rows: Vec<ContractRow> = ix
            .provided()
            .map(|p| ContractRow {
                consumers: ix.consumers_of(p).len(),
                provided: p.clone(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.consumers
                .cmp(&a.consumers)
                .then(a.provided.sym.cmp(&b.provided.sym))
                .then(a.provided.repo.cmp(&b.provided.repo))
        });
        Ok(rows)
    }

    /// Who consumes the symbols `query` names (`orders`, `table orders`,
    /// `orders.total`, `get_orders`, `GET /api/orders`).
    pub fn consumers(&self, workspace: WorkspaceId, query: &str) -> AppResult<Vec<Consumed>> {
        let ix = self.index(workspace)?;
        Ok(ix
            .find(query)
            .into_iter()
            .map(|p| Consumed {
                shape: ix.shape(p),
                sites: ix.consumers_of(p).into_iter().cloned().collect(),
                provided: p.clone(),
            })
            .collect())
    }

    /// Records what the gate checked and what reached agents.
    pub fn pump(&self) -> Vec<AppError> {
        let mut errors = Vec::new();
        let Some(ledger) = &self.ledger else {
            self.hub.drain();
            return errors;
        };
        let now = self.clock.now();
        for e in self.hub.drain() {
            let (job, payload) = match e {
                ContractEvent::Checked {
                    job,
                    changed,
                    consumers,
                    repos,
                    dangling,
                    outside,
                    passed,
                } => (
                    job,
                    Payload::new()
                        .str("event", "contracts_check")
                        .bool("passed", passed)
                        .int("changed", changed as i64)
                        .int("consumers", consumers as i64)
                        .strs("repos", &repos)
                        .strs("dangling", &dangling)
                        .strs("outside", &outside),
                ),
                ContractEvent::Injected { job, keys, tokens } => (
                    job,
                    Payload::new()
                        .str("event", "contracts_injected")
                        .strs("items", &keys)
                        .int("tokens", tokens as i64),
                ),
            };
            if let Err(e) = ledger.record_knowledge(Some(job), payload, now) {
                errors.push(e);
            }
        }
        errors
    }

    /// One inbox item per job whose change breaks repositories its
    /// mission does not include.
    pub fn attention(&self) -> Vec<AttentionItem> {
        let now = self.clock.now();
        let mut seen = self.seen.borrow_mut();
        let current = self.hub.escalations();
        seen.retain(|j, _| current.iter().any(|e| e.job == *j));
        current
            .into_iter()
            .map(|e| {
                let since = *seen.entry(e.job).or_insert(now);
                let first = e.places.first().cloned().unwrap_or_default();
                AttentionItem {
                    id: AttentionId::new(format!("contracts:outside:{}", e.job.0)),
                    kind: AttentionKind::Knowledge,
                    priority: Priority::P2,
                    title: format!(
                        "this change breaks {first}{} which isn't part of the mission",
                        if e.places.len() > 1 {
                            format!(" and {} more places", e.places.len() - 1)
                        } else {
                            String::new()
                        }
                    ),
                    detail: format!(
                        "job {} changed what its repositories provide but did not update {}; add them to the mission or revert the change",
                        e.job.0,
                        e.repos.join(", ")
                    ),
                    session: None,
                    job: Some(e.job),
                    request: None,
                    since,
                    deadline: None,
                    likelihood: Likelihood::Certain,
                    risk: None,
                    signature: None,
                    batch: e.places.len(),
                    suggest: None,
                }
            })
            .collect()
    }

    /// The tree a started job works in, read now, becomes its baseline.
    fn baseline(&self, start: &JobStart<'_>) -> AppResult<()> {
        let Some(id) = start.session.workspace else {
            return Ok(());
        };
        let Some(w) = self.workspaces.get(id)? else {
            return Ok(());
        };
        let index = self.index(id)?;
        let cache = self.hub.cache(id);
        let repos = Self::repos_of(&w);
        let scan = scan_tree(&*self.source, start.cwd, &repos, &cache);
        // The first job of a workspace nobody scanned yet: keep what was
        // read for the next job and the gate.
        if index.is_empty() && !scan.records.is_empty() {
            let _ = self.store.apply(id, &scan.upserts, &scan.removed);
            let fresh: Cache = scan
                .records
                .iter()
                .cloned()
                .map(|r| ((r.repo.clone(), r.path.clone()), r))
                .collect();
            self.hub.set_cache(id, Arc::new(fresh));
        }
        self.hub.register(
            start.job,
            hub::JobTree {
                root: start.cwd.to_path_buf(),
                repos,
                commands: w
                    .repos
                    .iter()
                    .map(|r| {
                        (
                            r.name.clone(),
                            r.commands
                                .iter()
                                .map(|c| (c.name.clone(), c.line.clone()))
                                .collect(),
                        )
                    })
                    .collect(),
                baseline: Arc::new(scan.index),
                cache: Arc::new(
                    scan.records
                        .into_iter()
                        .map(|r| ((r.repo.clone(), r.path.clone()), r))
                        .collect(),
                ),
            },
        );
        Ok(())
    }
}

impl JobStarts for ContractService {
    fn started(&self, start: &JobStart<'_>) -> AppResult<()> {
        self.baseline(start)
    }
}

impl MissionWatch for ContractService {
    fn launched(&self, job: JobId, mission: &Mission) {
        self.hub.set_mission(
            job,
            mission.contract.repos.clone(),
            mission.contract.acceptance.checks.clone(),
        );
    }

    fn delivered(&self, mission: &Mission) -> AppResult<()> {
        let Some(id) = mission.workspace else {
            return Ok(());
        };
        if let Some(w) = self.workspaces.get(id)? {
            self.rescan(&w)?;
        }
        Ok(())
    }
}

/// Tells several [`JobStarts`] about every job.
pub struct StartFan(pub Vec<Rc<dyn JobStarts>>);

impl JobStarts for StartFan {
    fn started(&self, start: &JobStart<'_>) -> AppResult<()> {
        let mut first = Ok(());
        for s in &self.0 {
            let r = s.started(start);
            if first.is_ok() {
                first = r;
            }
        }
        first
    }
}

/// Tells several [`MissionWatch`]es about every launch and delivery.
pub struct WatchFan(pub Vec<Rc<dyn MissionWatch>>);

impl MissionWatch for WatchFan {
    fn launched(&self, job: JobId, mission: &Mission) {
        self.0.iter().for_each(|w| w.launched(job, mission));
    }

    fn delivered(&self, mission: &Mission) -> AppResult<()> {
        let mut first = Ok(());
        for w in &self.0 {
            let r = w.delivered(mission);
            if first.is_ok() {
                first = r;
            }
        }
        first
    }
}

#[cfg(test)]
mod tests;
