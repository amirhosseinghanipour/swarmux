//! Evidence bundles: sealing one when a mission is delivered, keeping it,
//! checking it again later (`verify`), labelling what became of the work
//! and turning runs into the cards the Ledger screen shows.
//!
//! The ports live here, in the feature: where bundles are kept
//! ([`EvidenceStore`]), who signs them ([`Signer`]), what git says
//! ([`Vcs`]), where a clean copy of a commit can be had ([`Checkouts`])
//! and what GitHub says about a pull request ([`OutcomeProbe`]).

mod cards;
mod labels;
mod trail;
mod verify;

pub use cards::build_cards;
pub use verify::Rerunner;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use swarmux_domain::evidence::{
    BUNDLE_SCHEMA, Bundle, CheckpointRef, ContractRef, Cost, DiffSummary, Label, LedgerRange,
    PrOutcome, ReviewRef, Signature, VerifyReport, checks_of,
};
use swarmux_domain::ledger::{Hash, hex};
use swarmux_domain::mission::{Mission, MissionState, compile};
use swarmux_domain::{JobId, Millis, ReviewState};

use crate::anchor::AnchorService;
use crate::checkpoint::CheckpointService;
use crate::ledger::LedgerService;
use crate::ports::{Clock, JobRepository};
use crate::{AppError, AppResult};

/// A bundle as kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub bundle: Bundle,
    /// The bundle's digest when it was sealed, hex.
    pub digest: String,
    /// The ledger entry that recorded the sealing.
    pub seal_seq: Option<u64>,
    pub labels: Vec<Label>,
}

/// Where bundles are kept.
pub trait EvidenceStore {
    /// Inserts the bundle or replaces the one of its mission. Labels
    /// already attached stay.
    fn save(&self, bundle: &Bundle, seal_seq: Option<u64>, now: Millis) -> AppResult<()>;
    fn get(&self, slug: &str) -> AppResult<Option<Stored>>;
    /// Every bundle, oldest first.
    fn list(&self) -> AppResult<Vec<Stored>>;
    fn set_labels(&self, slug: &str, labels: &[Label]) -> AppResult<()>;
}

/// Signs a bundle's digest. Real keys come later; the stub only hashes.
pub trait Signer {
    fn sign(&self, digest: &Hash) -> Signature;
}

/// The hash-only stand-in: the "signature" is the digest itself, so edits
/// are detected but nobody is vouched for.
#[derive(Clone, Copy, Debug, Default)]
pub struct HashSigner;

impl Signer for HashSigner {
    fn sign(&self, digest: &Hash) -> Signature {
        Signature {
            alg: "sha256".into(),
            value: hex(digest),
        }
    }
}

/// Where a repository's work stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadFacts {
    pub head: String,
    /// Files changed since the head commit.
    pub dirty: bool,
    pub branch: Option<String>,
    /// Where the branch left the repository's main line.
    pub base: Option<String>,
    /// The repository's top folder.
    pub repo: PathBuf,
}

/// What git says, read only.
pub trait Vcs {
    /// The head of the repository `dir` lies in; `None` outside one.
    fn head(&self, dir: &Path) -> AppResult<Option<HeadFacts>>;
    /// The change from `base` (or an empty tree) to `head`.
    fn diff(&self, dir: &Path, base: Option<&str>, head: &str) -> AppResult<DiffSummary>;
}

/// A clean copy of one commit that lives while it is held.
pub trait Checkout: Send {
    fn path(&self) -> &Path;
}

/// Makes clean copies of commits.
pub trait Checkouts: Send + Sync {
    /// `sha` of `repo`, checked out into a folder of its own, with
    /// nothing from the repository's working files in it.
    fn checkout(&self, repo: &Path, sha: &str) -> AppResult<Box<dyn Checkout>>;
}

/// The branch to look up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRequest {
    pub slug: String,
    pub repo: PathBuf,
    pub branch: String,
}

/// What a probe found: the pull request of the branch, if it has one.
pub type ProbeReply = Result<Option<PrOutcome>, String>;
/// Receives a probe's reply, from any thread.
pub type ProbeSink = Box<dyn FnOnce(ProbeReply) + Send + 'static>;

/// Asks GitHub about a branch's pull request. Never blocks.
pub trait OutcomeProbe {
    fn probe(&self, request: ProbeRequest, sink: ProbeSink);
}

type Verified = (String, Vec<swarmux_domain::evidence::VerifyItem>);

/// Seals, keeps, verifies and labels evidence bundles.
pub struct EvidenceService {
    store: Rc<dyn EvidenceStore>,
    ledger: Rc<LedgerService>,
    jobs: Rc<dyn JobRepository>,
    vcs: Rc<dyn Vcs>,
    signer: Rc<dyn Signer>,
    clock: Rc<dyn Clock>,
    checkpoints: Option<Rc<CheckpointService>>,
    anchor: Option<Rc<AnchorService>>,
    rerunner: Option<Rerunner>,
    probe: Option<Rc<dyn OutcomeProbe>>,
    probe_tx: Sender<(String, ProbeReply)>,
    probe_rx: RefCell<Receiver<(String, ProbeReply)>>,
    probing: RefCell<HashSet<String>>,
    verify_tx: Sender<Verified>,
    verify_rx: RefCell<Receiver<Verified>>,
    /// Static findings of a verification waiting for its re-run.
    pending: RefCell<HashMap<String, Vec<swarmux_domain::evidence::VerifyItem>>>,
    reports: RefCell<HashMap<String, VerifyReport>>,
    /// Why the last question to GitHub failed.
    problem: RefCell<Option<String>>,
}

