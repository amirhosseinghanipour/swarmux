//! Mission contracts as TOML, and a [`MissionStore`] that writes each
//! contract into its repository (`.swarmux/missions/<slug>.toml`, where it
//! can be versioned) before keeping the mission in the database.
//!
//! ```toml
//! title = "Fast search"
//! goal = "make search answer in under 50 ms on the bench corpus"
//! due = 2026-11-14T16:00:00+03:30   # optional: when you need it by
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
//!
//! A workspace mission names the repositories it touches, producers
//! first, and may run checks in each of them, as `repo: command` lines or
//! as a table by repository (read into the same lines):
//!
//! ```toml
//! repos = ["backend-supabase", "app-flutter"]
//!
//! [acceptance]
//! checks = ["backend-supabase: supabase db lint"]
//!
//! [acceptance.by_repo]
//! app-flutter = ["flutter analyze", "flutter test"]
//! ```
//!
//! A workspace's root is often not a repository, so its missions are kept
//! in swarmux's own folder (`workspaces/<id>/missions/`) instead.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Deserialize;
use swarmux_application::{AppError, AppResult, MissionStore};
use swarmux_domain::mission::{
    Acceptance, Budget, Clarification, Constraints, Contract, MISSIONS_DIR, Mission, MissionState,
    OnExhaust,
};
use swarmux_domain::{Millis, iso_utc, parse_rfc3339_millis};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContract {
    /// The version of the contract format; read, not yet needed.
    #[allow(dead_code)]
    schema_version: Option<toml::Value>,
    #[serde(default)]
    title: String,
    goal: String,
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    acceptance: RawAcceptance,
    #[serde(default)]
    constraints: RawConstraints,
    #[serde(default)]
    budget: RawBudget,
    /// When it is needed by: a TOML datetime with an offset
    /// (`2026-11-14T16:00:00+03:30`, or `Z`), or the same as a string.
    due: Option<toml::Value>,
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
    /// Checks by repository: `app = ["flutter test"]`.
    #[serde(default)]
    by_repo: BTreeMap<String, Vec<String>>,
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

/// The due time of a contract: RFC 3339 with an offset (a TOML offset
/// datetime, or a string holding one), as unix milliseconds. A date alone
/// or a time with no offset says nothing about when, so it is refused
/// rather than read as some zone.
fn parse_due(value: &toml::Value) -> Result<Millis, String> {
    let text = match value {
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::String(s) => s.trim().to_string(),
        other => {
            return Err(format!(
                "due must be a timestamp like 2026-11-14T16:00:00+03:30 (RFC 3339), not {}",
                other.type_str()
            ));
        }
    };
    parse_rfc3339_millis(&text).ok_or_else(|| {
        format!("due must be an RFC 3339 timestamp with an offset or Z, like 2026-11-14T16:00:00+03:30, not {text:?}")
    })
}

/// Reads a contract; the error says what is wrong and where.
pub fn parse_contract(text: &str) -> Result<Contract, String> {
    let raw: RawContract = toml::from_str(text).map_err(|e| e.to_string())?;
    let due = raw.due.as_ref().map(parse_due).transpose()?;
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
    let mut checks = raw.acceptance.checks;
    for (repo, lines) in raw.acceptance.by_repo {
        let repo = repo.trim();
        if repo.is_empty() || repo.contains(char::is_whitespace) {
            return Err(format!(
                "acceptance.by_repo: {repo:?} is not a repository name"
            ));
        }
        checks.extend(
            lines
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| format!("{repo}: {}", l.trim())),
        );
    }
    Ok(Contract {
        title: raw.title,
        goal: raw.goal,
        repos: raw.repos,
        acceptance: Acceptance {
            checks,
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
        due,
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
    let mut out = String::from(
        "schema_version = 1
",
    );
    out.push_str(&format!("title = {}\n", quote(&c.title)));
    out.push_str(&format!("goal = {}\n", quote(&c.goal)));
    if !c.repos.is_empty() {
        out.push_str(&format!("repos = {}\n", list(&c.repos)));
    }
    // A TOML datetime, in UTC: the same instant whatever offset it was
    // written with. Before the first table, where top-level keys belong.
    if let Some(due) = c.due {
        out.push_str(&format!("due = {}\n", iso_utc(due)));
    }
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

/// Where a workspace mission's contract is written: in swarmux's folder
/// for the workspace, since its root is often not a repository.
pub fn workspace_contract_path(workspaces: &Path, mission: &Mission) -> Option<PathBuf> {
    let id = mission.workspace?;
    Some(
        workspaces
            .join(id.to_string())
            .join("missions")
            .join(format!("{}.toml", mission.slug)),
    )
}

/// Writes contracts into their repositories (a workspace mission's into
/// swarmux's folder for its workspace), then hands the mission on. Only a
/// contract that can still change is written: once a mission is out its
/// file stays as it was when it left.
pub struct MissionFiles {
    inner: Rc<dyn MissionStore>,
    /// Where workspaces keep their missions.
    workspaces: Option<PathBuf>,
}

impl MissionFiles {
    pub fn new(inner: Rc<dyn MissionStore>) -> MissionFiles {
        MissionFiles {
            inner,
            workspaces: None,
        }
    }

    /// Writes workspace missions under `dir` (`<dir>/<workspace id>/missions/`).
    pub fn with_workspace_dir(mut self, dir: PathBuf) -> MissionFiles {
        self.workspaces = Some(dir);
        self
    }
}

impl MissionStore for MissionFiles {
    fn save(&self, mission: &Mission) -> AppResult<()> {
        let editable = matches!(
            mission.state,
            MissionState::Draft | MissionState::Linted | MissionState::Refused
        );
        let root = Path::new(&mission.root);
        let path = match (&self.workspaces, mission.workspace) {
            (Some(dir), Some(_)) => workspace_contract_path(dir, mission),
            (None, Some(_)) => None,
            _ => root.is_dir().then(|| contract_path(root, &mission.slug)),
        };
        if let (true, Some(path)) = (editable, path) {
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
            due: Some(1_793_613_600_000),
            repos: vec!["backend".into(), "app".into()],
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
    fn due_is_an_optional_rfc3339_timestamp() {
        // Absent: no due time, and an old contract reads as it always did.
        let old = parse_contract("goal = \"g\"\n[budget]\nusd = 1.0\n").unwrap();
        assert_eq!(old.due, None);
        assert!(!render_contract(&old).contains("due"));
        // A native TOML datetime with Z, with an offset, or a string.
        let at = 1_793_613_600_000; // 2026-11-02T10:00:00Z
        for text in [
            "due = 2026-11-02T10:00:00Z\ngoal = \"g\"",
            "due = 2026-11-02T13:30:00+03:30\ngoal = \"g\"",
            "due = \"2026-11-02T10:00:00Z\"\ngoal = \"g\"",
            "goal = \"g\"\ndue = 2026-11-02T10:00:00.000Z",
        ] {
            assert_eq!(parse_contract(text).unwrap().due, Some(at), "{text}");
        }
        // It is written before the first table, as a datetime, in UTC, and
        // reads back as the same instant.
        let mut c = parse_contract("goal = \"g\"\ndue = 2026-11-02T13:30:00+03:30").unwrap();
        let text = render_contract(&c);
        assert!(text.contains("due = 2026-11-02T10:00:00Z\n"), "{text}");
        assert!(text.find("due =").unwrap() < text.find("[acceptance]").unwrap());
        assert_eq!(parse_contract(&text).unwrap().due, Some(at));
        c.due = None;
        assert_eq!(parse_contract(&render_contract(&c)).unwrap().due, None);
    }

    #[test]
    fn a_bad_due_says_what_it_wants() {
        for (text, needle) in [
            ("goal = \"g\"\ndue = 2026-11-02", "RFC 3339"),
            ("goal = \"g\"\ndue = 2026-11-02T10:00:00", "RFC 3339"),
            ("goal = \"g\"\ndue = \"tomorrow\"", "RFC 3339"),
            ("goal = \"g\"\ndue = \"\"", "RFC 3339"),
            ("goal = \"g\"\ndue = 5", "not integer"),
            ("goal = \"g\"\ndue = true", "not boolean"),
        ] {
            let e = parse_contract(text).unwrap_err();
            assert!(e.contains(needle), "{text}: {e}");
            assert!(e.contains("due"), "{text}: {e}");
        }
    }

    #[test]
    fn checks_by_repository_become_prefixed_lines() {
        let c = parse_contract(
            "goal = \"g\"\nrepos = [\"backend\", \"app\"]\n[acceptance]\nchecks = [\"backend: supabase db lint\"]\n[acceptance.by_repo]\napp = [\"flutter analyze\", \" flutter test \", \"\"]\n",
        )
        .unwrap();
        assert_eq!(c.repos, ["backend", "app"]);
        assert_eq!(
            c.acceptance.checks,
            [
                "backend: supabase db lint",
                "app: flutter analyze",
                "app: flutter test"
            ]
        );
        assert_eq!(parse_contract(&render_contract(&c)).unwrap(), c);
        let bad = parse_contract("goal = \"g\"\n[acceptance.by_repo]\n\"a b\" = [\"x\"]\n");
        assert!(bad.unwrap_err().contains("by_repo"));
    }

    #[test]
    fn workspace_missions_are_kept_in_swarmuxs_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("product");
        fs::create_dir_all(&root).unwrap();
        let kept = Rc::new(Kept::default());
        let files = MissionFiles::new(kept.clone()).with_workspace_dir(dir.path().join("ws"));
        let mut m = Mission::new("guest", &root.display().to_string(), full(), 1);
        let id = swarmux_domain::WorkspaceId::random();
        m.workspace = Some(id);
        files.save(&m).unwrap();
        let path = dir
            .path()
            .join("ws")
            .join(id.to_string())
            .join("missions")
            .join("guest.toml");
        assert_eq!(
            parse_contract(&fs::read_to_string(path).unwrap()).unwrap(),
            full()
        );
        assert!(
            !root.join(".swarmux").exists(),
            "nothing in the workspace root"
        );
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
