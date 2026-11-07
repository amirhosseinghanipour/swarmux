//! `verify`: does a bundle still hold? Its digest and place in the ledger
//! are checked here, and its checks are run again on a clean copy of the
//! commit it names.

use std::path::Path;
use std::sync::Arc;

use swarmux_domain::evidence::{
    Bundle, Rerun, VerifyItem, VerifyReport, compare_checks, sealed_digest,
};
use swarmux_domain::gate::GateCheck;
use swarmux_domain::ledger::hex;
use swarmux_domain::verifier::repo_key;

use super::{Checkouts, EvidenceService, Stored};
use crate::gate::{Checker, run_checks};
use crate::ledger::verify_all;
use crate::verifier::HoldoutStore;
use crate::{AppError, AppResult};

/// The first characters of a hash, to show.
pub fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

/// Runs a bundle's checks again in a clean copy of its head commit.
#[derive(Clone)]
pub struct Rerunner {
    checker: Arc<dyn Checker>,
    checkouts: Arc<dyn Checkouts>,
    holdout: Option<Arc<dyn HoldoutStore>>,
}

impl Rerunner {
    pub fn new(checker: Arc<dyn Checker>, checkouts: Arc<dyn Checkouts>) -> Rerunner {
        Rerunner {
            checker,
            checkouts,
            holdout: None,
        }
    }

    /// Runs the repository's hidden checks again too, for bundles whose
    /// verdict ran some.
    pub fn with_holdout(mut self, store: Arc<dyn HoldoutStore>) -> Rerunner {
        self.holdout = Some(store);
        self
    }

    /// The repository's hidden checks as they are now, run in `dir` after
    /// the visible ones: they all pass, or how many did. Never which.
    fn hidden_item(&self, bundle: &Bundle, dir: &Path) -> Option<VerifyItem> {
        let sealed = bundle.hidden.filter(|h| h.ran > 0)?;
        let subject = "hidden checks";
        let expected = format!("all pass (sealed {})", sealed.text());
        let Some(store) = &self.holdout else {
            return Some(VerifyItem::bad(subject, &expected, "no hidden check store"));
        };
        let Some(repo) = repo_key(&bundle.repo) else {
            return Some(VerifyItem::bad(subject, &expected, "no repository name"));
        };
        let checks = match store.list(&repo) {
            Ok(c) if !c.is_empty() => c,
            Ok(_) => return Some(VerifyItem::bad(subject, &expected, "none kept any more")),
            Err(e) => return Some(VerifyItem::bad(subject, &expected, &e.to_string())),
        };
        let placed: Vec<bool> = checks
            .iter()
            .map(|c| store.place(&repo, c, dir).is_ok())
            .collect();
        let passed = checks
            .iter()
            .zip(placed)
            .filter(|(c, ok)| {
                *ok && run_checks(&*self.checker, std::slice::from_ref(&c.check), dir)
                    .first()
                    .is_some_and(|r| r.passed())
            })
            .count();
        let found = format!("{passed}/{} pass", checks.len());
        Some(if passed == checks.len() {
            VerifyItem::ok(subject, &found)
        } else {
            VerifyItem::bad(subject, &expected, &found)
        })
    }

    /// Blocks until every check has run. The clean copy is gone afterwards.
    pub fn run(&self, bundle: &Bundle) -> Vec<VerifyItem> {
        let checkout = match self
            .checkouts
            .checkout(Path::new(&bundle.repo), &bundle.head_sha)
        {
            Ok(c) => c,
            Err(e) => {
                return vec![VerifyItem::bad(
                    "clean checkout",
                    short(&bundle.head_sha),
                    &e.to_string(),
                )];
            }
        };
        let mut items = vec![VerifyItem::ok("clean checkout", short(&bundle.head_sha))];
        let reruns: Vec<Rerun> = bundle
            .checks
            .iter()
            .map(|c| {
                let check = GateCheck {
                    name: c.name.clone(),
                    program: c.program.clone(),
                    args: c.args.clone(),
                    timeout_secs: c.timeout_secs,
                    dir: None,
                };
                let run = run_checks(&*self.checker, &[check], checkout.path())
                    .into_iter()
                    .next();
                match run {
                    Some(r) => Rerun {
                        name: c.name.clone(),
                        exit: r.exit,
                        passed: r.passed(),
                        problem: r
                            .spawn_error
                            .as_ref()
                            .map(|e| format!("could not start: {e}"))
                            .or_else(|| {
                                r.timed_out
                                    .then(|| format!("timed out after {} s", c.timeout_secs))
                            }),
                    },
                    None => Rerun {
                        name: c.name.clone(),
                        exit: None,
                        passed: false,
                        problem: Some("no result".into()),
                    },
                }
            })
            .collect();
        items.extend(compare_checks(bundle, &reruns));
        items.extend(self.hidden_item(bundle, checkout.path()));
        drop(checkout);
        items
    }
}

fn no_rerun() -> VerifyItem {
    VerifyItem::bad(
        "re-run",
        "the checks run again",
        "no way to make a clean checkout",
    )
}

