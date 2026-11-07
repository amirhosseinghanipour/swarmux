//! Project knowledge: the use cases over rules, facts and decisions, and
//! the ports they need.
//!
//! Every change is an event, stored and chained into the ledger; the items
//! are the projection folded from the events, kept in memory and in the
//! store, and rebuildable at any time. Anchors are hashed again when a
//! workspace is scanned, when a job starts and after a mission lands; an
//! item whose anchors changed turns stale and is withheld until the owner
//! confirms or edits it.
//!
//! The [`KnowledgeHub`] carries the items to the hook transport, which
//! pushes the core when a session starts (and after each compaction),
//! pulls the items a file is scoped to when an agent reads or edits it,
//! and lets guard rules deny or hold calls. Gate and lint rules reach the
//! done-gate through [`RuleChecks`].

mod hub;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use swarmux_domain::knowledge::anchor::changed;
use swarmux_domain::knowledge::{
    Anchor, Change, Draft, Event, Home, Item, ItemId, JobScope, Kind, Projection, Registry, Status,
    Trust, check_move, map, valid_for,
};
use swarmux_domain::ledger::Payload;
use swarmux_domain::mission::Mission;
use swarmux_domain::{AttentionId, AttentionItem, AttentionKind, JobId, Likelihood, Priority};

use crate::jobs::{JobStart, JobStarts};
use crate::ledger::LedgerService;
use crate::missions::MissionWatch;
use crate::ports::Clock;
use crate::workspace::WorkspaceStore;
use crate::{AppError, AppResult};

pub use hub::{
    JobInfo, KnowledgeHub, NoToolContext, RuleCheck, RuleChecks, RuleGuard, ToolContext, Usage,
};

/// Where knowledge events and the projection are kept.
pub trait KnowledgeStore {
    /// Stores `event` and `item` as the event left it; returns the
    /// event's `seq`.
    fn append(&self, event: &Event, item: &Item) -> AppResult<u64>;
    /// Every event, in `seq` order.
    fn events(&self) -> AppResult<Vec<Event>>;
    /// The newest event's `seq`; 0 when there is none.
    fn last_seq(&self) -> AppResult<u64>;
    /// The stored projection.
    fn items(&self) -> AppResult<Vec<Item>>;
    /// Replaces the stored projection.
    fn replace_items(&self, items: &[Item]) -> AppResult<()>;
}

/// Reads what anchors point at.
pub trait AnchorReader {
    /// The text of `rel` below `root`; `None` when it can't be read.
    fn read(&self, root: &Path, rel: &str) -> Option<String>;
}

/// Turns items into a file owners keep in git, and back.
pub trait KnowledgeCodec {
    fn encode(&self, items: &[Item]) -> String;
    /// The items of `text`, all of `home`.
    fn decode(&self, text: &str, home: &Home) -> Result<Vec<Item>, String>;
}

/// What [`KnowledgeService::rebuild`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rebuilt {
    pub events: usize,
    pub items: usize,
    /// Items the stored projection had differently (or not at all).
    pub differed: Vec<ItemId>,
}

/// What saving the map did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapSaved {
    pub added: usize,
    pub kept: usize,
    pub retired: usize,
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
}

struct State {
    projection: Projection,
    /// The newest event `seq` folded in.
    seen: u64,
}

/// The knowledge use cases.
pub struct KnowledgeService {
    store: Rc<dyn KnowledgeStore>,
    clock: Rc<dyn Clock>,
    ledger: Option<Rc<LedgerService>>,
    reader: Option<Rc<dyn AnchorReader>>,
    workspaces: Option<Rc<dyn WorkspaceStore>>,
    hub: Arc<KnowledgeHub>,
    state: RefCell<State>,
}

/// What the owner's own changes are recorded as.
const OWNER: &str = "owner";
const SWARMUX: &str = "swarmux";

impl KnowledgeService {
    /// Folds the stored events.
    pub fn new(store: Rc<dyn KnowledgeStore>, clock: Rc<dyn Clock>) -> AppResult<KnowledgeService> {
        let events = store.events()?;
        let seen = events.last().map_or(0, |e| e.seq);
        let service = KnowledgeService {
            store,
            clock,
            ledger: None,
            reader: None,
            workspaces: None,
            hub: KnowledgeHub::new(),
            state: RefCell::new(State {
                projection: Projection::fold(&events),
                seen,
            }),
        };
        service.publish();
        Ok(service)
    }

