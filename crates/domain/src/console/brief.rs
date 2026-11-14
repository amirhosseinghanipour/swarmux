//! What the ledger says happened between two moments, for the Return
//! brief: what it cost, which calls the Guard held or refused, which calls
//! fell back because swarmux was not reachable, which always-allow rules
//! were added, which hidden checks did not run, which jobs were checked in
//! the agent's own worktree, the verification figures of the window and a
//! short timeline.
//!
//! Every figure is derived from ledger entries and nothing else; a figure
//! with nothing to derive it from is absent, never zero. The ledger is
//! read as it was written: nothing here knows about the terminal.
//!
//! # What the entries mean here
//!
//! - **Spend** is the `cost_usd` of the jobs' `job_finished` entries inside
//!   the window. A revised entry (new final numbers for a job that had
//!   finished) adds only what it changed, when the earlier figure is among
//!   the entries given; a job that finished before the window and was not
//!   revised in it adds nothing.
//! - **Guard.** A `guard_decision` whose outcome is `ask` is a call held for
//!   the user; a later `allow` or `deny` of the same job, tool and subject
//!   is how the hold ended and is not counted as a routine allow. A `deny`
//!   with no hold before it was refused outright. An `allow` with no hold is
//!   a routine allow: counted, not listed. A `stop` is the loop breaker.
//! - **Fallbacks** are `policy` entries with event `guard_fallback`: a hook
//!   call decided without swarmux (unreachable app, unattended job).
//! - **Rules added** are `policy` entries with event `added`: an always-allow
//!   rule. (The plan calls this `guard_rule_added`; the ledger's event is
//!   `added`.)
//! - **Hidden checks not run** and **ran in the worktree** are what a job's
//!   last gate verdict in the window said (`hidden_not_run`, `ran_in`); a
//!   later verdict of the same job that no longer says so clears it.

use std::collections::BTreeMap;

use crate::json::Json;
use crate::ledger::{LedgerEntry, LedgerKind};
use crate::{JobId, Millis};

use super::metrics::{Metrics, compute as compute_metrics};

/// The timeline keeps the newest moments up to this many.
pub const TIMELINE_KEPT: usize = 60;

/// What the jobs that ended in the window cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spend {
    pub usd: f64,
    /// Jobs that ended in the window, with a cost.
    pub jobs: u32,
}

/// How a call the Guard did not simply allow ended, as far as the ledger
/// says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallState {
    /// Held for the user; no entry says how it ended.
    Held,
    /// Held, then allowed.
    HeldAllowed,
    /// Held, then denied (by the user or when the time ran out).
    HeldDenied,
    /// Refused without asking.
    Denied,
    /// The loop breaker stopped the job.
    Stopped,
}

/// A call the Guard held, refused or stopped.
#[derive(Clone, Debug, PartialEq)]
pub struct GuardCall {
    pub at: Millis,
    pub job: Option<i64>,
    pub tool: String,
    pub subject: String,
    /// The rule that decided it; none when the risk or no rule did.
    pub rule: Option<String>,
    pub state: CallState,
}

/// The Guard's decisions in the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuardDigest {
    /// Allowed without asking: counted, not listed.
    pub allowed: u32,
    /// Held, refused and stopped, oldest first.
    pub calls: Vec<GuardCall>,
}

/// A hook call that fell back because swarmux was not reachable.
#[derive(Clone, Debug, PartialEq)]
pub struct Fallback {
    /// When the hook decided, as it recorded (the entry may be written
    /// later, when swarmux started again).
    pub at: Millis,
    pub job: Option<i64>,
    pub tool: String,
    pub reason: String,
    /// What it did: `allow` or `deny`.
    pub mode: String,
}

/// An always-allow rule that was added.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleAdded {
    pub at: Millis,
    pub rule: String,
    pub scope: String,
    pub root: Option<String>,
    pub source: String,
    pub expires_at: Option<Millis>,
}

/// Hidden checks that a job's gate could not run.
#[derive(Clone, Debug, PartialEq)]
pub struct HiddenNotRun {
    pub job: Option<i64>,
    pub at: Millis,
    /// How many hidden checks were planned.
    pub planned: u32,
    pub reason: String,
}

