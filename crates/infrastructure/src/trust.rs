//! Repository trust, read back from the ledger, and a repository's
//! identity and what it supplies, read from its files.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use swarmux_application::trust::TrustReader;
use swarmux_domain::ledger::{LedgerEntry, LedgerKind, hex, sha256};
use swarmux_domain::trust::{TrustEntry, TrustEvent, TrustRecord};
use swarmux_domain::verifier::identity_key;

use crate::SqliteLedger;

/// The grants of the ledger in swarmux's database. Reads only the entries
/// appended since the last look, so asking on every hook call costs one
/// query.
pub struct LedgerTrustReader {
    db: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    ledger: Option<SqliteLedger>,
    scanned: u64,
    events: Vec<TrustEvent>,
}

impl LedgerTrustReader {
    pub fn new(db: &Path) -> LedgerTrustReader {
        LedgerTrustReader {
            db: db.to_path_buf(),
            state: Mutex::new(State::default()),
        }
    }
}

impl TrustReader for LedgerTrustReader {
    fn events(&self) -> Vec<TrustEvent> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.ledger.is_none() {
            // No database yet means no grants, not a failure.
            if !self.db.is_file() {
                return Vec::new();
            }
            state.ledger = SqliteLedger::open(&self.db).ok();
        }
        let State {
            ledger,
            scanned,
            events,
        } = &mut *state;
        let Some(ledger) = ledger else {
            return events.clone();
        };
        let Ok(head) = ledger.head_seq() else {
            return events.clone();
        };
        if head > *scanned
            && let Ok(entries) = ledger.of_kind_after(LedgerKind::Policy, *scanned)
        {
            events.extend(entries.iter().filter_map(parse_event));
            *scanned = head;
        }
        events.clone()
    }
}

/// A `trust_granted` or `trust_revoked` entry; anything else is `None`.
pub fn parse_event(entry: &LedgerEntry) -> Option<TrustEvent> {
    let json: serde_json::Value = serde_json::from_str(&entry.draft.payload_json).ok()?;
    let repo_id = json["repo_id"].as_str()?.to_string();
    match json["event"].as_str()? {
        "trust_granted" => {
            let mut entries: Vec<TrustEntry> = json["file_hashes"]
                .as_array()?
                .iter()
                .filter_map(|f| {
                    Some(TrustEntry {
                        path: f["path"].as_str()?.to_string(),
                        sha256: f["sha256"].as_str()?.to_string(),
                    })
                })
                .collect();
            entries.sort();
            Some(TrustEvent::Granted(TrustRecord {
                repo_id,
                root: json["root"].as_str().unwrap_or_default().to_string(),
                entries,
                at: entry.draft.ts,
            }))
        }
        "trust_revoked" => Some(TrustEvent::Revoked {
            repo_id,
            at: entry.draft.ts,
        }),
        _ => None,
    }
}

/// The identity key of the repository `root` is, or one made from its path
/// when it is not a git repository (or has no commit yet).
pub fn repo_id_of(root: &Path) -> String {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    match crate::clean_checkout::identity_of(Path::new("git"), root) {
        Some(id) => identity_key(&name, &id),
        None => {
            let path = root.to_string_lossy().replace('\\', "/");
            let hash = hex(&sha256(path.as_bytes()));
            identity_key(&name, &hash[..12])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::ledger::Ledger;
    use swarmux_domain::ledger::LedgerDraft;

    fn append(ledger: &SqliteLedger, payload: &str) {
        ledger
            .append(LedgerDraft {
                ts: 5,
                kind: LedgerKind::Policy,
                actor: "operator".into(),
                job: None,
                session: None,
                payload_json: payload.into(),
            })
            .unwrap();
    }

    #[test]
    fn grants_and_revocations_are_read_back_in_order_and_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("swarmux.db");
        let reader = LedgerTrustReader::new(&db);
        assert!(reader.events().is_empty(), "no database, no grants");
        let ledger = SqliteLedger::open(&db).unwrap();
        append(&ledger, r#"{"event":"guard_rule_added","id":"x"}"#);
        assert!(
            reader.events().is_empty(),
            "other policy entries say nothing"
        );
        append(
            &ledger,
            r#"{"event":"trust_granted","repo_id":"a.id-1","root":"/a","file_hashes":[{"path":".swarmux/gates.toml","sha256":"ab"}]}"#,
        );
        let events = reader.events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TrustEvent::Granted(r) => {
                assert_eq!(r.repo_id, "a.id-1");
                assert_eq!(r.entries[0].path, ".swarmux/gates.toml");
                assert_eq!(r.entries[0].sha256, "ab");
            }
            other => panic!("{other:?}"),
        }
        append(
            &ledger,
            r#"{"event":"trust_revoked","repo_id":"a.id-1","root":"/a"}"#,
        );
        let events = reader.events();
        assert_eq!(events.len(), 2, "only the new entry was read");
        assert!(matches!(events[1], TrustEvent::Revoked { .. }));
    }

    #[test]
    fn a_folder_that_is_not_a_repository_is_known_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = repo_id_of(dir.path());
        assert_eq!(a, repo_id_of(dir.path()));
        let other = tempfile::tempdir().unwrap();
        assert_ne!(a, repo_id_of(other.path()));
    }
}
