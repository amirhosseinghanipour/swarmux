//! Repository trust: the use cases.
//!
//! [`TrustGuard`] sits in the gate's loading path (it is `Send + Sync`, the
//! hook transport's threads use it): a repository whose files the owner has
//! not granted trust for runs none of the commands it ships. It remembers
//! what such repositories want, for the inbox and `swarmux doctor`.
//! [`TrustService`] is what the owner uses to see, grant and revoke trust;
//! grants and revocations are ledger entries, and the ledger is where trust
//! is read from.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::trust::{
    Supplied, TrustEvent, TrustRecord, TrustState, prompt_text, trust_state,
};
use swarmux_domain::{AttentionItem, Millis};

use crate::AppResult;
use crate::gate::{GateSource, LoadedGate};
use crate::ledger::LedgerService;

/// Where the grants are read from: the ledger's `trust_granted` and
/// `trust_revoked` entries, oldest first.
pub trait TrustReader: Send + Sync {
    fn events(&self) -> Vec<TrustEvent>;
}

/// The newest event of `repo_id`.
pub fn latest(events: &[TrustEvent], repo_id: &str) -> Option<TrustEvent> {
    events
        .iter()
        .rev()
        .find(|e| e.repo_id() == repo_id)
        .cloned()
}

/// A repository that asked and has not been answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub supplied: Supplied,
    pub state: TrustState,
    pub since: Millis,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the gate does with a repository that is not trusted.
pub struct TrustGuard {
    reader: Arc<dyn TrustReader>,
    pending: Mutex<BTreeMap<String, Pending>>,
    clock: Arc<dyn Fn() -> Millis + Send + Sync>,
}

impl TrustGuard {
    pub fn new(
        reader: Arc<dyn TrustReader>,
        clock: Arc<dyn Fn() -> Millis + Send + Sync>,
    ) -> TrustGuard {
        TrustGuard {
            reader,
            pending: Mutex::default(),
            clock,
        }
    }

    /// The state of `supplied` now.
    pub fn state_of(&self, supplied: &Supplied) -> TrustState {
        let events = self.reader.events();
        trust_state(supplied, latest(&events, &supplied.repo_id).as_ref())
    }

    /// Applies the trust rule to a gate just loaded. When the repository is
    /// not trusted its checks are not run (and, for an unattended job,
    /// neither are guessed ones), every key that loosens the gate reads as
    /// its default, and the gate's `problem` says why, so no verdict passes
    /// on checks that did not run. What restricts still applies. The
    /// repository is remembered for the inbox.
    pub fn apply(
        &self,
        source: &dyn GateSource,
        gate: &mut LoadedGate,
        unattended: bool,
    ) -> TrustState {
        let supplied = source.supplied(gate);
        let state = self.state_of(&supplied);
        if state.allows() {
            lock(&self.pending).remove(&supplied.repo_id);
            return state;
        }
        let since = (self.clock)();
        lock(&self.pending)
            .entry(supplied.repo_id.clone())
            .and_modify(|p| {
                if p.state != state || p.supplied != supplied {
                    p.supplied = supplied.clone();
                    p.state = state.clone();
                }
            })
            .or_insert_with(|| Pending {
                supplied: supplied.clone(),
                state: state.clone(),
                since,
            });
        let had_checks = !gate.spec.checks.is_empty();
        let mut strict = gate.spec.untrusted();
        // Guessed commands run in front of a person who is watching, as
        // before; commands the repository ships, and anything unattended,
        // wait for the grant.
        let keep_guessed = !supplied.has_file_commands() && !unattended;
        if keep_guessed {
            strict.checks = gate.spec.checks.clone();
        }
        let withheld = had_checks && strict.checks.is_empty();
        if !gate.spec.loosening().is_empty() {
            strict.warnings.push(
                "this repository is not trusted: the gates.toml keys that loosen the gate are ignored"
                    .into(),
            );
        }
        gate.spec = strict;
        if withheld && gate.problem.is_none() {
            gate.problem = Some(format!(
                "{}: its checks did not run; `swarmux trust grant {}` shows exactly what would",
                state.as_str(),
                supplied.root
            ));
        }
        state
    }

    /// Repositories waiting for an answer, as inbox items.
    pub fn attention(&self) -> Vec<AttentionItem> {
        lock(&self.pending)
            .iter()
            .filter(|(_, p)| !p.state.allows())
            .map(|(id, p)| {
                let (title, detail) = prompt_text(&p.supplied, &p.state);
                AttentionItem::trust(id, &title, &detail, p.since)
            })
            .collect()
    }

    /// What is waiting, for `swarmux doctor`.
    pub fn waiting(&self) -> Vec<Pending> {
        lock(&self.pending)
            .values()
            .filter(|p| !p.state.allows())
            .cloned()
            .collect()
    }

    /// Forgets a repository (it was granted just now).
    pub fn settled(&self, repo_id: &str) {
        lock(&self.pending).remove(repo_id);
    }
}

/// Seeing, granting and revoking trust. Lives on the UI or CLI thread: the
/// ledger service is not shared between threads.
pub struct TrustService {
    ledger: Rc<LedgerService>,
    reader: Arc<dyn TrustReader>,
}

impl TrustService {
    pub fn new(ledger: Rc<LedgerService>, reader: Arc<dyn TrustReader>) -> TrustService {
        TrustService { ledger, reader }
    }

    /// The newest grant or revocation of each repository.
    pub fn records(&self) -> Vec<TrustEvent> {
        let events = self.reader.events();
        let mut newest: BTreeMap<String, TrustEvent> = BTreeMap::new();
        for e in events {
            newest.insert(e.repo_id().to_string(), e);
        }
        newest.into_values().collect()
    }

    /// The state of `supplied` now.
    pub fn state_of(&self, supplied: &Supplied) -> TrustState {
        let events = self.reader.events();
        trust_state(supplied, latest(&events, &supplied.repo_id).as_ref())
    }

    /// Grants trust for exactly the files and commands in `supplied`.
    pub fn grant(&self, supplied: &Supplied) -> AppResult<TrustRecord> {
        let now = self.ledger.now();
        let record = TrustRecord {
            repo_id: supplied.repo_id.clone(),
            root: supplied.root.clone(),
            entries: supplied.entries.clone(),
            at: now,
        };
        self.ledger.record_trust_granted(&record, supplied, now)?;
        Ok(record)
    }

    /// Takes the grant of `repo_id` back.
    pub fn revoke(&self, repo_id: &str, root: &str) -> AppResult<()> {
        let now = self.ledger.now();
        self.ledger.record_trust_revoked(repo_id, root, now)
    }
}

#[cfg(test)]
mod tests;
