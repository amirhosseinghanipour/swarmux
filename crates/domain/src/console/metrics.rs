//! The verification figures: how often the gate passes at the first claim,
//! how long a claim takes to be verified, how much gets verified an hour,
//! how many checks ran and how many evidence bundles were sealed.
//!
//! Every figure is derived from ledger entries and nothing else; a figure
//! with nothing to derive it from is `None`, never zero. The window is the
//! last `window` milliseconds before `now`.
//!
//! # What the ledger records, and so what the figures mean
//!
//! - **A claim** is the agent saying it is done: Claude Code's `Stop`.
//!   The ledger holds it as the hook entry `agent_event` (actor `hook`,
//!   event `Stop`) and as the gate's verdict on that stop (`gate_verdict`,
//!   event `Stop`, with how many stops it had blocked before). The claim's
//!   moment is the earlier of the two entries of the job's first judged
//!   claim. A subagent's stop is not the job's claim.
//! - **Verified** is the gate's `pass` on a claim of the lead.
//! - Both entries are written when swarmux processes them, not at the
//!   instant they happened in the agent's process, so a claim the gate
//!   passed at once measures about nothing; the span grows with every block
//!   and the agent's rework in between. That is what claim-to-verified is.
//! - **First run** means the job's first judged claim. A claim the window
//!   begins in the middle of (the gate had blocked earlier stops, so its
//!   verdict carries `blocks` above zero) is not known to be the first and
//!   is left out of both the first-run rate and the claim-to-verified
//!   spans, instead of being counted as if it were.
//! - **Checks run** counts the check results the gate wrote into its
//!   verdicts, manual runs included (`swarmux gate`).
//! - A ledger entry that does not say what a figure needs (an older entry
//!   with no `blocks`) is skipped for that figure.

use std::collections::BTreeMap;

use crate::json::Json;
use crate::ledger::{LedgerEntry, LedgerKind};
use crate::{JobId, Millis};

/// Shortest span a per-hour figure is worked out over: a ledger that
/// began a minute ago has no rate.
const MIN_SPAN_MS: Millis = 60_000;
const HOUR_MS: Millis = 3_600_000;

/// How long claims took to be verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spread {
    pub median_ms: u64,
    /// The 90th percentile, nearest rank.
    pub p90_ms: u64,
    /// How many verified claims the spread is over.
    pub samples: u32,
}

/// First-run gate passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstRun {
    pub passed: u32,
    /// Jobs whose first judged claim the window saw.
    pub judged: u32,
}

impl FirstRun {
    /// `0.0..=1.0`.
    pub fn rate(&self) -> f64 {
        f64::from(self.passed) / f64::from(self.judged.max(1))
    }
}

/// The figures of one window.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    /// The window's end.
    pub at: Millis,
    /// The window asked for, and how much of it the entries cover (a
    /// younger ledger, or a read cut short, covers less).
    pub window_ms: Millis,
    pub covered_ms: Millis,
    /// Ledger entries the figures were worked out from.
    pub entries: u32,
    /// The oldest entries of the window were read; false when the read was
    /// cut short, so the figures describe only the newest part of it.
    pub complete: bool,
    /// `None` when no job's first claim was judged in the window.
    pub first_run: Option<FirstRun>,
    /// `None` when no claim was verified in the window.
    pub claim_to_verified: Option<Spread>,
    /// Jobs the gate passed in the window.
    pub verified: u32,
    /// `None` when the window covers too little time to give a rate.
    pub verified_per_hour: Option<f64>,
    pub checks_run: u32,
    pub bundles_sealed: u32,
}

impl Metrics {
    /// The figures of an empty ledger.
    pub fn none(at: Millis, window_ms: Millis) -> Metrics {
        compute(&[], at, window_ms, true)
    }
}

