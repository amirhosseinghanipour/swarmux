//! Reading a gate verdict back from the payload the ledger holds.

use super::CheckResult;
use crate::gate::GateCheck;
use crate::json::Json;
use crate::mission::check_of;
use crate::verifier::HiddenTally;

/// `{"hidden": {"ran": 3, "passed": 3}}`, as verdicts and bundles write it.
pub(crate) fn hidden_of(j: &Json) -> Option<HiddenTally> {
    let h = j.get("hidden")?;
    let count = |k: &str| {
        h.get(k)
            .and_then(Json::as_u64)
            .map(|n| n.min(u64::from(u32::MAX)) as u32)
    };
    Some(HiddenTally {
        ran: count("ran")?,
        passed: count("passed")?,
    })
}

/// What a gate verdict entry says.
#[derive(Clone, Debug, PartialEq)]
pub struct VerdictFacts {
    pub outcome: String,
    /// `stop`, `subagent_stop`; empty for a manual run.
    pub event: String,
    pub blocks: u32,
    pub runs: Vec<RunFacts>,
    /// How many hidden checks ran and passed, when any did.
    pub hidden: Option<HiddenTally>,
}

/// One check run, as the verdict recorded it.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFacts {
    pub name: String,
    pub command: String,
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub flaky: bool,
    pub spawn_error: bool,
    pub duration_ms: u64,
    pub first_failure: String,
}

impl RunFacts {
    pub fn passed(&self) -> bool {
        self.exit == Some(0) && !self.timed_out && !self.spawn_error
    }
}

/// Reads a `gate_verdict` payload; `None` when it isn't JSON.
pub fn parse_verdict(payload_json: &str) -> Option<VerdictFacts> {
    let j = Json::parse(payload_json).ok()?;
    let text = |o: &Json, k: &str| {
        o.get(k)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let runs = j
        .get("checks")
        .and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|r| RunFacts {
            name: text(r, "name"),
            command: text(r, "command"),
            exit: r.get("exit").and_then(Json::as_f64).map(|n| n as i32),
            timed_out: r.get("timed_out").and_then(Json::as_bool).unwrap_or(false),
            flaky: r.get("flaky").and_then(Json::as_bool).unwrap_or(false),
            spawn_error: r.get("spawn_error").and_then(Json::as_str).is_some(),
            duration_ms: r.get("duration_ms").and_then(Json::as_u64).unwrap_or(0),
            first_failure: text(r, "first_failure"),
        })
        .collect();
    Some(VerdictFacts {
        outcome: text(&j, "outcome"),
        event: text(&j, "event"),
        blocks: j.get("blocks").and_then(Json::as_u64).unwrap_or(0) as u32,
        runs,
        hidden: hidden_of(&j),
    })
}

/// The checks of a verdict with the command to run each again: the
/// definition among `defined` of the same name when there is one, else
/// the command line the verdict recorded.
pub fn checks_of(verdict: &VerdictFacts, defined: &[GateCheck]) -> Vec<CheckResult> {
    verdict
        .runs
        .iter()
        .filter_map(|run| {
            let def = defined
                .iter()
                .find(|d| d.name == run.name)
                .cloned()
                .or_else(|| check_of(&run.command))?;
            Some(
                CheckResult {
                    name: run.name.clone(),
                    program: def.program,
                    args: def.args,
                    timeout_secs: def.timeout_secs,
                    exit: run.exit,
                    passed: run.passed(),
                    flaky: run.flaky,
                    duration_ms: run.duration_ms,
                    first_failure: String::new(),
                }
                .with_failure(&run.first_failure),
            )
        })
        .collect()
}

/// The digest an `evidence` ledger entry payload names.
pub fn sealed_digest(payload_json: &str) -> Option<String> {
    Json::parse(payload_json)
        .ok()?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}