/// A job whose checks ran in its own worktree, beside whatever it left
/// there, not in a clean copy.
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeRun {
    pub job: Option<i64>,
    pub at: Millis,
    pub root: String,
}

/// How a moment of the timeline reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Plain,
}

/// One line of the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub at: Millis,
    pub job: Option<i64>,
    pub tone: Tone,
    pub text: String,
}

/// What the ledger says about one window.
#[derive(Clone, Debug, PartialEq)]
pub struct Digest {
    pub from: Millis,
    pub to: Millis,
    /// Ledger entries inside the window.
    pub entries: u32,
    /// The oldest entries of the window were read; false when the read was
    /// cut short and the figures describe only the newest part.
    pub complete: bool,
    pub spend: Option<Spend>,
    pub guard: GuardDigest,
    pub fallbacks: Vec<Fallback>,
    pub rules_added: Vec<RuleAdded>,
    pub hidden_not_run: Vec<HiddenNotRun>,
    pub worktree_runs: Vec<WorktreeRun>,
    /// Oldest first, at most [`TIMELINE_KEPT`] (the newest).
    pub timeline: Vec<Moment>,
    /// The verification figures over the same window.
    pub metrics: Metrics,
}

fn payload(e: &LedgerEntry) -> Option<Json> {
    Json::parse(&e.draft.payload_json).ok()
}

fn text<'a>(p: &'a Json, key: &str) -> &'a str {
    p.get(key).and_then(Json::as_str).unwrap_or("")
}