    /// Records every change in the ledger.
    pub fn with_ledger(mut self, ledger: Rc<LedgerService>) -> KnowledgeService {
        self.ledger = Some(ledger);
        self
    }

    /// Hashes anchors with `reader`; without one, anchors never go stale.
    pub fn with_reader(mut self, reader: Rc<dyn AnchorReader>) -> KnowledgeService {
        self.reader = Some(reader);
        self
    }

    /// Finds workspaces' roots and repositories.
    pub fn with_workspaces(mut self, workspaces: Rc<dyn WorkspaceStore>) -> KnowledgeService {
        self.workspaces = Some(workspaces);
        self
    }

    /// What the hook transport reads.
    pub fn hub(&self) -> Arc<KnowledgeHub> {
        self.hub.clone()
    }

    fn publish(&self) {
        let items = self.state.borrow().projection.items().cloned().collect();
        self.hub.publish(items);
    }

    /// Every item, by id.
    pub fn list(&self) -> Vec<Item> {
        self.state.borrow().projection.items().cloned().collect()
    }

    /// The items of `home`, by id.
    pub fn of_home(&self, home: &Home) -> Vec<Item> {
        self.state
            .borrow()
            .projection
            .items()
            .filter(|i| i.home.same(home))
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &ItemId) -> Option<Item> {
        self.state.borrow().projection.get(id).cloned()
    }

    fn must(&self, id: &ItemId) -> AppResult<Item> {
        self.get(id)
            .ok_or_else(|| AppError::Knowledge(format!("no item {id}")))
    }

    /// The registry of `home`, or of everything.
    pub fn registry(&self, home: Option<&Home>) -> Registry {
        let s = self.state.borrow();
        Registry::of(
            s.projection
                .items()
                .filter(|i| home.is_none_or(|h| i.home.same(h))),
        )
    }

    /// The folder a home's anchors are relative to.
    pub fn root_of(&self, home: &Home) -> AppResult<Option<PathBuf>> {
        match home {
            Home::Repo(path) => Ok(Some(PathBuf::from(path))),
            Home::Workspace(id) => match &self.workspaces {
                Some(store) => Ok(store.get(*id)?.map(|w| w.root)),
                None => Ok(None),
            },
        }
    }

    /// `anchors` hashed as their files are now under `home`'s root.
    fn hash(&self, home: &Home, anchors: &[Anchor]) -> AppResult<Vec<Anchor>> {
        let root = self.root_of(home)?;
        Ok(anchors
            .iter()
            .map(|a| match (&self.reader, &root) {
                (Some(reader), Some(root)) => a.hashed(reader.read(root, &a.path).as_deref()),
                _ => a.clone(),
            })
            .collect())
    }

    /// Records `change` of `id`: in the ledger, then the store, then the
    /// projection. Returns the item as it is after.
    fn commit(&self, id: &ItemId, actor: &str, change: Change) -> AppResult<Item> {
        let item = self.commit_quiet(id, actor, change, None)?;
        self.publish();
        Ok(item)
    }