impl EvidenceService {
    pub fn new(
        store: Rc<dyn EvidenceStore>,
        ledger: Rc<LedgerService>,
        jobs: Rc<dyn JobRepository>,
        vcs: Rc<dyn Vcs>,
        clock: Rc<dyn Clock>,
    ) -> EvidenceService {
        let (probe_tx, probe_rx) = channel();
        let (verify_tx, verify_rx) = channel();
        EvidenceService {
            store,
            ledger,
            jobs,
            vcs,
            signer: Rc::new(HashSigner),
            clock,
            checkpoints: None,
            anchor: None,
            rerunner: None,
            probe: None,
            probe_tx,
            probe_rx: RefCell::new(probe_rx),
            probing: RefCell::default(),
            verify_tx,
            verify_rx: RefCell::new(verify_rx),
            pending: RefCell::default(),
            reports: RefCell::default(),
            problem: RefCell::default(),
        }
    }

    pub fn with_signer(mut self, signer: Rc<dyn Signer>) -> EvidenceService {
        self.signer = signer;
        self
    }

    /// Lists a mission's checkpoints in its bundle and finds its worktree.
    pub fn with_checkpoints(mut self, checkpoints: Rc<CheckpointService>) -> EvidenceService {
        self.checkpoints = Some(checkpoints);
        self
    }

    /// Takes a signed checkpoint of the ledger as each bundle is sealed
    /// and keeps it in the bundle.
    pub fn with_anchor(mut self, anchor: Rc<AnchorService>) -> EvidenceService {
        self.anchor = Some(anchor);
        self
    }

    /// Lets `verify` re-run checks in clean copies of commits.
    pub fn with_rerunner(mut self, rerunner: Rerunner) -> EvidenceService {
        self.rerunner = Some(rerunner);
        self
    }

    /// Lets labels be asked of GitHub.
    pub fn with_probe(mut self, probe: Rc<dyn OutcomeProbe>) -> EvidenceService {
        self.probe = Some(probe);
        self
    }

    pub fn get(&self, slug: &str) -> AppResult<Option<Stored>> {
        self.store.get(slug)
    }

    /// Every bundle, oldest first.
    pub fn list(&self) -> AppResult<Vec<Stored>> {
        self.store.list()
    }

    /// Seals the bundle of a delivered mission: what the contract was,
    /// what the gate said and the checks showed, the diff, the cost and
    /// where the ledger stood. Nothing is delivered without one, so a
    /// mission that can't be sealed isn't delivered.
    pub fn seal(&self, mission: &Mission) -> AppResult<Bundle> {
        let fail = |why: String| AppError::Mission(format!("{}: {why}", mission.slug));
        if mission.state != MissionState::Delivered {
            return Err(fail("only a delivered mission has a bundle".into()));
        }
        let job = mission
            .job
            .ok_or_else(|| fail("it has no job to vouch for".into()))?;
        let trail = trail::of_job(self.ledger.ledger(), job)?;
        let verdict = trail
            .passing()
            .ok_or_else(|| fail("no passing gate verdict is on the ledger".into()))?;
        let defined = compile(&mission.contract)
            .map(|c| c.gate.checks)
            .unwrap_or_default();
        let checks = checks_of(&verdict.facts, &defined);
        if checks.is_empty() && verdict.facts.hidden.is_none_or(|h| h.ran == 0) {
            return Err(fail("the passing verdict ran no checks".into()));
        }
        let (dir, repo_hint) = self.workdir(job, mission);
        let facts = self
            .vcs
            .head(&dir)?
            .ok_or_else(|| fail(format!("{} is not in a git repository", dir.display())))?;
        let diff = self.vcs.diff(&dir, facts.base.as_deref(), &facts.head)?;
        let checkpoints = match &self.checkpoints {
            Some(c) => c
                .list(job)?
                .into_iter()
                .map(|c| CheckpointRef {
                    commit: c.commit,
                    reason: c.reason.as_str().to_string(),
                    seq: c.ledger_seq,
                })
                .collect(),
            None => Vec::new(),
        };
        let (cost, wall_ms, review) = self.cost_of(job, mission)?;
        // A checkpoint that cannot be signed (no key, locked keychain)
        // does not stop a delivery: the bundle simply has none.
        let anchor = self.anchor.as_ref().and_then(|a| a.checkpoint().ok());
        let k = &mission.contract;
        let mut bundle = Bundle {
            schema: BUNDLE_SCHEMA,
            contract: ContractRef {
                slug: mission.slug.clone(),
                title: k.title.clone(),
                goal: k.goal.clone(),
                root: mission.root.clone(),
                checks: k.acceptance.checks.clone(),
                protected: k.constraints.protected.clone(),
                paths_rw: k.constraints.paths_rw.clone(),
                budget_usd: k.budget.usd,
                wall_minutes: k.budget.wall_minutes,
                human_signoff: k.acceptance.human_signoff,
            },
            job: job.0,
            attempt: 1,
            session: mission.session.map(|s| s.to_string()),
            repo: repo_hint.unwrap_or(facts.repo).display().to_string(),
            branch: facts.branch,
            base_sha: facts.base,
            head_sha: facts.head,
            dirty: facts.dirty,
            diff,
            checks,
            hidden: verdict.facts.hidden,
            gates: trail.gates(),
            review,
            checkpoints,
            cost,
            wall_ms,
            ledger: LedgerRange {
                from_seq: trail.first_seq,
                to_seq: trail.last_seq,
                head_hash: hex(&trail.last_hash),
            },
            sealed_at: self.clock.now(),
            anchor,
            signature: None,
        };
        bundle.signature = Some(self.signer.sign(&bundle.digest()));
        let seq = self.ledger.record_evidence(
            &mission.slug,
            Some(job),
            &bundle.digest_hex(),
            &bundle.head_sha,
            bundle.ledger.to_seq,
            self.clock.now(),
        )?;
        self.store.save(&bundle, Some(seq), self.clock.now())?;
        Ok(bundle)
    }

