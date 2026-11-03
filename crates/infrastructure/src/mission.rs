//! Mission contracts as TOML, and a [`MissionStore`] that writes each
//! contract into its repository (`.swarmux/missions/<slug>.toml`, where it
//! can be versioned) before keeping the mission in the database.
//!
//! ```toml
//! title = "Fast search"
//! goal = "make search answer in under 50 ms on the bench corpus"
//!
//! [acceptance]
//! checks = ["cargo test -p search", "sh scripts/bench.sh --max-ms 50"]
//! human_signoff = false
//!
//! [constraints]
//! paths_rw = ["src/search/**"]
//! protected = ["tests/**", "scripts/bench.sh"]
//!
//! [budget]
//! usd = 2.0
//! wall_minutes = 90
//! on_exhaust = "escalate"
//!
//! [[clarifications]]
//! question = "Which corpus?"
//! answer = "the bench corpus"
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Deserialize;
use swarmux_application::{AppError, AppResult, MissionStore};
use swarmux_domain::mission::{
    Acceptance, Budget, Clarification, Constraints, Contract, MISSIONS_DIR, Mission, MissionState,
    OnExhaust,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContract {
    #[serde(default)]
    title: String,
    goal: String,
    #[serde(default)]
    acceptance: RawAcceptance,
    #[serde(default)]
    constraints: RawConstraints,
    #[serde(default)]
    budget: RawBudget,
    #[serde(default)]
    clarifications: Vec<RawClarification>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAcceptance {
    #[serde(default)]
    checks: Vec<String>,
    #[serde(default)]
    human_signoff: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConstraints {
    #[serde(default)]
    paths_rw: Vec<String>,
    #[serde(default)]
    protected: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBudget {
    usd: Option<f64>,
    wall_minutes: Option<u32>,
    on_exhaust: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClarification {
    question: String,
    #[serde(default)]
    answer: String,
}

/// Reads a contract; the error says what is wrong and where.
pub fn parse_contract(text: &str) -> Result<Contract, String> {
    let raw: RawContract = toml::from_str(text).map_err(|e| e.to_string())?;
    let on_exhaust = match raw.budget.on_exhaust.as_deref() {
        None => OnExhaust::default(),
        Some(s) => OnExhaust::parse(s).ok_or_else(|| {
            format!("budget.on_exhaust must be \"escalate\" or \"warn\", not {s:?}")
        })?,
    };
    if let Some(usd) = raw.budget.usd
        && !(usd.is_finite() && usd > 0.0)
    {
        return Err(format!("budget.usd must be above 0, not {usd}"));
    }
    Ok(Contract {
        title: raw.title,
        goal: raw.goal,
        acceptance: Acceptance {
            checks: raw.acceptance.checks,
            human_signoff: raw.acceptance.human_signoff,
        },
        constraints: Constraints {
            paths_rw: raw.constraints.paths_rw,
            protected: raw.constraints.protected,
        },
        budget: Budget {
            usd: raw.budget.usd,
            wall_minutes: raw.budget.wall_minutes,
            on_exhaust,
        },
        clarifications: raw
            .clarifications
            .into_iter()
            .map(|c| Clarification {
                question: c.question,
                answer: c.answer,
            })
            .collect(),
    })
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| quote(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// Writes a contract as TOML that [`parse_contract`] reads back.
pub fn render_contract(c: &Contract) -> String {
    let mut out = String::new();
    out.push_str(&format!("title = {}\n", quote(&c.title)));
    out.push_str(&format!("goal = {}\n", quote(&c.goal)));
    out.push_str("\n[acceptance]\n");
    out.push_str(&format!("checks = {}\n", list(&c.acceptance.checks)));
    out.push_str(&format!("human_signoff = {}\n", c.acceptance.human_signoff));
    out.push_str("\n[constraints]\n");
    out.push_str(&format!("paths_rw = {}\n", list(&c.constraints.paths_rw)));
    out.push_str(&format!("protected = {}\n", list(&c.constraints.protected)));
    out.push_str("\n[budget]\n");
    if let Some(usd) = c.budget.usd {
        out.push_str(&format!("usd = {usd:?}\n"));
    }
    if let Some(mins) = c.budget.wall_minutes {
        out.push_str(&format!("wall_minutes = {mins}\n"));
    }
    out.push_str(&format!(
        "on_exhaust = {}\n",
        quote(c.budget.on_exhaust.as_str())
    ));
    for q in &c.clarifications {
        out.push_str("\n[[clarifications]]\n");
        out.push_str(&format!("question = {}\n", quote(&q.question)));
        out.push_str(&format!("answer = {}\n", quote(&q.answer)));
    }
    out
}

/// Where a mission's contract is written in its repository.
pub fn contract_path(root: &Path, slug: &str) -> PathBuf {
    root.join(MISSIONS_DIR).join(format!("{slug}.toml"))
}

/// Writes contracts into their repositories, then hands the mission on.
/// Only a contract that can still change is written: once a mission is out
/// its file stays as it was when it left.
pub struct MissionFiles {
    inner: Rc<dyn MissionStore>,
}

impl MissionFiles {
    pub fn new(inner: Rc<dyn MissionStore>) -> MissionFiles {
        MissionFiles { inner }
    }
}

impl MissionStore for MissionFiles {
    fn save(&self, mission: &Mission) -> AppResult<()> {
        let editable = matches!(
            mission.state,
            MissionState::Draft | MissionState::Linted | MissionState::Refused
        );
        let root = Path::new(&mission.root);
        if editable && root.is_dir() {
            let path = contract_path(root, &mission.slug);
            let write = || -> std::io::Result<()> {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::write(&path, render_contract(&mission.contract))
            };
            write()
                .map_err(|e| AppError::Storage(format!("cannot write {}: {e}", path.display())))?;
        }
        self.inner.save(mission)
    }

    fn list(&self) -> AppResult<Vec<Mission>> {
        self.inner.list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn full() -> Contract {
        Contract {
            title: "Fast \"search\"".into(),
            goal: "make search answer\nin under 50 ms\t(p99)".into(),
            acceptance: Acceptance {
                checks: vec!["cargo test -p search".into(), "sh \"my bench.sh\"".into()],
                human_signoff: true,
            },
            constraints: Constraints {
                paths_rw: vec!["src/search/**".into()],
                protected: vec!["tests/**".into(), "C:\\oracles\\**".into()],
            },
            budget: Budget {
                usd: Some(2.5),
                wall_minutes: Some(90),
                on_exhaust: OnExhaust::Warn,
            },
            clarifications: vec![Clarification {
                question: "Which corpus?".into(),
                answer: String::new(),
            }],
        }
    }

    #[test]
    fn contracts_round_trip_through_toml() {
        let c = full();
        let text = render_contract(&c);
        assert_eq!(parse_contract(&text).unwrap(), c, "{text}");
        let bare = Contract {
            goal: "g".into(),
            ..Contract::default()
        };
        assert_eq!(parse_contract(&render_contract(&bare)).unwrap(), bare);
    }

    #[test]
    fn a_minimal_file_reads_with_defaults() {
        let c = parse_contract("goal = \"make it fast\"").unwrap();
        assert_eq!(c.goal, "make it fast");
        assert!(c.acceptance.checks.is_empty());
        assert_eq!(c.budget.on_exhaust, OnExhaust::Escalate);
    }

    #[test]
    fn bad_files_say_why() {
        for (text, why) in [
            ("title = \"x\"", "goal"),
            ("goal = \"g\"\nsurprise = 1", "surprise"),
            (
                "goal = \"g\"\n[budget]\non_exhaust = \"panic\"",
                "on_exhaust",
            ),
            ("goal = \"g\"\n[budget]\nusd = -1.0", "above 0"),
        ] {
            let e = parse_contract(text).unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
    }

    #[derive(Default)]
    struct Kept(RefCell<Vec<Mission>>);

    impl MissionStore for Kept {
        fn save(&self, m: &Mission) -> AppResult<()> {
            self.0.borrow_mut().push(m.clone());
            Ok(())
        }
        fn list(&self) -> AppResult<Vec<Mission>> {
            Ok(self.0.borrow().clone())
        }
    }

    #[test]
    fn editable_contracts_are_written_into_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let kept = Rc::new(Kept::default());
        let files = MissionFiles::new(kept.clone());
        let root = dir.path().display().to_string();
        let mut m = Mission::new("fast-search", &root, full(), 1);
        files.save(&m).unwrap();
        let path = dir.path().join(".swarmux/missions/fast-search.toml");
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(parse_contract(&text).unwrap(), full());

        // Once it is out the file is left alone.
        m.state = MissionState::Running;
        m.contract.goal = "changed".into();
        files.save(&m).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert_eq!(files.list().unwrap().len(), 2);

        // A root that is not a folder is only kept in the database.
        let gone = Mission::new("gone", "/no/such/folder", full(), 1);
        files.save(&gone).unwrap();
        assert_eq!(kept.0.borrow().len(), 3);
    }
}
