//! Fakes of the evidence ports, and a rig that delivers missions on a
//! ledger in memory (feature `test-support`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use swarmux_domain::evidence::{Bundle, DiffSummary, Label, PrOutcome};
use swarmux_domain::gate::{CheckRun, GateCheck, GateFindings, GateOrigin, GateOutcome};
use swarmux_domain::mission::{
    Acceptance, Budget, Constraints, Contract, Mission, MissionState, Signal,
};
use swarmux_domain::{JobId, JobResult, JobStatus, Millis, Session, SessionId};

use super::{
    Checkout, Checkouts, EvidenceService, EvidenceStore, HeadFacts, OutcomeProbe, ProbeReply,
    ProbeRequest, ProbeSink, Rerunner, Stored, Vcs,
};
use crate::gate::{Checker, GateRecord};
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository, NewJob, SessionRepository};
use crate::testing::{FixedClock, InMemoryLedger, InMemoryStore};
use crate::{AppError, AppResult};

/// Bundles in memory.
#[derive(Default)]
pub struct InMemoryEvidence {
    rows: RefCell<Vec<Stored>>,
}

impl EvidenceStore for InMemoryEvidence {
    fn save(&self, bundle: &Bundle, seal_seq: Option<u64>, _now: Millis) -> AppResult<()> {
        let mut rows = self.rows.borrow_mut();
        let stored = Stored {
            bundle: bundle.clone(),
            digest: bundle.digest_hex(),
            seal_seq,
            labels: Vec::new(),
        };
        match rows
            .iter_mut()
            .find(|s| s.bundle.contract.slug == bundle.contract.slug)
        {
            Some(s) => {
                let labels = std::mem::take(&mut s.labels);
                *s = Stored { labels, ..stored };
            }
            None => rows.push(stored),
        }
        Ok(())
    }

    fn get(&self, slug: &str) -> AppResult<Option<Stored>> {
        Ok(self
            .rows
            .borrow()
            .iter()
            .find(|s| s.bundle.contract.slug == slug)
            .cloned())
    }

    fn list(&self) -> AppResult<Vec<Stored>> {
        Ok(self.rows.borrow().clone())
    }

    fn set_labels(&self, slug: &str, labels: &[Label]) -> AppResult<()> {
        if let Some(s) = self
            .rows
            .borrow_mut()
            .iter_mut()
            .find(|s| s.bundle.contract.slug == slug)
        {
            s.labels = labels.to_vec();
        }
        Ok(())
    }
}

impl InMemoryEvidence {
    /// Edits the stored bundle behind the service's back.
    pub fn tamper(&self, slug: &str, f: impl FnOnce(&mut Bundle)) {
        if let Some(s) = self
            .rows
            .borrow_mut()
            .iter_mut()
            .find(|s| s.bundle.contract.slug == slug)
        {
            f(&mut s.bundle);
        }
    }
}

/// A repository whose head is whatever the test says.
pub struct FakeVcs {
    pub facts: RefCell<Option<HeadFacts>>,
    pub dirs: RefCell<Vec<PathBuf>>,
}

impl FakeVcs {
    pub fn at(head: &str) -> FakeVcs {
        FakeVcs {
            facts: RefCell::new(Some(HeadFacts {
                head: head.into(),
                dirty: false,
                branch: Some("swarmux/demo/1".into()),
                base: Some("base0000".into()),
                repo: PathBuf::from("/repo"),
            })),
            dirs: RefCell::default(),
        }
    }
}

impl Vcs for FakeVcs {
    fn head(&self, dir: &Path) -> AppResult<Option<HeadFacts>> {
        self.dirs.borrow_mut().push(dir.to_path_buf());
        Ok(self.facts.borrow().clone())
    }

    fn diff(&self, _dir: &Path, _base: Option<&str>, _head: &str) -> AppResult<DiffSummary> {
        Ok(DiffSummary {
            files: 2,
            insertions: 30,
            deletions: 4,
            hash: "d1ff".into(),
        })
    }
}

struct Held(PathBuf);

impl Checkout for Held {
    fn path(&self) -> &Path {
        &self.0
    }
}

/// Clean checkouts that are only a path; they remember what was asked.
#[derive(Default)]
pub struct FakeCheckouts {
    pub asked: Mutex<Vec<(PathBuf, String)>>,
    pub refuse: Mutex<Option<String>>,
}

