use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use swarmux_domain::gate::{GateCheck, GateOrigin, GateSpec, Pin};
use swarmux_domain::ledger::LedgerKind;
use swarmux_domain::trust::{
    CommandSource, Supplied, TrustEntry, TrustEvent, TrustRecord, TrustState, WantedCommand,
};

use super::*;
use crate::testing::{FixedClock, InMemoryLedger};

/// A repository whose gate says what the test wants it to say.
struct Source {
    origin: GateOrigin,
    spec: GateSpec,
    supplied: Supplied,
}

impl GateSource for Source {
    fn load(&self, _dir: &Path) -> LoadedGate {
        LoadedGate {
            root: PathBuf::from("/w"),
            spec: self.spec.clone(),
            origin: self.origin,
            problem: None,
        }
    }

    fn pins(&self, _root: &Path, _globs: &[String]) -> Vec<Pin> {
        Vec::new()
    }

    fn supplied(&self, _gate: &LoadedGate) -> Supplied {
        self.supplied.clone()
    }
}

#[derive(Default)]
struct Reader(Mutex<Vec<TrustEvent>>);

impl TrustReader for Reader {
    fn events(&self) -> Vec<TrustEvent> {
        self.0.lock().unwrap().clone()
    }
}

fn shipped(contents: &str) -> Source {
    let mut spec = GateSpec::default();
    spec.checks.push(GateCheck::new("unit", "cargo", &["test"]));
    spec.protected = vec!["oracles/**".into()];
    spec.allow_substrate = vec!["Makefile".into()];
    Source {
        origin: GateOrigin::File,
        supplied: Supplied {
            repo_id: "shop.id-3fa9c1d2e4b7".into(),
            root: "/w".into(),
            entries: vec![TrustEntry::of_bytes(
                ".swarmux/gates.toml",
                contents.as_bytes(),
            )],
            commands: vec![WantedCommand {
                name: "unit".into(),
                line: "cargo test".into(),
                source: CommandSource::File,
            }],
            loosens: spec.loosening(),
        },
        spec,
    }
}

fn guard(reader: &Arc<Reader>) -> TrustGuard {
    TrustGuard::new(reader.clone(), FixedClock::new(5).reader())
}

fn grant(reader: &Reader, source: &Source) {
    reader
        .0
        .lock()
        .unwrap()
        .push(TrustEvent::Granted(TrustRecord {
            repo_id: source.supplied.repo_id.clone(),
            root: source.supplied.root.clone(),
            entries: source.supplied.entries.clone(),
            at: 1,
        }));
}

fn load(guard: &TrustGuard, source: &Source, unattended: bool) -> LoadedGate {
    let mut gate = source.load(Path::new("/w"));
    guard.apply(source, &mut gate, unattended);
    gate
}

#[test]
fn a_repository_nobody_trusted_runs_none_of_its_checks() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let source = shipped("[[check]]");
    for unattended in [false, true] {
        let gate = load(&guard, &source, unattended);
        assert!(gate.spec.checks.is_empty(), "unattended={unattended}");
        let why = gate.problem.expect("the gate says why it did not run");
        assert!(why.contains("not trusted yet"), "{why}");
        assert!(why.contains("swarmux trust grant /w"), "{why}");
    }
}

#[test]
fn what_restricts_applies_before_the_grant_and_what_loosens_does_not() {
    let reader = Arc::new(Reader::default());
    let gate = load(&guard(&reader), &shipped("[[check]]"), false);
    assert_eq!(gate.spec.protected, ["oracles/**"]);
    assert!(gate.spec.allow_substrate.is_empty());
    assert!(
        gate.spec.warnings.iter().any(|w| w.contains("not trusted")),
        "{:?}",
        gate.spec.warnings
    );
}

#[test]
fn the_grant_lets_the_checks_run_and_a_changed_file_asks_again() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let source = shipped("[[check]]");
    grant(&reader, &source);
    let gate = load(&guard, &source, true);
    assert_eq!(gate.spec.checks.len(), 1, "granted: it runs");
    assert!(gate.problem.is_none());
    assert_eq!(
        gate.spec.allow_substrate,
        ["Makefile"],
        "its keys count now"
    );
    assert!(guard.attention().is_empty());

    // The file changes on disk: the old grant does not cover it.
    let edited = shipped("[[check]] # now it does something else");
    let gate = load(&guard, &edited, true);
    assert!(gate.spec.checks.is_empty());
    let items = guard.attention();
    assert_eq!(items.len(), 1);
    assert!(items[0].title.contains("changed what it wants to run"));
    assert!(items[0].detail.contains("run cargo test"));
}

