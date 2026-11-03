//! Reading a job's story off the ledger: its first and last entries and
//! its gate verdicts.

use swarmux_domain::JobId;
use swarmux_domain::evidence::{GateRef, VerdictFacts, parse_verdict};
use swarmux_domain::ledger::{Hash, LedgerKind, hex, sha256};

use crate::AppResult;
use crate::ledger::Ledger;

/// Entries read per page.
const PAGE: usize = 1000;

/// A gate verdict entry of the job.
pub struct Verdict {
    pub seq: u64,
    pub digest: String,
    pub facts: VerdictFacts,
}

/// What the ledger holds about one job.
pub struct Trail {
    pub first_seq: u64,
    pub last_seq: u64,
    pub last_hash: Hash,
    verdicts: Vec<Verdict>,
}

impl Trail {
    /// The newest verdict that passed.
    pub fn passing(&self) -> Option<&Verdict> {
        self.verdicts
            .iter()
            .rev()
            .find(|v| v.facts.outcome == "pass")
    }

    pub fn gates(&self) -> Vec<GateRef> {
        self.verdicts
            .iter()
            .map(|v| GateRef {
                seq: v.seq,
                outcome: v.facts.outcome.clone(),
                event: v.facts.event.clone(),
                blocks: v.facts.blocks,
                digest: v.digest.clone(),
            })
            .collect()
    }
}

/// Walks the whole ledger once for the entries of `job`.
pub fn of_job(ledger: &dyn Ledger, job: JobId) -> AppResult<Trail> {
    let mut trail = Trail {
        first_seq: 0,
        last_seq: 0,
        last_hash: [0; 32],
        verdicts: Vec::new(),
    };
    let mut from = 1;
    loop {
        let page = ledger.range(from, PAGE)?;
        let Some(last) = page.last() else { break };
        from = last.seq + 1;
        let full = page.len() == PAGE;
        for e in page.iter().filter(|e| e.draft.job == Some(job)) {
            if trail.first_seq == 0 {
                trail.first_seq = e.seq;
            }
            trail.last_seq = e.seq;
            trail.last_hash = e.hash;
            if e.draft.kind == LedgerKind::GateVerdict
                && let Some(facts) = parse_verdict(&e.draft.payload_json)
            {
                trail.verdicts.push(Verdict {
                    seq: e.seq,
                    digest: hex(&sha256(e.draft.payload_json.as_bytes())),
                    facts,
                });
            }
        }
        if !full {
            break;
        }
    }
    Ok(trail)
}