/// What the gate wrote about one judged stop.
struct Verdict {
    ts: Millis,
    pass: bool,
    /// Stops blocked before this one; `None` in an entry that lacks it.
    blocks: Option<u64>,
}

#[derive(Default)]
struct JobTrail {
    /// The lead's judged stops, in ledger order.
    verdicts: Vec<Verdict>,
    /// The first `Stop` hook entry.
    first_stop: Option<Millis>,
}

fn payload(e: &LedgerEntry) -> Option<Json> {
    Json::parse(&e.draft.payload_json).ok()
}

fn is_lead_stop(p: &Json) -> bool {
    p.get("event")
        .and_then(Json::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("stop"))
}

/// The figures over `entries` (in ledger order) for the window of
/// `window_ms` ending at `now`. Entries before the window are ignored.
/// `reached_start` says the read went back to the window's start, or to the
/// ledger's first entry, whichever is later; when it did not, the
/// figures say so (`complete`) and are rates over the part that was read.
pub fn compute(
    entries: &[LedgerEntry],
    now: Millis,
    window_ms: Millis,
    reached_start: bool,
) -> Metrics {
    let since = now.saturating_sub(window_ms);
    let inside: Vec<&LedgerEntry> = entries.iter().filter(|e| e.draft.ts >= since).collect();
    let covered_ms = if !reached_start {
        // The read stopped before the window's start: only what it saw.
        inside.first().map_or(window_ms, |f| now - f.draft.ts)
    } else if let Some(genesis) = entries.first().filter(|f| f.seq == 1) {
        // The ledger may be younger than the window.
        (now - genesis.draft.ts).clamp(0, window_ms)
    } else {
        window_ms
    };

    let mut trails: BTreeMap<i64, JobTrail> = BTreeMap::new();
    let (mut checks_run, mut bundles_sealed) = (0u32, 0u32);
    for e in &inside {
        match e.draft.kind {
            LedgerKind::GateVerdict => {
                let Some(p) = payload(e) else { continue };
                let runs = p
                    .get("checks")
                    .and_then(Json::as_array)
                    .map_or(0, <[Json]>::len);
                checks_run = checks_run.saturating_add(u32::try_from(runs).unwrap_or(u32::MAX));
                // A manual run (no event) judged no claim.
                let (true, Some(JobId(job))) = (is_lead_stop(&p), e.draft.job) else {
                    continue;
                };
                trails.entry(job).or_default().verdicts.push(Verdict {
                    ts: e.draft.ts,
                    pass: p.get("outcome").and_then(Json::as_str) == Some("pass"),
                    blocks: p.get("blocks").and_then(Json::as_u64),
                });
            }
            LedgerKind::AgentEvent if e.draft.actor == "hook" => {
                let Some(p) = payload(e) else { continue };
                if let (true, Some(JobId(job))) = (is_lead_stop(&p), e.draft.job) {
                    let t = trails.entry(job).or_default();
                    t.first_stop.get_or_insert(e.draft.ts);
                }
            }
            LedgerKind::Evidence => bundles_sealed = bundles_sealed.saturating_add(1),
            _ => {}
        }
    }

    let (mut judged, mut passed_first) = (0u32, 0u32);
    let mut spans: Vec<u64> = Vec::new();
    let mut verified = 0u32;
    for t in trails.values() {
        let Some(first) = t.verdicts.first() else {
            continue;
        };
        let verified_at = t.verdicts.iter().find(|v| v.pass).map(|v| v.ts);
        verified += u32::from(verified_at.is_some());
        // Only a claim the window saw from its start counts as a first one.
        if first.blocks != Some(0) {
            continue;
        }
        judged += 1;
        passed_first += u32::from(first.pass);
        if let Some(at) = verified_at {
            let claim = t.first_stop.map_or(first.ts, |s| s.min(first.ts));
            spans.push(u64::try_from((at - claim).max(0)).unwrap_or(0));
        }
    }

    Metrics {
        at: now,
        window_ms,
        covered_ms,
        entries: u32::try_from(inside.len()).unwrap_or(u32::MAX),
        complete: reached_start,
        first_run: (judged > 0).then_some(FirstRun {
            passed: passed_first,
            judged,
        }),
        claim_to_verified: spread(&mut spans),
        verified,
        verified_per_hour: (covered_ms >= MIN_SPAN_MS && verified > 0)
            .then(|| f64::from(verified) / (covered_ms as f64 / HOUR_MS as f64)),
        checks_run,
        bundles_sealed,
    }
}