#[test]
fn the_inbox_item_shows_the_exact_commands_and_goes_when_granted() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let source = shipped("[[check]]");
    load(&guard, &source, false);
    let items = guard.attention();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.kind, swarmux_domain::AttentionKind::Trust);
    assert!(item.detail.contains("run cargo test"), "{}", item.detail);
    assert_eq!(item.id.as_str(), "trust:shop.id-3fa9c1d2e4b7");
    assert_eq!(guard.waiting().len(), 1);

    grant(&reader, &source);
    load(&guard, &source, false);
    assert!(guard.attention().is_empty());
    assert!(guard.waiting().is_empty());
}

#[test]
fn guessed_commands_run_in_front_of_a_person_but_not_unattended() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let mut spec = GateSpec::default();
    spec.checks
        .push(GateCheck::new("npm test", "npm", &["test"]));
    let source = Source {
        origin: GateOrigin::Detected,
        supplied: Supplied {
            repo_id: "web.id-aaaaaaaaaaaa".into(),
            root: "/w".into(),
            entries: vec![TrustEntry::detected("npm test", Some("jest"))],
            commands: vec![WantedCommand {
                name: "npm test".into(),
                line: "npm test".into(),
                source: CommandSource::Detected,
            }],
            loosens: Vec::new(),
        },
        spec,
    };
    let attended = load(&guard, &source, false);
    assert_eq!(
        attended.spec.checks.len(),
        1,
        "as before, with you watching"
    );
    assert!(attended.problem.is_none());
    let unattended = load(&guard, &source, true);
    assert!(
        unattended.spec.checks.is_empty(),
        "no guess runs unattended"
    );
    assert!(unattended.problem.is_some());
    assert_eq!(guard.waiting().len(), 1, "it asks either way");
}

#[test]
fn a_repository_that_supplies_nothing_is_left_alone() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let source = Source {
        origin: GateOrigin::Detected,
        spec: GateSpec::default(),
        supplied: Supplied::default(),
    };
    let gate = load(&guard, &source, true);
    assert!(gate.problem.is_none());
    assert!(guard.waiting().is_empty());
}

#[test]
fn granting_and_revoking_are_ledger_entries() {
    let source = shipped("[[check]]");
    let ledger = Rc::new(InMemoryLedger::default());
    let clock = Rc::new(FixedClock::new(10));
    let service = Rc::new(LedgerService::new(ledger.clone(), clock));
    let reader = Arc::new(Reader::default());
    let trust = TrustService::new(service, reader);
    let record = trust.grant(&source.supplied).unwrap();
    assert_eq!(record.entries, source.supplied.entries);
    trust.revoke("shop.id-3fa9c1d2e4b7", "/w").unwrap();
    let entries: Vec<_> = ledger
        .entries()
        .into_iter()
        .filter(|e| e.draft.kind == LedgerKind::Policy)
        .collect();
    assert_eq!(entries.len(), 2);
    let granted = &entries[0].draft.payload_json;
    assert!(granted.contains("\"event\":\"trust_granted\""), "{granted}");
    assert!(
        granted.contains("\"repo_id\":\"shop.id-3fa9c1d2e4b7\""),
        "{granted}"
    );
    assert!(granted.contains("\"file_hashes\""), "{granted}");
    assert!(granted.contains(".swarmux/gates.toml"), "{granted}");
    assert!(granted.contains("cargo test"), "{granted}");
    assert!(entries[1].draft.payload_json.contains("trust_revoked"));
    swarmux_domain::ledger::verify(&ledger.entries()).expect("the chain holds");
}

#[test]
fn a_revoked_repository_is_not_trusted_again_until_granted() {
    let reader = Arc::new(Reader::default());
    let guard = guard(&reader);
    let source = shipped("[[check]]");
    grant(&reader, &source);
    assert_eq!(guard.state_of(&source.supplied), TrustState::Trusted);
    reader.0.lock().unwrap().push(TrustEvent::Revoked {
        repo_id: source.supplied.repo_id.clone(),
        at: 2,
    });
    assert_eq!(guard.state_of(&source.supplied), TrustState::Revoked);
    let gate = load(&guard, &source, false);
    assert!(gate.spec.checks.is_empty());
}
