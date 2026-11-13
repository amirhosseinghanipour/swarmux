//! Evidence bundles: the verifiable record of a delivered mission.
//!
//! A bundle names the contract, the commit the work ended at, what the
//! gate said and which checks ran, what the diff looked like, what it
//! cost and which stretch of the ledger tells the story. It is plain
//! JSON; its digest is the SHA-256 of the JSON without the signature, so
//! anyone can recompute it. `swarmux verify` runs the checks again on a
//! clean copy of that commit and compares.

mod cards;
mod diff;
mod verdict;
mod verify;

pub use cards::{
    Brief, Label, LabelKind, LedgerStats, PrOutcome, PrState, RunCard, RunEvidence, RunOutcome,
    SETTLE_MS, brief, heat_strip, label_of,
};
pub use diff::{ChangeKind, DiffFile, files_from_json, files_json, totals as diff_totals};
pub use verdict::{RunFacts, VerdictFacts, checks_of, parse_verdict, sealed_digest};
pub use verify::{Rerun, VerifyItem, VerifyReport, compare_checks};

use crate::anchor::Checkpoint;
use crate::json::Json;
use crate::ledger::{Hash, Payload, hex, sha256};
use crate::verifier::HiddenTally;
use crate::{JobId, Millis};

/// The bundle format version.
pub const BUNDLE_SCHEMA: u32 = 1;
/// Longest failure excerpt kept per check, in bytes.
const FAILURE_MAX: usize = 2000;

/// What the bundle is about: the mission's contract.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContractRef {
    pub slug: String,
    pub title: String,
    pub goal: String,
    /// The folder the mission worked in.
    pub root: String,
    pub checks: Vec<String>,
    pub protected: Vec<String>,
    pub paths_rw: Vec<String>,
    pub budget_usd: Option<f64>,
    pub wall_minutes: Option<u32>,
    pub human_signoff: bool,
}

/// One check and how it went when the gate ran it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
    pub exit: Option<i32>,
    pub passed: bool,
    pub flaky: bool,
    pub duration_ms: u64,
    /// The first failing line and what follows it, verbatim; empty on a pass.
    pub first_failure: String,
}

impl CheckResult {
    /// The command as a person would type it.
    pub fn command_line(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|a| {
                if a.is_empty() || a.contains([' ', '\t', '"']) {
                    format!("\"{}\"", a.replace('"', "\\\""))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn cut_failure(text: &str) -> String {
        let mut end = text.len().min(FAILURE_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_string()
    }

    pub fn with_failure(mut self, text: &str) -> CheckResult {
        self.first_failure = CheckResult::cut_failure(text);
        self
    }
}

/// One gate verdict on the job, with the digest of its ledger entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRef {
    pub seq: u64,
    pub outcome: String,
    /// `stop`, `subagent_stop` or empty for a manual run.
    pub event: String,
    pub blocks: u32,
    /// SHA-256 of the entry's payload, hex.
    pub digest: String,
}

/// A checkpoint taken during the job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointRef {
    pub commit: String,
    pub reason: String,
    pub seq: Option<u64>,
}

/// What the diff between the base and the head looked like.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
    /// SHA-256 of the diff text, hex; empty when there is no diff.
    pub hash: String,
    /// The files behind the totals, with the lines git counted in each.
    /// Empty for a bundle sealed before they were kept: its totals stand
    /// alone, and no per-file count is ever worked out after the fact.
    pub changes: Vec<DiffFile>,
}

/// The reviewer's verdict, when a review ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRef {
    pub verdict: String,
    pub concerns: u32,
    pub job: i64,
}

/// What the mission cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cost {
    pub total_usd: f64,
    /// Where the figure comes from: `reported` is what Claude Code said.
    pub basis: String,
    pub plan: f64,
    pub implement: f64,
    pub verify: f64,
    pub review: f64,
    /// Dollars per model, largest first.
    pub by_model: Vec<(String, f64)>,
}

/// The stretch of the ledger that tells the story.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LedgerRange {
    pub from_seq: u64,
    pub to_seq: u64,
    /// The hash of entry `to_seq`, hex.
    pub head_hash: String,
}

/// A signature over the bundle's digest. Until real keys exist the
/// algorithm is `sha256` and the value is the digest itself: it detects
/// edits, it does not prove who wrote the bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub alg: String,
    pub value: String,
}