impl Checkouts for FakeCheckouts {
    fn checkout(&self, repo: &Path, sha: &str) -> AppResult<Box<dyn Checkout>> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((repo.to_path_buf(), sha.to_string()));
        match self
            .refuse
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            Some(why) => Err(AppError::Workspace(why)),
            None => Ok(Box::new(Held(PathBuf::from("/clean")))),
        }
    }
}

/// A checker that answers by check name (anything else passes) and
/// remembers where it was run.
#[derive(Default)]
pub struct ScriptedChecker {
    exits: Mutex<HashMap<String, i32>>,
    pub roots: Mutex<Vec<PathBuf>>,
}

impl ScriptedChecker {
    pub fn exit(&self, name: &str, code: i32) {
        self.exits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_string(), code);
    }
}

impl Checker for ScriptedChecker {
    fn run(&self, check: &GateCheck, root: &Path) -> CheckRun {
        self.roots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(root.to_path_buf());
        let code = self
            .exits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&check.name)
            .copied()
            .unwrap_or(0);
        CheckRun {
            name: check.name.clone(),
            command: check.command_line(),
            exit: Some(code),
            timed_out: false,
            spawn_error: None,
            duration_ms: 5,
            output: if code == 0 {
                String::new()
            } else {
                "error: failed".into()
            },
            flaky: false,
        }
    }
}

/// Records the branches asked about; the test answers them.
#[derive(Default)]
pub struct FakeProbe {
    requests: RefCell<Vec<ProbeRequest>>,
    sinks: RefCell<Vec<Option<ProbeSink>>>,
}

impl FakeProbe {
    pub fn requests(&self) -> Vec<ProbeRequest> {
        self.requests.borrow().clone()
    }

    /// Answers the `i`-th question (once).
    pub fn reply(&self, i: usize, reply: ProbeReply) {
        if let Some(sink) = self.sinks.borrow_mut()[i].take() {
            sink(reply);
        }
    }
}

impl OutcomeProbe for FakeProbe {
    fn probe(&self, request: ProbeRequest, sink: ProbeSink) {
        self.requests.borrow_mut().push(request);
        self.sinks.borrow_mut().push(Some(sink));
    }
}

/// A pull request as the probe would report it.
pub fn pr(number: u64, state: swarmux_domain::evidence::PrState) -> PrOutcome {
    PrOutcome {
        number,
        state,
        url: format!("https://example.test/pull/{number}"),
        reverted_at: None,
    }
}

/// An evidence service on a ledger in memory, wired to fakes, over a job
/// store and clock that are the caller's.
pub struct EvidenceKit {
    pub ledger: Rc<InMemoryLedger>,
    pub ledger_service: Rc<LedgerService>,
    pub bundles: Rc<InMemoryEvidence>,
    pub vcs: Rc<FakeVcs>,
    pub checkouts: Arc<FakeCheckouts>,
    pub checker: Arc<ScriptedChecker>,
    pub probe: Rc<FakeProbe>,
    pub service: Rc<EvidenceService>,
}

impl EvidenceKit {
    pub fn new(store: Rc<InMemoryStore>, clock: Rc<FixedClock>) -> EvidenceKit {
        let ledger = Rc::new(InMemoryLedger::default());
        let ledger_service = Rc::new(LedgerService::new(ledger.clone(), clock.clone()));
        let bundles = Rc::new(InMemoryEvidence::default());
        let vcs = Rc::new(FakeVcs::at("cafe0000cafe0000"));
        let checkouts = Arc::new(FakeCheckouts::default());
        let checker = Arc::new(ScriptedChecker::default());
        let probe = Rc::new(FakeProbe::default());
        let service = Rc::new(
            EvidenceService::new(
                bundles.clone(),
                ledger_service.clone(),
                store,
                vcs.clone(),
                clock,
            )
            .with_rerunner(Rerunner::new(checker.clone(), checkouts.clone()))
            .with_probe(probe.clone()),
        );
        EvidenceKit {
            ledger,
            ledger_service,
            bundles,
            vcs,
            checkouts,
            checker,
            probe,
            service,
        }
    }
}

/// An evidence kit with missions that can be delivered at will.
pub struct EvidenceRig {
    pub store: Rc<InMemoryStore>,
    pub ledger: Rc<InMemoryLedger>,
    pub ledger_service: Rc<LedgerService>,
    pub clock: Rc<FixedClock>,
    pub bundles: Rc<InMemoryEvidence>,
    pub vcs: Rc<FakeVcs>,
    pub checkouts: Arc<FakeCheckouts>,
    pub checker: Arc<ScriptedChecker>,
    pub probe: Rc<FakeProbe>,
    pub service: Rc<EvidenceService>,
    pub session: Session,
}