impl EvidenceService {
    /// What can be checked without running anything: the signature, the
    /// stored copy, the sealing entry and the ledger.
    pub fn static_items(&self, bundle: &Bundle, stored: Option<&Stored>) -> Vec<VerifyItem> {
        let digest = bundle.digest_hex();
        let mut out = vec![match bundle.signature_ok() {
            Ok(true) => VerifyItem::ok("signature", "sha256 of the content"),
            Ok(false) => VerifyItem::bad("signature", "matches the content", "content was edited"),
            Err(why) => VerifyItem::bad("signature", "a valid signature", &why),
        }];
        if let Some(s) = stored {
            out.push(VerifyItem::new(
                "stored digest",
                short(&s.digest),
                short(&digest),
            ));
            if let Some(seq) = s.seal_seq {
                let found = self
                    .ledger
                    .ledger()
                    .range(seq, 1)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .filter(|e| e.seq == seq)
                    .and_then(|e| sealed_digest(&e.draft.payload_json))
                    .unwrap_or_else(|| "not on the ledger".into());
                out.push(VerifyItem::new(
                    &format!("ledger seal #{seq}"),
                    short(&digest),
                    short(&found),
                ));
            }
        }
        out.extend(self.ledger_items(bundle));
        out.extend(self.anchor_item(bundle));
        if bundle.dirty {
            out.push(VerifyItem::bad(
                "work in the commit",
                "everything committed",
                "files were uncommitted when it was sealed",
            ));
        }
        out
    }

    /// The bundle's checkpoint: signed, and the ledger still has the head
    /// it signed. A bundle sealed without one says so, as no problem.
    fn anchor_item(&self, bundle: &Bundle) -> Option<VerifyItem> {
        let subject = "ledger anchor";
        let Some(cp) = &bundle.anchor else {
            return Some(VerifyItem::ok(
                subject,
                "none (sealed without a checkpoint)",
            ));
        };
        let anchors = self.anchor.as_ref()?;
        Some(match anchors.check(cp) {
            Ok(j) => match j.problem() {
                None => VerifyItem::ok(
                    subject,
                    &format!(
                        "checkpoint #{} signed{}",
                        cp.seq,
                        if j.own_key { "" } else { " (by another key)" }
                    ),
                ),
                Some(why) => VerifyItem::bad(subject, "signed and still on the ledger", &why),
            },
            Err(e) => VerifyItem::bad(subject, "readable", &e.to_string()),
        })
    }

    fn ledger_items(&self, bundle: &Bundle) -> Vec<VerifyItem> {
        let ledger = self.ledger.ledger();
        let mut out = Vec::new();
        match verify_all(ledger) {
            Ok(r) if r.is_ok() => out.push(VerifyItem::ok(
                "ledger chain",
                &format!("{} entries intact", r.entries),
            )),
            Ok(r) => out.push(VerifyItem::bad(
                "ledger chain",
                "unbroken",
                &r.broken.map_or_else(String::new, |b| b.to_string()),
            )),
            Err(e) => out.push(VerifyItem::bad("ledger chain", "readable", &e.to_string())),
        }
        let to = bundle.ledger.to_seq;
        let found = ledger
            .range(to, 1)
            .ok()
            .and_then(|v| v.into_iter().next())
            .filter(|e| e.seq == to)
            .map_or_else(|| "missing".to_string(), |e| hex(&e.hash));
        out.push(VerifyItem::new(
            &format!("ledger head #{to}"),
            short(&bundle.ledger.head_hash),
            short(&found),
        ));
        out
    }

    /// Verifies the bundle and waits for the checks: for the command line.
    pub fn verify_now(&self, bundle: &Bundle) -> AppResult<VerifyReport> {
        let stored = self.store.get(&bundle.contract.slug)?;
        let mut items = self.static_items(bundle, stored.as_ref());
        match &self.rerunner {
            Some(r) => items.extend(r.run(bundle)),
            None => items.push(no_rerun()),
        }
        Ok(VerifyReport { items })
    }

    /// Starts verifying the mission's bundle; the checks run on a thread of
    /// their own and [`EvidenceService::pump_verify`] collects the report.
    pub fn start_verify(&self, slug: &str) -> AppResult<()> {
        let Some(stored) = self.store.get(slug)? else {
            return Err(AppError::Mission(format!("{slug} has no bundle")));
        };
        if self.verifying(slug) {
            return Ok(());
        }
        let mut items = self.static_items(&stored.bundle, Some(&stored));
        self.reports.borrow_mut().remove(slug);
        let Some(rerunner) = self.rerunner.clone() else {
            items.push(no_rerun());
            self.reports
                .borrow_mut()
                .insert(slug.to_string(), VerifyReport { items });
            return Ok(());
        };
        self.pending.borrow_mut().insert(slug.to_string(), items);
        let tx = self.verify_tx.clone();
        let key = slug.to_string();
        let bundle = stored.bundle;
        let started = std::thread::Builder::new()
            .name("swarmux-verify-bundle".into())
            .spawn(move || {
                let items = rerunner.run(&bundle);
                // Nobody listening (the app is closing): nothing to report.
                let _ = tx.send((key, items));
            });
        if let Err(e) = started {
            self.pending.borrow_mut().remove(slug);
            return Err(AppError::Workspace(format!(
                "cannot start the verification: {e}"
            )));
        }
        Ok(())
    }

    /// A verification of this mission's bundle is running.
    pub fn verifying(&self, slug: &str) -> bool {
        self.pending.borrow().contains_key(slug)
    }

    /// The last finished verification of the mission's bundle.
    pub fn report(&self, slug: &str) -> Option<VerifyReport> {
        self.reports.borrow().get(slug).cloned()
    }

    /// Collects finished verifications; the slugs that finished.
    pub fn pump_verify(&self) -> Vec<String> {
        let mut done = Vec::new();
        let rx = self.verify_rx.borrow();
        while let Ok((slug, rerun)) = rx.try_recv() {
            let mut items = self.pending.borrow_mut().remove(&slug).unwrap_or_default();
            items.extend(rerun);
            self.reports
                .borrow_mut()
                .insert(slug.clone(), VerifyReport { items });
            done.push(slug);
        }
        done
    }
}