/// The verifiable record of one delivered mission.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bundle {
    pub schema: u32,
    pub contract: ContractRef,
    pub job: i64,
    pub attempt: u32,
    pub session: Option<String>,
    /// The repository's top folder: where the commits live.
    pub repo: String,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: String,
    /// Files were uncommitted at the head when the bundle was sealed, so
    /// the commit alone doesn't hold the work.
    pub dirty: bool,
    pub diff: DiffSummary,
    pub checks: Vec<CheckResult>,
    /// How many hidden checks the passing verdict ran and passed. Never
    /// which ones: their names, commands and output stay out of bundles.
    pub hidden: Option<HiddenTally>,
    pub gates: Vec<GateRef>,
    pub review: Option<ReviewRef>,
    pub checkpoints: Vec<CheckpointRef>,
    pub cost: Cost,
    pub wall_ms: u64,
    pub ledger: LedgerRange,
    pub sealed_at: Millis,
    /// The signed checkpoint of the ledger that covers the bundle's
    /// entries, when one could be taken: ties the bundle to the ledger
    /// head with a key that is not in the database.
    pub anchor: Option<Checkpoint>,
    pub signature: Option<Signature>,
}

fn check_json(c: &CheckResult) -> Payload {
    Payload::new()
        .str("name", &c.name)
        .str("program", &c.program)
        .strs("args", &c.args)
        .int("timeout_secs", clamp(c.timeout_secs))
        .opt_int("exit", c.exit.map(i64::from))
        .bool("passed", c.passed)
        .bool("flaky", c.flaky)
        .int("duration_ms", clamp(c.duration_ms))
        .str("first_failure", &c.first_failure)
}