    /// Seals every delivered mission that has no bundle yet (those that
    /// were delivered before bundles existed). The ones that can't be
    /// sealed come back with the reason.
    pub fn backfill(&self, missions: &[Mission]) -> Vec<(String, AppError)> {
        let have: HashSet<String> = match self.store.list() {
            Ok(all) => all.into_iter().map(|s| s.bundle.contract.slug).collect(),
            Err(e) => return vec![(String::new(), e)],
        };
        missions
            .iter()
            .filter(|m| m.state == MissionState::Delivered && !have.contains(&m.slug))
            .filter_map(|m| self.seal(m).err().map(|e| (m.slug.clone(), e)))
            .collect()
    }

    /// Where to look at the work: the job's worktree while it has one,
    /// else the mission's folder; and the repository the worktree belongs to.
    fn workdir(&self, job: JobId, mission: &Mission) -> (PathBuf, Option<PathBuf>) {
        let record = self
            .checkpoints
            .as_ref()
            .and_then(|c| c.worktree(job).ok().flatten());
        match record {
            Some(r) if r.removed_at.is_none() => (r.path, Some(r.repo)),
            Some(r) => (PathBuf::from(&mission.root), Some(r.repo)),
            None => (PathBuf::from(&mission.root), None),
        }
    }

    /// Cost by phase and model, wall time and the reviewer's verdict.
    fn cost_of(&self, job: JobId, mission: &Mission) -> AppResult<(Cost, u64, Option<ReviewRef>)> {
        let result = self.jobs.result(job)?;
        let model_of = |id: JobId| -> AppResult<String> {
            Ok(self.jobs.get(id)?.map(|j| j.model).unwrap_or_default())
        };
        let implement = result
            .as_ref()
            .map_or(mission.spend.implement, |r| r.cost_usd);
        let mut by_model: Vec<(String, f64)> = Vec::new();
        let mut add = |model: String, usd: f64| {
            if usd <= 0.0 {
                return;
            }
            match by_model.iter_mut().find(|(m, _)| *m == model) {
                Some(e) => e.1 += usd,
                None => by_model.push((model, usd)),
            }
        };
        add(model_of(job)?, implement);
        let (review_usd, review) = match self.jobs.review(job)? {
            Some(ReviewState::Done {
                review,
                job: rj,
                cost_usd,
            }) => {
                add(model_of(rj)?, cost_usd);
                (
                    cost_usd,
                    Some(ReviewRef {
                        verdict: review.verdict.as_str().to_string(),
                        concerns: review.concerns.len() as u32,
                        job: rj.0,
                    }),
                )
            }
            Some(ReviewState::Failed {
                cost_usd, job: rj, ..
            }) => {
                if let Some(rj) = rj {
                    add(model_of(rj)?, cost_usd);
                }
                (cost_usd, None)
            }
            _ => (0.0, None),
        };
        let lint_model = mission
            .lint
            .as_ref()
            .map(|l| l.source.clone())
            .filter(|s| !s.is_empty() && s != "local")
            .unwrap_or_else(|| "haiku".into());
        add(lint_model, mission.spend.plan);
        by_model.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let cost = Cost {
            total_usd: implement + review_usd + mission.spend.plan + mission.spend.verify,
            basis: "reported".into(),
            plan: mission.spend.plan,
            implement,
            verify: mission.spend.verify,
            review: review_usd,
            by_model,
        };
        Ok((cost, result.map_or(0, |r| r.duration_ms), review))
    }
}

#[cfg(test)]
mod tests;
