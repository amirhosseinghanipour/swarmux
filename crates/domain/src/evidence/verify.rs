//! Comparing what a bundle says with what running its checks again shows.

use super::Bundle;

/// One thing verified: what the bundle claims and what was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyItem {
    pub subject: String,
    pub expected: String,
    pub found: String,
    pub ok: bool,
}

impl VerifyItem {
    pub fn new(subject: &str, expected: &str, found: &str) -> VerifyItem {
        VerifyItem {
            subject: subject.to_string(),
            expected: expected.to_string(),
            found: found.to_string(),
            ok: expected == found,
        }
    }

    pub fn ok(subject: &str, what: &str) -> VerifyItem {
        VerifyItem {
            subject: subject.to_string(),
            expected: what.to_string(),
            found: what.to_string(),
            ok: true,
        }
    }

    pub fn bad(subject: &str, expected: &str, found: &str) -> VerifyItem {
        VerifyItem {
            subject: subject.to_string(),
            expected: expected.to_string(),
            found: found.to_string(),
            ok: false,
        }
    }
}

/// The outcome of a re-run of one check, in a clean copy of the head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rerun {
    pub name: String,
    pub exit: Option<i32>,
    pub passed: bool,
    /// It could not run, or timed out.
    pub problem: Option<String>,
}

/// Every verification item and the verdict they add up to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub items: Vec<VerifyItem>,
}

impl VerifyReport {
    /// Every item matches.
    pub fn matches(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.ok)
    }

    pub fn mismatches(&self) -> impl Iterator<Item = &VerifyItem> {
        self.items.iter().filter(|i| !i.ok)
    }

    /// `match` or `mismatch`.
    pub fn verdict(&self) -> &'static str {
        if self.matches() { "match" } else { "mismatch" }
    }
}

fn state(passed: bool, exit: Option<i32>) -> String {
    match (passed, exit) {
        (true, _) => "pass".to_string(),
        (false, Some(code)) => format!("fail (exit {code})"),
        (false, None) => "fail".to_string(),
    }
}

/// Each check of the bundle against its re-run: the outcome has to be
/// the same, and a check that did not run is a mismatch.
pub fn compare_checks(bundle: &Bundle, reruns: &[Rerun]) -> Vec<VerifyItem> {
    let mut out: Vec<VerifyItem> = bundle
        .checks
        .iter()
        .map(|c| {
            let subject = format!("check {}", c.name);
            let expected = state(c.passed, c.exit);
            match reruns.iter().find(|r| r.name == c.name) {
                None => VerifyItem::bad(&subject, &expected, "not run"),
                Some(r) => match &r.problem {
                    Some(p) => VerifyItem::bad(&subject, &expected, p),
                    None => {
                        let found = state(r.passed, r.exit);
                        // A pass is a pass whatever the exit code; a
                        // failure has to fail the same way.
                        let ok = r.passed == c.passed && (c.passed || r.exit == c.exit);
                        VerifyItem {
                            subject,
                            expected,
                            found,
                            ok,
                        }
                    }
                },
            }
        })
        .collect();
    if bundle.checks.is_empty() {
        out.push(VerifyItem::bad(
            "checks",
            "at least one check",
            "none recorded",
        ));
    }
    out
}