/// Median and 90th percentile (nearest rank) of `spans`.
fn spread(spans: &mut [u64]) -> Option<Spread> {
    if spans.is_empty() {
        return None;
    }
    spans.sort_unstable();
    let n = spans.len();
    let median_ms = if n % 2 == 1 {
        spans[n / 2]
    } else {
        (spans[n / 2 - 1] + spans[n / 2]) / 2
    };
    let rank = (n * 9).div_ceil(10).max(1);
    Some(Spread {
        median_ms,
        p90_ms: spans[rank - 1],
        samples: u32::try_from(n).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{LedgerDraft, Payload};

    const MIN: Millis = 60_000;

    /// A chain of entries from drafts, numbered from 1.
    fn chain(drafts: Vec<LedgerDraft>) -> Vec<LedgerEntry> {
        let mut out: Vec<LedgerEntry> = Vec::new();
        for d in drafts {
            let e = LedgerEntry::next(out.last(), d);
            out.push(e);
        }
        out
    }

    fn draft(
        ts: Millis,
        kind: LedgerKind,
        actor: &str,
        job: Option<i64>,
        p: Payload,
    ) -> LedgerDraft {
        LedgerDraft {
            ts,
            kind,
            actor: actor.into(),
            job: job.map(JobId),
            session: None,
            payload_json: p.build(),
        }
    }

    fn verdict(ts: Millis, job: i64, outcome: &str, blocks: i64, checks: usize) -> LedgerDraft {
        let runs = (0..checks)
            .map(|i| Payload::new().str("name", &format!("c{i}")))
            .collect();
        draft(
            ts,
            LedgerKind::GateVerdict,
            "gate",
            Some(job),
            Payload::new()
                .str("event", "Stop")
                .str("outcome", outcome)
                .int("blocks", blocks)
                .objects("checks", runs),
        )
    }

    fn stop(ts: Millis, job: i64) -> LedgerDraft {
        draft(
            ts,
            LedgerKind::AgentEvent,
            "hook",
            Some(job),
            Payload::new().str("event", "Stop"),
        )
    }

    fn genesis(ts: Millis) -> LedgerDraft {
        LedgerDraft::genesis(ts, "test")
    }

    fn run(drafts: Vec<LedgerDraft>, now: Millis, window: Millis) -> Metrics {
        compute(&chain(drafts), now, window, true)
    }

    #[test]
    fn an_empty_ledger_has_no_figures_and_no_zeros_made_up() {
        let m = Metrics::none(10 * HOUR_MS, 24 * HOUR_MS);
        assert_eq!(m.first_run, None);
        assert_eq!(m.claim_to_verified, None);
        assert_eq!(m.verified_per_hour, None);
        assert_eq!((m.verified, m.checks_run, m.bundles_sealed), (0, 0, 0));
        assert!(m.complete);
    }

    #[test]
    fn first_run_rate_counts_jobs_whose_first_claim_passed() {
        let m = run(
            vec![
                genesis(0),
                // Job 1 passes at once; job 2 is blocked, then passes; job 3
                // is blocked and gives up; job 4 passes at once.
                verdict(10 * MIN, 1, "pass", 0, 2),
                verdict(11 * MIN, 2, "block", 0, 2),
                verdict(20 * MIN, 2, "pass", 1, 2),
                verdict(12 * MIN, 3, "block", 0, 1),
                verdict(13 * MIN, 3, "escalate", 2, 1),
                verdict(14 * MIN, 4, "pass", 0, 3),
            ],
            HOUR_MS,
            HOUR_MS,
        );
        let f = m.first_run.unwrap();
        assert_eq!((f.passed, f.judged), (2, 4));
        assert!((f.rate() - 0.5).abs() < 1e-9);
        assert_eq!(m.verified, 3, "jobs 1, 2 and 4 were verified");
        assert_eq!(m.checks_run, 2 + 2 + 2 + 1 + 1 + 3);
    }

    #[test]
    fn a_claim_the_window_began_in_the_middle_of_is_not_called_a_first_run() {
        // The verdict says two stops were blocked before it: the first claim
        // is outside what was read. It is verified, but it is no first run
        // and gives no claim-to-verified span.
        let m = run(
            vec![genesis(0), verdict(30 * MIN, 7, "pass", 2, 1)],
            HOUR_MS,
            HOUR_MS,
        );
        assert_eq!(m.first_run, None);
        assert_eq!(m.claim_to_verified, None);
        assert_eq!(m.verified, 1);
        // An entry that does not say how many stops it blocked is the same.
        let old = draft(
            30 * MIN,
            LedgerKind::GateVerdict,
            "gate",
            Some(8),
            Payload::new().str("event", "Stop").str("outcome", "pass"),
        );
        let m = run(vec![genesis(0), old], HOUR_MS, HOUR_MS);
        assert_eq!(m.first_run, None);
        assert_eq!(m.verified, 1);
    }

    #[test]
    fn claim_to_verified_runs_from_the_first_claim_to_the_pass() {
        // Four jobs verified after 0, 1, 3 and 10 minutes of blocks and
        // rework: the first claim is the earlier of the Stop hook entry and
        // the gate's first verdict.
        let m = run(
            vec![
                genesis(0),
                stop(MIN, 1),
                verdict(MIN, 1, "pass", 0, 1),
                stop(2 * MIN, 2),
                verdict(2 * MIN, 2, "block", 0, 1),
                verdict(3 * MIN, 2, "pass", 1, 1),
                // No hook entry for job 3: the first verdict is the claim.
                verdict(4 * MIN, 3, "block", 0, 1),
                verdict(7 * MIN, 3, "pass", 1, 1),
                stop(8 * MIN, 4),
                verdict(9 * MIN, 4, "block", 0, 1),
                verdict(18 * MIN, 4, "pass", 1, 1),
            ],
            HOUR_MS,
            HOUR_MS,
        );
        let s = m.claim_to_verified.unwrap();
        assert_eq!(s.samples, 4);
        // Spans: 0, 1, 3 and 10 minutes. Median of four is the middle two.
        assert_eq!(s.median_ms, ((MIN + 3 * MIN) / 2) as u64);
        assert_eq!(
            s.p90_ms,
            (10 * MIN) as u64,
            "nearest rank of 4 samples is the 4th"
        );
    }

    #[test]
    fn a_job_never_verified_gives_no_span_and_a_subagent_is_not_a_claim() {
        let sub = draft(
            5 * MIN,
            LedgerKind::GateVerdict,
            "gate",
            Some(9),
            Payload::new()
                .str("event", "SubagentStop")
                .str("outcome", "pass")
                .int("blocks", 0),
        );
        let manual = draft(
            6 * MIN,
            LedgerKind::GateVerdict,
            "cli",
            Some(9),
            Payload::new()
                .str("outcome", "pass")
                .int("blocks", 0)
                .objects("checks", vec![Payload::new(), Payload::new()]),
        );
        let m = run(
            vec![genesis(0), verdict(MIN, 1, "escalate", 0, 1), sub, manual],
            HOUR_MS,
            HOUR_MS,
        );
        let f = m.first_run.unwrap();
        assert_eq!(
            (f.passed, f.judged),
            (0, 1),
            "only job 1's claim was judged"
        );
        assert_eq!(m.claim_to_verified, None);
        assert_eq!(m.verified, 0);
        assert_eq!(m.verified_per_hour, None);
        // The manual run's two checks count: they ran.
        assert_eq!(m.checks_run, 3);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let mut ten: Vec<u64> = (1..=10).collect();
        let s = spread(&mut ten).unwrap();
        assert_eq!(
            (s.median_ms, s.p90_ms),
            (5, 9),
            "(5 + 6) / 2 rounds down; the 9th of 10"
        );
        let mut one = vec![42];
        let s = spread(&mut one).unwrap();
        assert_eq!((s.median_ms, s.p90_ms, s.samples), (42, 42, 1));
        let mut odd = vec![9, 1, 5];
        let s = spread(&mut odd).unwrap();
        assert_eq!((s.median_ms, s.p90_ms), (5, 9));
        assert_eq!(spread(&mut []), None);
    }

    #[test]
    fn verified_per_hour_is_over_the_time_the_ledger_covers() {
        // Two verified in a window of 24 h on a ledger that began 4 h ago:
        // 0.5 an hour, not 2 / 24.
        let now = 100 * HOUR_MS;
        let began = now - 4 * HOUR_MS;
        let m = run(
            vec![
                genesis(began),
                verdict(began + HOUR_MS, 1, "pass", 0, 1),
                verdict(began + 2 * HOUR_MS, 2, "pass", 0, 1),
            ],
            now,
            24 * HOUR_MS,
        );
        assert_eq!(m.covered_ms, 4 * HOUR_MS);
        assert!((m.verified_per_hour.unwrap() - 0.5).abs() < 1e-9);
        // A ledger that began 30 seconds ago has no rate yet.
        let m = run(
            vec![genesis(now - 30_000), verdict(now - 1_000, 1, "pass", 0, 1)],
            now,
            24 * HOUR_MS,
        );
        assert_eq!(m.verified, 1);
        assert_eq!(m.verified_per_hour, None);
    }

    #[test]
    fn an_older_ledger_is_a_full_window_and_a_cut_read_says_so() {
        let now = 200 * HOUR_MS;
        let entries = chain(vec![
            genesis(0),
            verdict(now - 2 * HOUR_MS, 1, "pass", 0, 1),
            verdict(now - HOUR_MS, 2, "pass", 0, 1),
        ]);
        // The ledger is far older than the window; the entries are the
        // window's (the older ones were not read).
        let m = compute(&entries[1..], now, 24 * HOUR_MS, true);
        assert_eq!(m.covered_ms, 24 * HOUR_MS);
        assert!(m.complete);
        // A read that stopped early covers only what it saw.
        let m = compute(&entries[1..], now, 24 * HOUR_MS, false);
        assert!(!m.complete);
        assert_eq!(m.covered_ms, 2 * HOUR_MS);
        assert!((m.verified_per_hour.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn entries_before_the_window_do_not_count() {
        let now = 10 * HOUR_MS;
        let m = run(
            vec![
                genesis(0),
                verdict(HOUR_MS, 1, "pass", 0, 5),
                draft(
                    2 * HOUR_MS,
                    LedgerKind::Evidence,
                    "swarmux",
                    Some(1),
                    Payload::new(),
                ),
                verdict(9 * HOUR_MS + 30 * MIN, 2, "block", 0, 1),
                draft(
                    9 * HOUR_MS + 40 * MIN,
                    LedgerKind::Evidence,
                    "swarmux",
                    Some(2),
                    Payload::new(),
                ),
            ],
            now,
            2 * HOUR_MS,
        );
        assert_eq!((m.verified, m.checks_run, m.bundles_sealed), (0, 1, 1));
        let f = m.first_run.unwrap();
        assert_eq!((f.passed, f.judged), (0, 1));
    }
}