fn clamp(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl Bundle {
    fn body(&self) -> Payload {
        let k = &self.contract;
        let contract = Payload::new()
            .str("slug", &k.slug)
            .str("title", &k.title)
            .str("goal", &k.goal)
            .str("root", &k.root)
            .strs("checks", &k.checks)
            .strs("protected", &k.protected)
            .strs("paths_rw", &k.paths_rw)
            .opt_num("budget_usd", k.budget_usd)
            .opt_int("wall_minutes", k.wall_minutes.map(i64::from))
            .bool("human_signoff", k.human_signoff);
        let diff = Payload::new()
            .int("files", i64::from(self.diff.files))
            .int("insertions", i64::from(self.diff.insertions))
            .int("deletions", i64::from(self.diff.deletions))
            .str("hash", &self.diff.hash);
        // Written only when files were kept, so bundles sealed before
        // they were keep their digest.
        let diff = if self.diff.changes.is_empty() {
            diff
        } else {
            diff.objects(
                "changes",
                self.diff.changes.iter().map(DiffFile::payload).collect(),
            )
        };
        let gates = self
            .gates
            .iter()
            .map(|g| {
                Payload::new()
                    .int("seq", clamp(g.seq))
                    .str("outcome", &g.outcome)
                    .str("event", &g.event)
                    .int("blocks", i64::from(g.blocks))
                    .str("digest", &g.digest)
            })
            .collect();
        let checkpoints = self
            .checkpoints
            .iter()
            .map(|c| {
                Payload::new()
                    .str("commit", &c.commit)
                    .str("reason", &c.reason)
                    .opt_int("seq", c.seq.map(clamp))
            })
            .collect();
        let review = match &self.review {
            Some(r) => Payload::new()
                .str("verdict", &r.verdict)
                .int("concerns", i64::from(r.concerns))
                .int("job", r.job)
                .build(),
            None => "null".to_string(),
        };
        let models = self
            .cost
            .by_model
            .iter()
            .map(|(m, usd)| Payload::new().str("model", m).num("usd", *usd))
            .collect();
        let cost = Payload::new()
            .num("total_usd", self.cost.total_usd)
            .str("basis", &self.cost.basis)
            .num("plan", self.cost.plan)
            .num("implement", self.cost.implement)
            .num("verify", self.cost.verify)
            .num("review", self.cost.review)
            .objects("by_model", models);
        let ledger = Payload::new()
            .int("from_seq", clamp(self.ledger.from_seq))
            .int("to_seq", clamp(self.ledger.to_seq))
            .str("head_hash", &self.ledger.head_hash);
        let body = Payload::new()
            .int("schema", i64::from(self.schema))
            .json("contract", &contract.build())
            .int("job", self.job)
            .int("attempt", i64::from(self.attempt))
            .opt_str("session", self.session.as_deref())
            .str("repo", &self.repo)
            .opt_str("branch", self.branch.as_deref())
            .opt_str("base_sha", self.base_sha.as_deref())
            .str("head_sha", &self.head_sha)
            .bool("dirty", self.dirty)
            .json("diff", &diff.build())
            .objects("checks", self.checks.iter().map(check_json).collect());
        // Written only when hidden checks ran, so bundles sealed before
        // they existed keep their digest.
        let body = match self.hidden {
            Some(h) => body.json(
                "hidden",
                &Payload::new()
                    .int("ran", i64::from(h.ran))
                    .int("passed", i64::from(h.passed))
                    .build(),
            ),
            None => body,
        };
        let body = body
            .objects("gates", gates)
            .json("review", &review)
            .objects("checkpoints", checkpoints)
            .json("cost", &cost.build())
            .int("wall_ms", clamp(self.wall_ms))
            .json("ledger", &ledger.build())
            .int("sealed_at", self.sealed_at);
        // Written only when a checkpoint was taken, so bundles sealed
        // before anchoring existed keep their digest.
        match &self.anchor {
            Some(a) => body.json("anchor", &a.to_json()),
            None => body,
        }
    }

    /// The JSON the digest is taken over: everything but the signature.
    pub fn body_json(&self) -> String {
        self.body().build()
    }

    pub fn digest(&self) -> Hash {
        sha256(self.body_json().as_bytes())
    }

    pub fn digest_hex(&self) -> String {
        hex(&self.digest())
    }

    /// The bundle file's text: the body, then the signature.
    pub fn to_json(&self) -> String {
        let signature = match &self.signature {
            Some(s) => Payload::new()
                .str("alg", &s.alg)
                .str("value", &s.value)
                .build(),
            None => "null".to_string(),
        };
        // The body's closing brace makes room for the signature.
        let mut text = self.body_json();
        text.pop();
        format!("{text},\"signature\":{signature}}}")
    }

    /// Whether the signature matches the content. Only the `sha256` stub
    /// exists; any other algorithm can't be checked here.
    pub fn signature_ok(&self) -> Result<bool, String> {
        match &self.signature {
            None => Err("not signed".into()),
            Some(s) if s.alg == "sha256" => Ok(s.value == self.digest_hex()),
            Some(s) => Err(format!("unknown signature algorithm {}", s.alg)),
        }
    }

    pub fn from_json(text: &str) -> Result<Bundle, String> {
        let j = Json::parse(text)?;
        let schema = num(&j, "schema")? as u32;
        if schema != BUNDLE_SCHEMA {
            return Err(format!("bundle schema {schema} is not supported"));
        }
        let c = j.get("contract").ok_or("no contract")?;
        let d = j.get("diff").ok_or("no diff")?;
        let cost = j.get("cost").ok_or("no cost")?;
        let l = j.get("ledger").ok_or("no ledger")?;
        let signature = match j.get("signature") {
            Some(s @ Json::Obj(_)) => Some(Signature {
                alg: string(s, "alg")?,
                value: string(s, "value")?,
            }),
            _ => None,
        };
        let review = match j.get("review") {
            Some(r @ Json::Obj(_)) => Some(ReviewRef {
                verdict: string(r, "verdict")?,
                concerns: num(r, "concerns")? as u32,
                job: num(r, "job")? as i64,
            }),
            _ => None,
        };
        Ok(Bundle {
            schema,
            contract: ContractRef {
                slug: string(c, "slug")?,
                title: string(c, "title")?,
                goal: string(c, "goal")?,
                root: string(c, "root")?,
                checks: strings(c, "checks"),
                protected: strings(c, "protected"),
                paths_rw: strings(c, "paths_rw"),
                budget_usd: c.get("budget_usd").and_then(Json::as_f64),
                wall_minutes: c
                    .get("wall_minutes")
                    .and_then(Json::as_u64)
                    .map(|n| n as u32),
                human_signoff: c
                    .get("human_signoff")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            },
            job: num(&j, "job")? as i64,
            attempt: num(&j, "attempt")? as u32,
            session: opt_string(&j, "session"),
            repo: string(&j, "repo")?,
            branch: opt_string(&j, "branch"),
            base_sha: opt_string(&j, "base_sha"),
            head_sha: string(&j, "head_sha")?,
            dirty: j.get("dirty").and_then(Json::as_bool).unwrap_or(false),
            diff: DiffSummary {
                files: num(d, "files")? as u32,
                insertions: num(d, "insertions")? as u32,
                deletions: num(d, "deletions")? as u32,
                hash: string(d, "hash")?,
                changes: files_from_json(d.get("changes")),
            },
            checks: items(&j, "checks")
                .iter()
                .map(|c| {
                    Ok(CheckResult {
                        name: string(c, "name")?,
                        program: string(c, "program")?,
                        args: strings(c, "args"),
                        timeout_secs: num(c, "timeout_secs")?,
                        exit: c.get("exit").and_then(Json::as_f64).map(|n| n as i32),
                        passed: c.get("passed").and_then(Json::as_bool).unwrap_or(false),
                        flaky: c.get("flaky").and_then(Json::as_bool).unwrap_or(false),
                        duration_ms: num(c, "duration_ms")?,
                        first_failure: string(c, "first_failure")?,
                    })
                })
                .collect::<Result<_, String>>()?,
            hidden: verdict::hidden_of(&j),
            gates: items(&j, "gates")
                .iter()
                .map(|g| {
                    Ok(GateRef {
                        seq: num(g, "seq")?,
                        outcome: string(g, "outcome")?,
                        event: string(g, "event")?,
                        blocks: num(g, "blocks")? as u32,
                        digest: string(g, "digest")?,
                    })
                })
                .collect::<Result<_, String>>()?,
            review,
            checkpoints: items(&j, "checkpoints")
                .iter()
                .map(|c| {
                    Ok(CheckpointRef {
                        commit: string(c, "commit")?,
                        reason: string(c, "reason")?,
                        seq: c.get("seq").and_then(Json::as_u64),
                    })
                })
                .collect::<Result<_, String>>()?,
            cost: Cost {
                total_usd: cost.get("total_usd").and_then(Json::as_f64).unwrap_or(0.0),
                basis: string(cost, "basis")?,
                plan: cost.get("plan").and_then(Json::as_f64).unwrap_or(0.0),
                implement: cost.get("implement").and_then(Json::as_f64).unwrap_or(0.0),
                verify: cost.get("verify").and_then(Json::as_f64).unwrap_or(0.0),
                review: cost.get("review").and_then(Json::as_f64).unwrap_or(0.0),
                by_model: items(cost, "by_model")
                    .iter()
                    .filter_map(|m| {
                        Some((
                            m.get("model")?.as_str()?.to_string(),
                            m.get("usd")?.as_f64()?,
                        ))
                    })
                    .collect(),
            },
            wall_ms: num(&j, "wall_ms")?,
            ledger: LedgerRange {
                from_seq: num(l, "from_seq")?,
                to_seq: num(l, "to_seq")?,
                head_hash: string(l, "head_hash")?,
            },
            sealed_at: num(&j, "sealed_at")? as i64,
            anchor: match j.get("anchor") {
                Some(a @ Json::Obj(_)) => Some(Checkpoint::from_value(a)?),
                _ => None,
            },
            signature,
        })
    }

    /// The share of checks that passed, as (passed, total).
    pub fn check_tally(&self) -> (usize, usize) {
        (
            self.checks.iter().filter(|c| c.passed).count(),
            self.checks.len(),
        )
    }

    /// Stops the gate blocked before it passed.
    pub fn blocks(&self) -> u32 {
        self.gates.iter().filter(|g| g.outcome == "block").count() as u32
    }
}

fn num(j: &Json, key: &str) -> Result<u64, String> {
    match j.get(key) {
        Some(Json::Num(n)) if n.is_finite() => Ok(if *n < 0.0 { 0 } else { *n as u64 }),
        _ => Err(format!("bundle has no number {key:?}")),
    }
}

fn string(j: &Json, key: &str) -> Result<String, String> {
    j.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("bundle has no text {key:?}"))
}

fn opt_string(j: &Json, key: &str) -> Option<String> {
    j.get(key).and_then(Json::as_str).map(str::to_string)
}

fn items<'a>(j: &'a Json, key: &str) -> &'a [Json] {
    j.get(key).and_then(Json::as_array).unwrap_or(&[])
}

fn strings(j: &Json, key: &str) -> Vec<String> {
    items(j, key)
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The job a bundle is about.
impl Bundle {
    pub fn job_id(&self) -> JobId {
        JobId(self.job)
    }
}

#[cfg(test)]
mod tests;