impl EvidenceRig {
    pub fn new() -> EvidenceRig {
        let store = Rc::new(InMemoryStore::default());
        let clock = Rc::new(FixedClock::new(1_000_000));
        let kit = EvidenceKit::new(store.clone(), clock.clone());
        let session =
            Session::new(SessionId::random(), "Demo app", "/repo".into(), "sonnet", 0).unwrap();
        SessionRepository::insert(&*store, &session).unwrap();
        EvidenceRig {
            store,
            ledger: kit.ledger,
            ledger_service: kit.ledger_service,
            clock,
            bundles: kit.bundles,
            vcs: kit.vcs,
            checkouts: kit.checkouts,
            checker: kit.checker,
            probe: kit.probe,
            service: kit.service,
            session,
        }
    }

    /// A contract with one check.
    pub fn contract(title: &str) -> Contract {
        Contract {
            title: title.into(),
            goal: "make the parser handle empty input without panicking".into(),
            acceptance: Acceptance {
                checks: vec!["cargo test -p parser".into()],
                human_signoff: false,
            },
            constraints: Constraints {
                paths_rw: vec!["src/**".into()],
                protected: vec!["tests/**".into()],
            },
            budget: Budget {
                usd: Some(2.0),
                ..Budget::default()
            },
            clarifications: Vec::new(),
        }
    }

    /// Runs a mission through the ledger up to its gate's verdict and
    /// returns it in `state`. A passing verdict is recorded when the job
    /// ended well; `blocked` adds a blocked stop before it.
    pub fn mission(&self, slug: &str, state: MissionState, blocked: bool) -> Mission {
        let new = NewJob {
            session_id: self.session.id,
            prompt_id: None,
            prompt: format!("do {slug}"),
            model: "sonnet".into(),
            priority: 0,
            fork: false,
            wants_review: false,
            review_of: None,
        };
        let job = JobRepository::insert(&*self.store, &new, self.clock.now()).unwrap();
        self.ledger_service
            .record_job_queued(job, &new, self.clock.now())
            .unwrap();
        self.store
            .set_status(job, JobStatus::Done, self.clock.now())
            .unwrap();
        self.store
            .save_result(
                job,
                &JobResult {
                    tokens_in: 1000,
                    tokens_out: 4000,
                    cache_read: 0,
                    cache_creation: None,
                    cost_usd: 1.5,
                    duration_ms: 90_000,
                    outcome: "success".into(),
                    error: None,
                    text: "done".into(),
                },
            )
            .unwrap();
        if blocked {
            self.verdict(job, GateOutcome::Block, false);
        }
        self.verdict(job, GateOutcome::Pass, true);
        let mut m = Mission::new(slug, "/repo", Self::contract(slug), self.clock.now());
        m.session = Some(self.session.id);
        m.job = Some(job);
        m.spend.implement = 1.5;
        m.spend.plan = 0.0005;
        m.state = MissionState::Verifying;
        if state == MissionState::Delivered {
            m.apply(Signal::Gate(GateOutcome::Pass), self.clock.now())
                .unwrap();
        } else {
            m.state = state;
        }
        self.ledger_service
            .record_mission(&m, MissionState::Verifying, self.clock.now())
            .unwrap();
        m
    }

    fn verdict(&self, job: JobId, outcome: GateOutcome, ok: bool) {
        let run = CheckRun {
            name: "cargo test -p parser".into(),
            command: "cargo test -p parser".into(),
            exit: Some(i32::from(!ok)),
            timed_out: false,
            spawn_error: None,
            duration_ms: 4200,
            output: if ok {
                String::new()
            } else {
                "thread 'main' panicked at src/lib.rs:3".into()
            },
            flaky: false,
        };
        let record = GateRecord {
            job: Some(job),
            session: "claude-session".into(),
            event: Some(swarmux_domain::HookEvent::Stop),
            root: "/repo".into(),
            origin: GateOrigin::File,
            outcome,
            blocks: u32::from(outcome == GateOutcome::Pass),
            max_blocks: 3,
            findings: GateFindings {
                runs: vec![run],
                ..GateFindings::default()
            },
            digest: "digest".into(),
            problem: None,
        };
        self.ledger_service
            .record_gate(&record, Some(self.session.id), self.clock.now())
            .unwrap();
    }
}

impl Default for EvidenceRig {
    fn default() -> Self {
        EvidenceRig::new()
    }
}