fn millis(p: &Json, key: &str) -> Option<Millis> {
    p.get(key)
        .and_then(Json::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n as Millis)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Shortens `s` to `max` characters with an ellipsis, on one line.
fn short(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let cut: String = one.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Works out the digest of the window `from..=to` over `entries`, in
/// ledger order. Entries after `to` are ignored; entries before `from`
/// only tell what a job had cost before. `reached_start` says the read went
/// back to the window's start (or the ledger's first entry).
pub fn digest(entries: &[LedgerEntry], from: Millis, to: Millis, reached_start: bool) -> Digest {
    let end = entries
        .iter()
        .rposition(|e| e.draft.ts <= to)
        .map_or(0, |i| i + 1);
    let upto = &entries[..end];
    let inside = |e: &LedgerEntry| e.draft.ts >= from;

    let mut last_cost: BTreeMap<i64, f64> = BTreeMap::new();
    let mut spend = Spend::default();
    let mut costed: Vec<i64> = Vec::new();
    let mut guard = GuardDigest::default();
    let mut fallbacks = Vec::new();
    let mut rules_added = Vec::new();
    let mut hidden: BTreeMap<Option<i64>, Option<HiddenNotRun>> = BTreeMap::new();
    let mut in_worktree: BTreeMap<Option<i64>, Option<WorktreeRun>> = BTreeMap::new();
    let mut timeline: Vec<Moment> = Vec::new();
    let mut entry_count = 0usize;

    for e in upto {
        let d = &e.draft;
        let job = d.job.map(|JobId(j)| j);
        let counts = inside(e);
        if counts {
            entry_count += 1;
        }
        let Some(p) = payload(e) else { continue };
        match d.kind {
            LedgerKind::JobFinished => {
                let (Some(j), Some(cost)) = (job, p.get("cost_usd").and_then(Json::as_f64)) else {
                    if counts && let Some(j) = job {
                        timeline.push(finished_moment(d.ts, j, &p, None));
                    }
                    continue;
                };
                if counts {
                    let before = last_cost.get(&j).copied().unwrap_or(0.0);
                    spend.usd += (cost - before).max(0.0);
                    if !costed.contains(&j) {
                        costed.push(j);
                    }
                    if p.get("revised").and_then(Json::as_bool) != Some(true) {
                        timeline.push(finished_moment(d.ts, j, &p, Some(cost)));
                    }
                }
                last_cost.insert(j, cost);
            }
            LedgerKind::GuardDecision if counts => {
                guard_entry(&mut guard, d.ts, job, &p, &mut timeline);
            }
            LedgerKind::Policy if counts => match text(&p, "event") {
                "guard_fallback" => {
                    let at = millis(&p, "at").unwrap_or(d.ts);
                    let f = Fallback {
                        at,
                        job,
                        tool: text(&p, "tool").to_string(),
                        reason: text(&p, "reason").to_string(),
                        mode: text(&p, "mode").to_string(),
                    };
                    timeline.push(Moment {
                        at,
                        job,
                        tone: Tone::Warn,
                        text: format!("{} fell back: swarmux was not reachable", f.tool),
                    });
                    fallbacks.push(f);
                }
                "added" => {
                    let r = RuleAdded {
                        at: d.ts,
                        rule: text(&p, "rule").to_string(),
                        scope: text(&p, "scope").to_string(),
                        root: Some(text(&p, "root"))
                            .filter(|r| !r.is_empty())
                            .map(str::to_string),
                        source: text(&p, "source").to_string(),
                        expires_at: millis(&p, "expires_at"),
                    };
                    timeline.push(Moment {
                        at: d.ts,
                        job: None,
                        tone: Tone::Warn,
                        text: format!("always-allow added: {}", short(&r.rule, 60)),
                    });
                    rules_added.push(r);
                }
                _ => {}
            },
            LedgerKind::GateVerdict if counts => {
                if let Some(h) = p.get("hidden_not_run") {
                    hidden.insert(
                        job,
                        Some(HiddenNotRun {
                            job,
                            at: d.ts,
                            planned: p
                                .get("hidden_not_run")
                                .and_then(|h| h.get("planned"))
                                .and_then(Json::as_u64)
                                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
                            reason: text(h, "reason").to_string(),
                        }),
                    );
                } else {
                    hidden.insert(job, None);
                }
                in_worktree.insert(
                    job,
                    (text(&p, "ran_in") == "worktree").then(|| WorktreeRun {
                        job,
                        at: d.ts,
                        root: text(&p, "root").to_string(),
                    }),
                );
                // Only the judgement of a claim is a moment: a manual run is not.
                if p.get("event").is_some() {
                    let (tone, what) = match text(&p, "outcome") {
                        "pass" => (Tone::Good, "gate passed"),
                        "block" => (Tone::Warn, "gate blocked the claim"),
                        "escalate" => (Tone::Warn, "gate could not vouch: escalated"),
                        "fail" => (Tone::Bad, "gate failed"),
                        _ => continue,
                    };
                    timeline.push(Moment {
                        at: d.ts,
                        job,
                        tone,
                        text: what.to_string(),
                    });
                }
            }
            LedgerKind::Mission if counts => {
                let to_state = text(&p, "to");
                let (tone, what) = match to_state {
                    "delivered" => (Tone::Good, "delivered"),
                    "escalated" => (Tone::Bad, "escalated"),
                    "refused" => (Tone::Bad, "refused"),
                    _ => continue,
                };
                let title = Some(text(&p, "title"))
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| text(&p, "mission"));
                timeline.push(Moment {
                    at: d.ts,
                    job,
                    tone,
                    text: format!("{} {what}", short(title, 50)),
                });
            }
            LedgerKind::Evidence if counts => timeline.push(Moment {
                at: d.ts,
                job,
                tone: Tone::Good,
                text: format!("evidence sealed for {}", text(&p, "mission")),
            }),
            LedgerKind::Operator if counts => timeline.push(Moment {
                at: d.ts,
                job,
                tone: Tone::Plain,
                text: "you sent a note to the agent".to_string(),
            }),
            _ => {}
        }
    }
    spend.jobs = count(costed.len());

    timeline.sort_by_key(|m| m.at);
    if timeline.len() > TIMELINE_KEPT {
        timeline.drain(..timeline.len() - TIMELINE_KEPT);
    }
    Digest {
        from,
        to,
        entries: count(entry_count),
        complete: reached_start,
        spend: (!costed.is_empty()).then_some(spend),
        guard,
        fallbacks,
        rules_added,
        hidden_not_run: hidden.into_values().flatten().collect(),
        worktree_runs: in_worktree.into_values().flatten().collect(),
        timeline,
        metrics: compute_metrics(upto, to, (to - from).max(0), reached_start),
    }
}

fn finished_moment(at: Millis, job: i64, p: &Json, cost: Option<f64>) -> Moment {
    let status = text(p, "status");
    let (tone, word) = match status {
        "done" => (Tone::Good, "finished"),
        "failed" => (Tone::Bad, "failed"),
        "interrupted" => (Tone::Warn, "was interrupted"),
        "cancelled" => (Tone::Plain, "was cancelled"),
        _ => (Tone::Plain, "ended"),
    };
    let text = match cost {
        Some(c) => format!("{word} · ${c:.2}"),
        None => word.to_string(),
    };
    Moment {
        at,
        job: Some(job),
        tone,
        text,
    }
}

fn guard_entry(
    guard: &mut GuardDigest,
    at: Millis,
    job: Option<i64>,
    p: &Json,
    timeline: &mut Vec<Moment>,
) {
    let tool = text(p, "tool");
    let subject = text(p, "subject");
    let rule = Some(text(p, "rule"))
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    let call = |state: CallState| GuardCall {
        at,
        job,
        tool: tool.to_string(),
        subject: subject.to_string(),
        rule: rule.clone(),
        state,
    };
    let held = |c: &&mut GuardCall| {
        c.state == CallState::Held && c.job == job && c.tool == tool && c.subject == subject
    };
    match text(p, "outcome") {
        "ask" => {
            guard.calls.push(call(CallState::Held));
            timeline.push(Moment {
                at,
                job,
                tone: Tone::Warn,
                text: format!("held: {} {}", tool, short(subject, 50)),
            });
        }
        "allow" => match guard.calls.iter_mut().rev().find(held) {
            Some(c) => c.state = CallState::HeldAllowed,
            None => guard.allowed = guard.allowed.saturating_add(1),
        },
        "deny" => match guard.calls.iter_mut().rev().find(held) {
            Some(c) => c.state = CallState::HeldDenied,
            None => {
                guard.calls.push(call(CallState::Denied));
                timeline.push(Moment {
                    at,
                    job,
                    tone: Tone::Bad,
                    text: format!("denied: {} {}", tool, short(subject, 50)),
                });
            }
        },
        "stop" => {
            guard.calls.push(call(CallState::Stopped));
            timeline.push(Moment {
                at,
                job,
                tone: Tone::Bad,
                text: "the loop breaker stopped the job".to_string(),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{LedgerDraft, Payload};

    const MIN: Millis = 60_000;

    fn chain(drafts: Vec<LedgerDraft>) -> Vec<LedgerEntry> {
        let mut out: Vec<LedgerEntry> = Vec::new();
        for d in drafts {
            let e = LedgerEntry::next(out.last(), d);
            out.push(e);
        }
        out
    }

    fn at(ts: Millis, kind: LedgerKind, actor: &str, job: Option<i64>, p: Payload) -> LedgerDraft {
        LedgerDraft {
            ts,
            kind,
            actor: actor.into(),
            job: job.map(JobId),
            session: None,
            payload_json: p.build(),
        }
    }

    fn decision(ts: Millis, job: i64, outcome: &str, tool: &str, subject: &str) -> LedgerDraft {
        at(
            ts,
            LedgerKind::GuardDecision,
            "guard",
            Some(job),
            Payload::new()
                .str("outcome", outcome)
                .str("tool", tool)
                .str("subject", subject),
        )
    }

    fn finished(ts: Millis, job: i64, status: &str, cost: f64, revised: bool) -> LedgerDraft {
        let mut p = Payload::new().str("status", status).num("cost_usd", cost);
        if revised {
            p = p.bool("revised", true);
        }
        at(ts, LedgerKind::JobFinished, "swarmux", Some(job), p)
    }

    fn run(drafts: Vec<LedgerDraft>, from: Millis, to: Millis) -> Digest {
        digest(&chain(drafts), from, to, true)
    }

    #[test]
    fn an_empty_window_has_no_spend_and_no_zeros_made_up() {
        let d = run(vec![LedgerDraft::genesis(0, "t")], 10 * MIN, 20 * MIN);
        assert_eq!(d.spend, None);
        assert_eq!(d.entries, 0);
        assert!(d.timeline.is_empty() && d.guard.calls.is_empty());
        assert_eq!(d.metrics.first_run, None);
    }

    #[test]
    fn spend_is_what_jobs_that_ended_in_the_window_cost() {
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                finished(5 * MIN, 1, "done", 9.0, false),
                finished(11 * MIN, 2, "done", 1.5, false),
                finished(12 * MIN, 3, "failed", 0.25, false),
                finished(25 * MIN, 4, "done", 100.0, false),
            ],
            10 * MIN,
            20 * MIN,
        );
        let s = d.spend.unwrap();
        assert!((s.usd - 1.75).abs() < 1e-9, "{s:?}");
        assert_eq!(s.jobs, 2);
    }

    #[test]
    fn a_revised_figure_adds_only_what_it_changed() {
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                finished(5 * MIN, 1, "done", 2.0, false),
                finished(12 * MIN, 1, "done", 2.5, true),
                finished(13 * MIN, 2, "done", 1.0, false),
                finished(14 * MIN, 2, "done", 1.25, true),
            ],
            10 * MIN,
            20 * MIN,
        );
        let s = d.spend.unwrap();
        // Job 1: 0.5 more than before the window; job 2: 1.0 then 0.25 more.
        assert!((s.usd - 1.75).abs() < 1e-9, "{s:?}");
        assert_eq!(s.jobs, 2);
    }

    #[test]
    fn a_held_call_that_ends_is_not_a_routine_allow() {
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                decision(MIN, 1, "allow", "Read", "a.rs"),
                decision(2 * MIN, 1, "allow", "Read", "b.rs"),
                decision(3 * MIN, 1, "ask", "Bash", "git push"),
                decision(4 * MIN, 1, "allow", "Bash", "git push"),
                decision(5 * MIN, 2, "ask", "Bash", "rm -rf x"),
                decision(6 * MIN, 2, "deny", "Bash", "rm -rf x"),
                decision(7 * MIN, 3, "deny", "Bash", "curl evil"),
                decision(8 * MIN, 4, "ask", "Bash", "git reset --hard"),
                decision(9 * MIN, 5, "stop", "Bash", "loop"),
            ],
            0,
            10 * MIN,
        );
        assert_eq!(
            d.guard.allowed, 2,
            "the allow that answered a hold is not routine"
        );
        let states: Vec<CallState> = d.guard.calls.iter().map(|c| c.state).collect();
        assert_eq!(
            states,
            [
                CallState::HeldAllowed,
                CallState::HeldDenied,
                CallState::Denied,
                CallState::Held,
                CallState::Stopped
            ]
        );
    }

    #[test]
    fn a_hold_is_answered_only_by_the_same_job_tool_and_subject() {
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                decision(MIN, 1, "ask", "Bash", "git push"),
                decision(2 * MIN, 2, "allow", "Bash", "git push"),
            ],
            0,
            10 * MIN,
        );
        assert_eq!(d.guard.calls[0].state, CallState::Held);
        assert_eq!(d.guard.allowed, 1);
    }

    #[test]
    fn fallbacks_and_added_rules_come_from_policy_entries() {
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                at(
                    11 * MIN,
                    LedgerKind::Policy,
                    "guard-hook",
                    Some(7),
                    Payload::new()
                        .str("event", "guard_fallback")
                        .int("at", 4 * MIN)
                        .str("tool", "Bash")
                        .str("reason", "swarmux was not running")
                        .str("mode", "deny"),
                ),
                at(
                    12 * MIN,
                    LedgerKind::Policy,
                    "operator",
                    None,
                    Payload::new()
                        .str("event", "added")
                        .str("rule", "Bash(cargo test:*)")
                        .str("scope", "repo")
                        .str("root", "/work/api")
                        .str("source", "approval")
                        .int("expires_at", 99 * MIN),
                ),
                at(
                    13 * MIN,
                    LedgerKind::Policy,
                    "operator",
                    None,
                    Payload::new().str("event", "removed").str("rule", "x"),
                ),
            ],
            10 * MIN,
            20 * MIN,
        );
        assert_eq!(d.fallbacks.len(), 1);
        assert_eq!(d.fallbacks[0].job, Some(7));
        assert_eq!(d.fallbacks[0].at, 4 * MIN);
        assert_eq!(d.fallbacks[0].mode, "deny");
        assert_eq!(d.rules_added.len(), 1);
        assert_eq!(d.rules_added[0].root.as_deref(), Some("/work/api"));
        assert_eq!(d.rules_added[0].expires_at, Some(99 * MIN));
    }

    fn verdict(ts: Millis, job: i64, outcome: &str, extra: Payload) -> LedgerDraft {
        at(
            ts,
            LedgerKind::GateVerdict,
            "gate",
            Some(job),
            extra
                .str("event", "Stop")
                .str("outcome", outcome)
                .int("blocks", 0),
        )
    }

    #[test]
    fn hidden_checks_not_run_and_worktree_runs_follow_the_jobs_last_verdict() {
        let not_run = Payload::new().json(
            "hidden_not_run",
            &Payload::new()
                .int("planned", 3)
                .str("reason", "no clean copy was made")
                .build(),
        );
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                verdict(
                    MIN,
                    1,
                    "escalate",
                    not_run
                        .clone()
                        .str("ran_in", "worktree")
                        .str("root", "/w/a"),
                ),
                verdict(2 * MIN, 2, "escalate", not_run.clone()),
                // Job 2 is judged again, and now the hidden checks ran.
                verdict(
                    3 * MIN,
                    2,
                    "pass",
                    Payload::new().str("ran_in", "clean_copy"),
                ),
            ],
            0,
            10 * MIN,
        );
        assert_eq!(d.hidden_not_run.len(), 1);
        assert_eq!(d.hidden_not_run[0].job, Some(1));
        assert_eq!(d.hidden_not_run[0].planned, 3);
        assert_eq!(d.hidden_not_run[0].reason, "no clean copy was made");
        assert_eq!(d.worktree_runs.len(), 1);
        assert_eq!(d.worktree_runs[0].root, "/w/a");
    }

    #[test]
    fn the_timeline_is_oldest_first_and_leaves_out_routine_allows_and_manual_gate_runs() {
        let manual = at(
            3 * MIN,
            LedgerKind::GateVerdict,
            "gate",
            Some(1),
            Payload::new().str("outcome", "pass"),
        );
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                decision(MIN, 1, "allow", "Read", "a"),
                finished(2 * MIN, 1, "done", 0.5, false),
                manual,
                verdict(4 * MIN, 1, "pass", Payload::new()),
                at(
                    5 * MIN,
                    LedgerKind::Mission,
                    "swarmux",
                    Some(1),
                    Payload::new()
                        .str("mission", "fix-rounding")
                        .str("from", "verifying")
                        .str("to", "delivered")
                        .str("title", "Fix rounding"),
                ),
            ],
            0,
            10 * MIN,
        );
        let texts: Vec<&str> = d.timeline.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            ["finished · $0.50", "gate passed", "Fix rounding delivered"]
        );
        assert!(d.timeline.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn entries_after_the_window_are_ignored_and_the_figures_say_when_the_read_was_cut() {
        let drafts = vec![
            LedgerDraft::genesis(0, "t"),
            finished(5 * MIN, 1, "done", 1.0, false),
            finished(50 * MIN, 2, "done", 9.0, false),
        ];
        let d = digest(&chain(drafts), 0, 10 * MIN, false);
        assert_eq!(d.spend.unwrap().jobs, 1);
        assert!(!d.complete && !d.metrics.complete);
    }

    #[test]
    fn the_verification_figures_are_those_of_the_window() {
        let stop = |ts, job| {
            at(
                ts,
                LedgerKind::AgentEvent,
                "hook",
                Some(job),
                Payload::new().str("event", "Stop"),
            )
        };
        let d = run(
            vec![
                LedgerDraft::genesis(0, "t"),
                stop(11 * MIN, 1),
                verdict(11 * MIN, 1, "pass", Payload::new()),
                stop(12 * MIN, 2),
                verdict(12 * MIN, 2, "block", Payload::new()),
            ],
            10 * MIN,
            20 * MIN,
        );
        let f = d.metrics.first_run.unwrap();
        assert_eq!((f.passed, f.judged), (1, 2));
        assert_eq!(d.metrics.verified, 1);
    }
}
