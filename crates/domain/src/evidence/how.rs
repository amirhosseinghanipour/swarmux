//! How the verdict that passed was reached, as a bundle of schema 2 records
//! it.
//!
//! A tick says a gate passed. It does not say where the checks ran, which
//! of the planned checks did not, whether hidden checks existed and did not
//! run, what the original tests said, or what the gate found and let by.
//! This is that, read off the verdict's ledger entry, so a bundle never
//! shows more certainty than the run had.

use crate::json::Json;
use crate::ledger::Payload;

/// What the base-suite step found, when it ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseSuiteRef {
    /// The original tests passed with the job's code.
    pub passed: bool,
    /// The files put back as the base commit had them.
    pub restored: Vec<String>,
    /// The verdict line.
    pub verdict: String,
}

/// How a passing verdict was reached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HowChecked {
    /// Where the visible checks ran: `clean_copy` or `worktree`; empty when
    /// the verdict did not say.
    pub ran_in: String,
    /// The checks the gate planned to run, by name.
    pub planned: Vec<String>,
    /// Checks the gate left out, with why.
    pub left_out: Vec<(String, String)>,
    /// Hidden checks that exist and did not run: how many, and why.
    pub hidden_not_run: Option<(u32, String)>,
    pub base_suite: Option<BaseSuiteRef>,
    /// Tests weakened: the evidence lines.
    pub tampering: Vec<String>,
    /// Harness files created, changed or removed: the evidence lines.
    pub substrate: Vec<String>,
    /// New or changed dependencies, one line per manifest or lockfile.
    pub dependencies: Vec<String>,
    /// The revert probe, when it ran: whether it proved the change, and why.
    pub revert_probe: Option<(bool, String)>,
    /// Why the gate could not vouch for the work, if it could not.
    pub incomplete: Vec<String>,
    /// The hash of the gate configuration that judged.
    pub policy_sha256: String,
}

impl HowChecked {
    /// The planned checks that are not among `ran` by name: planned and not
    /// run, whatever the reason.
    pub fn not_run<'a>(&'a self, ran: &'a [String]) -> Vec<&'a str> {
        self.planned
            .iter()
            .filter(|p| !ran.contains(p))
            .map(String::as_str)
            .collect()
    }

    /// `hidden checks: not run (no clean copy)`, when they did not run.
    pub fn hidden_text(&self) -> Option<String> {
        self.hidden_not_run
            .as_ref()
            .map(|(_, why)| format!("hidden checks: not run ({why})"))
    }

    pub(crate) fn to_json(&self) -> String {
        let left_out = self
            .left_out
            .iter()
            .map(|(n, w)| Payload::new().str("name", n).text("why", w))
            .collect();
        let mut p = Payload::new()
            .str("ran_in", &self.ran_in)
            .strs("planned", &self.planned)
            .objects("left_out", left_out)
            .strs("tampering", &self.tampering)
            .strs("substrate", &self.substrate)
            .strs("dependencies", &self.dependencies)
            .strs("incomplete", &self.incomplete)
            .str("policy_sha256", &self.policy_sha256);
        if let Some((planned, why)) = &self.hidden_not_run {
            p = p.json(
                "hidden_not_run",
                &Payload::new()
                    .int("planned", i64::from(*planned))
                    .text("reason", why)
                    .build(),
            );
        }
        if let Some(b) = &self.base_suite {
            p = p.json(
                "base_suite",
                &Payload::new()
                    .bool("passed", b.passed)
                    .strs("restored", &b.restored)
                    .text("verdict", &b.verdict)
                    .build(),
            );
        }
        if let Some((proved, detail)) = &self.revert_probe {
            p = p.json(
                "revert_probe",
                &Payload::new()
                    .bool("proved", *proved)
                    .text("detail", detail)
                    .build(),
            );
        }
        p.build()
    }

    /// Reads the `verification` object of a bundle (or the same keys of a
    /// gate verdict entry); a key that is not there reads as empty.
    pub fn from_value(v: &Json) -> HowChecked {
        let strs = |key: &str| -> Vec<String> {
            v.get(key)
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        };
        let text = |o: &Json, key: &str| {
            o.get(key)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        HowChecked {
            ran_in: text(v, "ran_in"),
            planned: strs("planned"),
            left_out: v
                .get("left_out")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .map(|o| (text(o, "name"), text(o, "why")))
                .collect(),
            hidden_not_run: v
                .get("hidden_not_run")
                .filter(|h| h.get("reason").is_some())
                .map(|h| {
                    (
                        h.get("planned")
                            .and_then(Json::as_u64)
                            .map_or(0, |n| n.min(u64::from(u32::MAX)) as u32),
                        text(h, "reason"),
                    )
                }),
            base_suite: v
                .get("base_suite")
                .filter(|b| matches!(b, Json::Obj(_)))
                .map(|b| BaseSuiteRef {
                    passed: b
                        .get("passed")
                        .or_else(|| b.get("outcome"))
                        .map(|p| p.as_bool().unwrap_or_else(|| p.as_str() == Some("pass")))
                        .unwrap_or(false),
                    restored: b
                        .get("restored")
                        .and_then(Json::as_array)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect(),
                    verdict: text(b, "verdict"),
                }),
            tampering: strs("tampering"),
            substrate: strs("substrate"),
            dependencies: strs("dependencies"),
            revert_probe: v
                .get("revert_probe")
                .filter(|p| matches!(p, Json::Obj(_)))
                .map(|p| {
                    (
                        p.get("proved").and_then(Json::as_bool).unwrap_or(false),
                        text(p, "detail"),
                    )
                }),
            incomplete: strs("incomplete"),
            policy_sha256: text(v, "policy_sha256"),
        }
    }
}