    /// Like [`commit`](Self::commit) without publishing, and with the
    /// ledger entry already written when `ledger` is given.
    fn commit_quiet(
        &self,
        id: &ItemId,
        actor: &str,
        change: Change,
        ledger: Option<u64>,
    ) -> AppResult<Item> {
        let now = self.clock.now();
        let ledger = match (ledger, &self.ledger) {
            (Some(seq), _) => Some(seq),
            (None, Some(l)) => {
                let home = match &change {
                    Change::Put(i) => i.home.key(),
                    _ => self.get(id).map(|i| i.home.key()).unwrap_or_default(),
                };
                Some(l.record_knowledge(None, describe(id, &home, actor, &change), now)?)
            }
            (None, None) => None,
        };
        let mut event = Event {
            seq: 0,
            at: now,
            item: id.clone(),
            actor: actor.to_string(),
            change,
            ledger,
        };
        let mut next = self.state.borrow().projection.clone();
        next.apply(&event);
        let after = next
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::Knowledge(format!("no item {id}")))?;
        event.seq = self.store.append(&event, &after)?;
        let mut s = self.state.borrow_mut();
        s.projection.apply(&event);
        s.seen = s.seen.max(event.seq);
        Ok(after)
    }

    /// Adds an item. Its anchors are hashed now.
    pub fn add(&self, draft: Draft) -> AppResult<Item> {
        draft.validate().map_err(AppError::Knowledge)?;
        let id = self.state.borrow().projection.next_id(draft.kind);
        let anchors = self.hash(&draft.home, &draft.anchors)?;
        let item = draft.into_item(id.clone(), anchors, self.clock.now());
        self.commit(&id, OWNER, Change::Put(Box::new(item)))
    }

    /// Replaces an item's content with `draft`'s. Its id, standing, trust
    /// and counters stay; a stale item is confirmed by the edit (its
    /// anchors are hashed again). `draft.status`, when given, is a move.
    pub fn edit(&self, id: &ItemId, draft: Draft) -> AppResult<Item> {
        draft.validate().map_err(AppError::Knowledge)?;
        let old = self.must(id)?;
        if old.status == Status::Retired {
            return Err(AppError::Knowledge(format!("{id} is retired")));
        }
        if draft.kind != old.kind {
            return Err(AppError::Knowledge(format!(
                "{id} is a {}; its kind can't change",
                old.kind.as_str()
            )));
        }
        let anchors = self.hash(&draft.home, &draft.anchors)?;
        let mut item = draft
            .clone()
            .into_item(id.clone(), anchors, self.clock.now());
        item.status = match old.status {
            Status::Stale => old.prior.unwrap_or(Status::Advisory),
            s => s,
        };
        item.prior = None;
        item.trust = old.trust;
        item.provenance = old.provenance.clone();
        if item.kind == Kind::Rule && draft.enforcer.is_none() {
            item.enforcer = old.enforcer.clone();
        }
        let item = self.commit(id, OWNER, Change::Put(Box::new(item)))?;
        match draft.status {
            Some(to) if to != item.status => self.set_status(id, to, "edited"),
            _ => Ok(item),
        }
    }

    /// Moves an item to `to`, if it may go there.
    pub fn set_status(&self, id: &ItemId, to: Status, note: &str) -> AppResult<Item> {
        let item = self.must(id)?;
        check_move(item.kind, item.status, to)
            .map_err(|why| AppError::Knowledge(format!("{id}: {why}")))?;
        self.commit(
            id,
            OWNER,
            Change::Status {
                to,
                note: note.to_string(),
            },
        )
    }

    /// One step up: proposed → shadow (rules that act) or active, shadow
    /// → enforced, advisory → enforced (rules).
    pub fn promote(&self, id: &ItemId) -> AppResult<Item> {
        let item = self.must(id)?;
        let to = match (item.kind, item.status) {
            (Kind::Rule, Status::Proposed) if !item.is_text_rule() => Status::Shadow,
            (Kind::Rule, Status::Proposed | Status::Shadow | Status::Advisory) => Status::Enforced,
            (_, Status::Proposed) => Status::Advisory,
            (_, s) => {
                return Err(AppError::Knowledge(format!(
                    "{id} is {}: nothing to promote it to",
                    s.as_str()
                )));
            }
        };
        self.set_status(id, to, "promoted")
    }

    pub fn retire(&self, id: &ItemId) -> AppResult<Item> {
        self.set_status(id, Status::Retired, "retired")
    }

    /// Whether the item may reach agents.
    pub fn trust(&self, id: &ItemId, trust: Trust) -> AppResult<Item> {
        self.must(id)?;
        self.commit(id, OWNER, Change::Trusted(trust))
    }

    /// The owner says the item still holds: its anchors are hashed again
    /// and a stale item goes back to where it was.
    pub fn confirm(&self, id: &ItemId) -> AppResult<Item> {
        let item = self.must(id)?;
        if item.status == Status::Retired {
            return Err(AppError::Knowledge(format!("{id} is retired")));
        }
        let anchors = self.hash(&item.home, &item.anchors)?;
        self.commit(
            id,
            OWNER,
            Change::Confirmed {
                valid_for: valid_for(&anchors),
                anchors,
            },
        )
    }

    /// Hashes the anchors of `home`'s items again; those that changed turn
    /// stale. Returns them. Free: nothing but file reads.
    pub fn recheck(&self, home: &Home) -> AppResult<Vec<ItemId>> {
        let (Some(reader), Some(root)) = (&self.reader, self.root_of(home)?) else {
            return Ok(Vec::new());
        };
        let candidates: Vec<Item> = self
            .of_home(home)
            .into_iter()
            .filter(|i| !i.anchors.is_empty())
            .filter(|i| !matches!(i.status, Status::Stale | Status::Retired))
            .collect();
        let mut stale = Vec::new();
        for item in candidates {
            let read = |rel: &str| reader.read(&root, rel);
            let moved: Vec<String> = changed(&item.anchors, &read)
                .into_iter()
                .map(Anchor::spec)
                .collect();
            if moved.is_empty() {
                continue;
            }
            let note = format!("changed: {}", moved.join(", "));
            self.commit_quiet(
                &item.id,
                SWARMUX,
                Change::Status {
                    to: Status::Stale,
                    note,
                },
                None,
            )?;
            stale.push(item.id);
        }
        if !stale.is_empty() {
            self.publish();
        }
        Ok(stale)
    }

    /// [`recheck`](Self::recheck) for every home with anchored items.
    pub fn recheck_all(&self) -> AppResult<Vec<ItemId>> {
        let mut homes: Vec<Home> = Vec::new();
        for i in self.list() {
            if !i.anchors.is_empty() && !homes.iter().any(|h| h.same(&i.home)) {
                homes.push(i.home);
            }
        }
        let mut out = Vec::new();
        for h in homes {
            out.extend(self.recheck(&h)?);
        }
        Ok(out)
    }

    /// Folds every stored event again and replaces the stored projection
    /// with the result, reporting the items it had differently.
    pub fn rebuild(&self) -> AppResult<Rebuilt> {
        let events = self.store.events()?;
        let fresh = Projection::fold(&events);
        let stored: BTreeMap<ItemId, Item> = self
            .store
            .items()?
            .into_iter()
            .map(|i| (i.id.clone(), i))
            .collect();
        let mut differed: Vec<ItemId> = fresh
            .items()
            .filter(|i| stored.get(&i.id) != Some(*i))
            .map(|i| i.id.clone())
            .collect();
        differed.extend(stored.keys().filter(|id| fresh.get(id).is_none()).cloned());
        differed.sort();
        let items: Vec<Item> = fresh.items().cloned().collect();
        self.store.replace_items(&items)?;
        let report = Rebuilt {
            events: events.len(),
            items: items.len(),
            differed,
        };
        let mut s = self.state.borrow_mut();
        s.seen = events.last().map_or(0, |e| e.seq);
        s.projection = fresh;
        drop(s);
        self.publish();
        Ok(report)
    }

    /// The core facts of `home`, by id.
    pub fn core(&self, home: &Home) -> Vec<Item> {
        self.of_home(home)
            .into_iter()
            .filter(|i| i.core && i.status != Status::Retired)
            .collect()
    }

    /// Makes `facts` the core of `home`: lines already there are kept, new
    /// ones added as the owner's facts, the rest retired.
    pub fn save_map(&self, home: &Home, facts: &[String]) -> AppResult<MapSaved> {
        map::check_size(facts).map_err(AppError::Knowledge)?;
        let core = self.core(home);
        let mut saved = MapSaved::default();
        for old in &core {
            if facts.iter().any(|f| f.trim() == old.text) {
                saved.kept += 1;
            } else {
                self.set_status(&old.id, Status::Retired, "dropped from the map")?;
                saved.retired += 1;
            }
        }
        for fact in facts {
            if core.iter().any(|i| i.text == fact.trim()) {
                continue;
            }
            let mut draft = Draft::new(home.clone(), Kind::Fact, fact);
            draft.core = true;
            draft.status = Some(Status::Advisory);
            self.add(draft)?;
            saved.added += 1;
        }
        Ok(saved)
    }

    /// The items of `home` for a file, by id.
    pub fn export(&self, home: &Home) -> Vec<Item> {
        self.of_home(home)
    }

    /// Takes items from a file: new ones are added (keeping their id when
    /// it is free), changed ones are put, the same ones left alone.
    pub fn import(&self, items: Vec<Item>) -> AppResult<Imported> {
        let mut done = Imported::default();
        for mut item in items {
            let draft = Draft {
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
            };
            draft.validate().map_err(AppError::Knowledge)?;
            match self.get(&item.id) {
                Some(old) if old.home.same(&item.home) => {
                    let same = old.text == item.text
                        && old.why == item.why
                        && old.scope == item.scope
                        && old.anchors == item.anchors
                        && old.status == item.status
                        && old.core == item.core
                        && old.enforcer == item.enforcer
                        && old.trust == item.trust;
                    if same {
                        done.unchanged += 1;
                        continue;
                    }
                    item.provenance.events.clear();
                    self.commit(&item.id.clone(), OWNER, Change::Put(Box::new(item)))?;
                    done.changed += 1;
                }
                Some(_) => {
                    item.id = self.state.borrow().projection.next_id(item.kind);
                    item.provenance.events.clear();
                    self.commit(&item.id.clone(), OWNER, Change::Put(Box::new(item)))?;
                    done.added += 1;
                }
                None => {
                    item.provenance.events.clear();
                    self.commit(&item.id.clone(), OWNER, Change::Put(Box::new(item)))?;
                    done.added += 1;
                }
            }
        }
        Ok(done)
    }

    /// Folds in what others stored since (the command line), and records
    /// what the transport delivered and caught.
    pub fn pump(&self) -> Vec<AppError> {
        let mut errors = Vec::new();
        match self.store.last_seq() {
            Ok(last) if last > self.state.borrow().seen => match self.store.events() {
                Ok(events) => {
                    let mut s = self.state.borrow_mut();
                    s.projection = Projection::fold(&events);
                    s.seen = last;
                    drop(s);
                    self.publish();
                }
                Err(e) => errors.push(e),
            },
            Ok(_) => {}
            Err(e) => errors.push(e),
        }
        let usage = self.hub.drain();
        if usage.is_empty() {
            return errors;
        }
        for u in usage {
            errors.extend(self.record_usage(&u).err());
        }
        self.publish();
        errors
    }

    fn record_usage(&self, u: &Usage) -> AppResult<()> {
        let now = self.clock.now();
        let (job, payload, changes): (Option<JobId>, Payload, Vec<(ItemId, Change)>) = match u {
            Usage::Injected {
                job,
                trigger,
                items,
                tokens,
            } => (
                Some(*job),
                Payload::new()
                    .str("event", "injected")
                    .str("trigger", trigger)
                    .strs(
                        "items",
                        &items.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    )
                    .int("tokens", i64::try_from(*tokens).unwrap_or(i64::MAX)),
                items
                    .iter()
                    .map(|i| {
                        (
                            i.clone(),
                            Change::Used {
                                hits: 1,
                                shadow: false,
                            },
                        )
                    })
                    .collect(),
            ),
            Usage::Guard {
                job,
                rule,
                action,
                shadow,
                tool,
                subject,
            } => (
                *job,
                Payload::new()
                    .str("event", if *shadow { "shadow" } else { "rule" })
                    .str("item", rule.as_str())
                    .str(
                        "action",
                        &if *shadow {
                            format!("would_{}", action.as_str())
                        } else {
                            action.as_str().to_string()
                        },
                    )
                    .str("tool", tool)
                    .text("subject", subject),
                vec![(
                    rule.clone(),
                    Change::Used {
                        hits: 1,
                        shadow: *shadow,
                    },
                )],
            ),
            Usage::Checked {
                job,
                rule,
                passed,
                shadow,
            } => {
                let mut changes = vec![(
                    rule.clone(),
                    Change::Used {
                        hits: 1,
                        shadow: *shadow,
                    },
                )];
                if !shadow {
                    changes.push((rule.clone(), Change::Checked { passed: *passed }));
                }
                (
                    Some(*job),
                    Payload::new()
                        .str("event", if *shadow { "shadow_check" } else { "check" })
                        .str("item", rule.as_str())
                        .bool("passed", *passed),
                    changes,
                )
            }
        };
        let seq = match &self.ledger {
            Some(l) => Some(l.record_knowledge(job, payload, now)?),
            None => None,
        };
        for (id, change) in changes {
            if self.get(&id).is_some() {
                self.commit_quiet(&id, SWARMUX, change, seq.or(Some(0)))?;
            }
        }
        Ok(())
    }

    /// One P3 inbox item per home with stale items, for the owner to
    /// confirm or edit them in one sitting.
    pub fn stale_items(&self) -> Vec<AttentionItem> {
        let s = self.state.borrow();
        let mut by_home: Vec<(Home, Vec<&Item>)> = Vec::new();
        for i in s.projection.items().filter(|i| i.status == Status::Stale) {
            match by_home.iter_mut().find(|(h, _)| h.same(&i.home)) {
                Some((_, list)) => list.push(i),
                None => by_home.push((i.home.clone(), vec![i])),
            }
        }
        by_home
            .into_iter()
            .map(|(home, items)| {
                let ids: Vec<String> = items.iter().map(|i| i.id.to_string()).collect();
                let since = items.iter().map(|i| i.updated_at).min().unwrap_or(0);
                AttentionItem {
                    id: AttentionId::new(format!("knowledge:stale:{}", home.key())),
                    kind: AttentionKind::Knowledge,
                    priority: Priority::P3,
                    title: format!(
                        "{} knowledge item{} stale: {}",
                        items.len(),
                        if items.len() == 1 { " is" } else { "s are" },
                        ids.join(", ")
                    ),
                    detail: "their code changed; agents don't see them until you confirm or edit them (Knowledge screen, 8)".into(),
                    session: None,
                    job: None,
                    request: None,
                    since,
                    deadline: None,
                    likelihood: Likelihood::Certain,
                    risk: None,
                    signature: None,
                    batch: items.len(),
                    suggest: None,
                }
            })
            .collect()
    }

    /// What a job of `start` is, for the hub.
    fn info_of(&self, start: &JobStart<'_>) -> AppResult<JobInfo> {
        let home = match start.session.workspace {
            Some(id) => Home::Workspace(id),
            None => Home::repo(&start.session.cwd.to_string_lossy()),
        };
        let folders = match (&home, &self.workspaces) {
            (Home::Workspace(id), Some(store)) => store
                .get(*id)?
                .map(|w| {
                    w.repos
                        .iter()
                        .map(|r| (r.name.clone(), r.path.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let class = if start.review { "review" } else { "work" };
        Ok(JobInfo {
            scope: JobScope {
                home,
                repos: start.repos.clone(),
                classes: vec![class.to_string()],
            },
            root: start.cwd.to_string_lossy().replace('\\', "/"),
            folders,
        })
    }
}

/// The ledger payload of an item's change.
fn describe(id: &ItemId, home: &str, actor: &str, change: &Change) -> Payload {
    let p = Payload::new()
        .str("event", change.name())
        .str("item", id.as_str())
        .str("home", home)
        .str("actor", actor);
    match change {
        Change::Put(i) => p
            .str("kind", i.kind.as_str())
            .str("status", i.status.as_str())
            .str("source", i.provenance.source.as_str())
            .text("text", &i.text)
            .text("why", &i.why)
            .str("valid_for", &i.valid_for),
        Change::Status { to, note } => p.str("to", to.as_str()).text("note", note),
        Change::Confirmed { valid_for, .. } => p.str("valid_for", valid_for),
        Change::Used { hits, shadow } => p
            .int("hits", i64::try_from(*hits).unwrap_or(i64::MAX))
            .bool("shadow", *shadow),
        Change::Checked { passed } => p.bool("passed", *passed),
        Change::Trusted(t) => p.str("trust", t.as_str()),
    }
}

impl JobStarts for KnowledgeService {
    fn started(&self, start: &JobStart<'_>) -> AppResult<()> {
        let info = self.info_of(start)?;
        let home = info.scope.home.clone();
        self.hub.register(start.job, info);
        self.recheck(&home).map(drop)
    }
}

impl MissionWatch for KnowledgeService {
    fn launched(&self, job: JobId, _mission: &Mission) {
        self.hub.add_class(job, "mission");
    }

    fn delivered(&self, mission: &Mission) -> AppResult<()> {
        let home = match mission.workspace {
            Some(id) => Home::Workspace(id),
            None => Home::repo(&mission.root),
        };
        self.recheck(&home).map(drop)
    }
}

#[cfg(test)]
mod tests;
