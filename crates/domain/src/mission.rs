//! Missions: an outcome the user wants, the evidence that proves it, the
//! constraints the work stays inside and what it may cost.
//!
//! A mission is linted before it runs (is it checkable? are its oracles
//! safe from the agent?), compiled into one job plus the done-gate checks
//! that judge it, and followed through a fixed set of states. It is never
//! simply "done": it is delivered only after the gate passed, and
//! otherwise escalated or refused.

use std::fmt;

use crate::gate::{GateCheck, GateOutcome, protected_glob, words_of};
use crate::workspace::split_check;
use crate::{JobId, Millis, SessionId, WorkspaceId};

mod landing;
pub use landing::{LandState, Landing, RepoLanding};

/// Where a repository keeps its mission contracts, relative to its root.
/// swarmux writes them; agents can't (the gate's own folder is protected).
pub const MISSIONS_DIR: &str = ".swarmux/missions";
/// Most clarifying questions a lint asks.
pub const MAX_QUESTIONS: usize = 3;
/// A goal shorter than this many words gets a question.
const VAGUE_WORDS: usize = 4;
/// Longest gate check name, in characters.
const CHECK_NAME_MAX: usize = 32;

/// What proves the outcome.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Acceptance {
    /// Command lines the gate runs (no shell); all must pass. A line
    /// `"app: flutter test"` runs in the repository `app` of a workspace
    /// mission (or the subfolder `app` of a repository's).
    pub checks: Vec<String>,
    /// A person signs off as well, after the checks.
    pub human_signoff: bool,
}

/// Where the work may and may not go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Constraints {
    /// Globs the agent is meant to work in; empty is anywhere.
    pub paths_rw: Vec<String>,
    /// Globs the agent may never write: the oracles above all.
    pub protected: Vec<String>,
}

/// What happens when the budget runs out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnExhaust {
    /// Stop the job and ask the user.
    #[default]
    Escalate,
    /// Let the job finish, then ask.
    Warn,
}

impl OnExhaust {
    pub fn as_str(self) -> &'static str {
        match self {
            OnExhaust::Escalate => "escalate",
            OnExhaust::Warn => "warn",
        }
    }

    pub fn parse(s: &str) -> Option<OnExhaust> {
        match s {
            "escalate" => Some(OnExhaust::Escalate),
            "warn" => Some(OnExhaust::Warn),
            _ => None,
        }
    }
}

/// What the mission may cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Budget {
    pub usd: Option<f64>,
    pub wall_minutes: Option<u32>,
    pub on_exhaust: OnExhaust,
}

/// A question the lint asked and the user's answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clarification {
    pub question: String,
    /// Empty until answered.
    pub answer: String,
}

/// The outcome contract, as the user writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contract {
    pub title: String,
    pub goal: String,
    /// The repositories of a workspace mission it touches, producers
    /// first: each gets a worktree and they land in this order. Empty
    /// means every repository of the workspace.
    pub repos: Vec<String>,
    pub acceptance: Acceptance,
    pub constraints: Constraints,
    pub budget: Budget,
    /// When the user needs it by, in unix milliseconds (written in the
    /// contract as an RFC 3339 timestamp). `None` means no due time: nothing
    /// is shown, ranked or linted about one. Never worked out for the user.
    pub due: Option<Millis>,
    pub clarifications: Vec<Clarification>,
}

/// Where a mission stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MissionState {
    /// Written, not linted since the last edit.
    #[default]
    Draft,
    Linted,
    /// Compiled into a job that waits its turn.
    Queued,
    Running,
    /// The job finished; the gate's verdict is awaited.
    Verifying,
    /// The gate passed.
    Delivered,
    /// The user decides: the gate gave up, the job failed or the budget ran out.
    Escalated,
    /// It cannot run as written (nothing can check it, or it contradicts itself).
    Refused,
}

impl MissionState {
    pub const ALL: [MissionState; 8] = [
        MissionState::Draft,
        MissionState::Linted,
        MissionState::Queued,
        MissionState::Running,
        MissionState::Verifying,
        MissionState::Delivered,
        MissionState::Escalated,
        MissionState::Refused,
    ];

    /// The way a mission goes when all is well.
    pub const PATH: [MissionState; 6] = [
        MissionState::Draft,
        MissionState::Linted,
        MissionState::Queued,
        MissionState::Running,
        MissionState::Verifying,
        MissionState::Delivered,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MissionState::Draft => "draft",
            MissionState::Linted => "linted",
            MissionState::Queued => "queued",
            MissionState::Running => "running",
            MissionState::Verifying => "verifying",
            MissionState::Delivered => "delivered",
            MissionState::Escalated => "escalated",
            MissionState::Refused => "refused",
        }
    }

    pub fn parse(s: &str) -> Option<MissionState> {
        MissionState::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// A job is out for it.
    pub fn in_flight(self) -> bool {
        matches!(
            self,
            MissionState::Queued | MissionState::Running | MissionState::Verifying
        )
    }

    /// Nothing more happens without the user.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            MissionState::Delivered | MissionState::Escalated | MissionState::Refused
        )
    }
}

impl fmt::Display for MissionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What can happen to a mission.
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// The contract changed.
    Edited,
    /// A lint finished; `blocking` when it found something that stops a run.
    Linted { blocking: bool },
    /// It was compiled into `job`.
    Queued(JobId),
    /// The job started.
    Started,
    /// The job ended, well or not.
    Finished { ok: bool },
    /// A gate verdict on the job.
    Gate(GateOutcome),
    /// The user sent the work back with a note: it carries on as `job`.
    SentBack(JobId),
    /// The budget ran out.
    Exhausted(String),
    /// It cannot run as written.
    Refuse(String),
}

impl Signal {
    pub fn name(&self) -> &'static str {
        match self {
            Signal::Edited => "edited",
            Signal::Linted { .. } => "linted",
            Signal::Queued(_) => "queued",
            Signal::Started => "started",
            Signal::Finished { .. } => "finished",
            Signal::Gate(_) => "gate",
            Signal::SentBack(_) => "sent back",
            Signal::Exhausted(_) => "exhausted",
            Signal::Refuse(_) => "refuse",
        }
    }
}

/// A signal the mission's state has no way to take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadTransition {
    pub from: MissionState,
    pub signal: &'static str,
}

impl fmt::Display for BadTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {} mission can't be {}", self.from, self.signal)
    }
}

/// The state `signal` takes `from` to.
///
/// The only way into [`MissionState::Delivered`] is a passing gate
/// verdict while verifying. A blocked stop changes nothing: the agent
/// carries on.
pub fn step(from: MissionState, signal: &Signal) -> Result<MissionState, BadTransition> {
    use MissionState as S;
    let to = match (from, signal) {
        (S::Draft | S::Linted | S::Escalated | S::Refused | S::Delivered, Signal::Edited) => {
            S::Draft
        }
        (S::Draft | S::Linted, Signal::Linted { .. }) => S::Linted,
        (S::Linted, Signal::Queued(_)) => S::Queued,
        (S::Draft | S::Linted, Signal::Refuse(_)) => S::Refused,
        (S::Queued, Signal::Started) => S::Running,
        (S::Running, Signal::Started) => S::Running,
        // The user read the work and sent it back: the agent carries on in a
        // new job, and the gate judges it again at its next claim.
        (S::Escalated | S::Verifying, Signal::SentBack(_)) => S::Queued,
        (S::Queued | S::Running, Signal::Finished { ok: true }) => S::Verifying,
        (S::Queued | S::Running | S::Verifying, Signal::Finished { ok: false }) => S::Escalated,
        (S::Verifying, Signal::Finished { ok: true }) => S::Verifying,
        (S::Verifying, Signal::Gate(GateOutcome::Pass)) => S::Delivered,
        (S::Running | S::Verifying, Signal::Gate(GateOutcome::Escalate)) => S::Escalated,
        (S::Running | S::Verifying, Signal::Gate(_)) => from,
        (S::Queued | S::Running | S::Verifying, Signal::Exhausted(_)) => S::Escalated,
        _ => {
            return Err(BadTransition {
                from,
                signal: signal.name(),
            });
        }
    };
    Ok(to)
}

/// How sure a lint finding is that something is wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The mission can't run until it is fixed.
    Blocking,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Blocking => "blocking",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        match s {
            "blocking" => Some(Severity::Blocking),
            "warning" => Some(Severity::Warning),
            "info" => Some(Severity::Info),
            _ => None,
        }
    }
}

/// What a lint finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// Nothing can check the outcome.
    MissingAcceptance,
    /// A file the checks judge by is writable by the agent.
    UnprotectedOracle,
    /// A path is both writable and protected.
    Conflict,
    /// The goal is too short to act on.
    VagueGoal,
    NoBudget,
    /// No paths to work in were named.
    Unscoped,
    /// A repository the workspace does not have, or repositories named
    /// outside a workspace.
    UnknownRepo,
    /// The due time has already passed.
    PastDue,
}

impl FindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::MissingAcceptance => "missing_acceptance",
            FindingKind::UnprotectedOracle => "unprotected_oracle",
            FindingKind::Conflict => "conflict",
            FindingKind::VagueGoal => "vague_goal",
            FindingKind::NoBudget => "no_budget",
            FindingKind::Unscoped => "unscoped",
            FindingKind::UnknownRepo => "unknown_repo",
            FindingKind::PastDue => "past_due",
        }
    }

    pub fn parse(s: &str) -> Option<FindingKind> {
        [
            FindingKind::MissingAcceptance,
            FindingKind::UnprotectedOracle,
            FindingKind::Conflict,
            FindingKind::VagueGoal,
            FindingKind::NoBudget,
            FindingKind::Unscoped,
            FindingKind::UnknownRepo,
            FindingKind::PastDue,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

/// One thing the lint found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    pub detail: String,
    /// A glob that, once protected, settles it.
    pub protect: Option<String>,
}

/// A question that would make the mission clearer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    pub question: String,
    /// Why it matters, one line.
    pub why: String,
}

/// What linting a contract found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LintReport {
    pub findings: Vec<Finding>,
    /// At most [`MAX_QUESTIONS`].
    pub questions: Vec<Question>,
    /// Who asked the questions: `local`, or the model that did.
    pub source: String,
    /// Why the model could not be asked, when it couldn't.
    pub problem: Option<String>,
    pub cost_usd: f64,
}

impl LintReport {
    pub fn blocking(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == Severity::Blocking)
    }

    /// The questions, the model's first, cut to [`MAX_QUESTIONS`] and
    /// without repeats.
    pub fn with_questions(mut self, first: Vec<Question>) -> LintReport {
        let mut all = first;
        all.append(&mut self.questions);
        let mut seen: Vec<String> = Vec::new();
        all.retain(|q| {
            let key = q.question.trim().to_lowercase();
            let fresh = !key.is_empty() && !seen.contains(&key);
            seen.push(key);
            fresh
        });
        all.truncate(MAX_QUESTIONS);
        self.questions = all;
        self
    }
}

/// Test runners and the files they judge by, relative to the root.
const ORACLES: [(&str, &[&str]); 9] = [
    ("cargo test", &["tests/**"]),
    ("cargo nextest", &["tests/**"]),
    ("pytest", &["tests/**", "test_*.py"]),
    ("python -m pytest", &["tests/**", "test_*.py"]),
    ("go test", &["*_test.go"]),
    ("npm test", &["*.test.*", "__tests__/**"]),
    ("pnpm test", &["*.test.*", "__tests__/**"]),
    ("yarn test", &["*.test.*", "__tests__/**"]),
    ("dotnet test", &["*Tests/**"]),
];
/// File endings of scripts and data a check may name as its oracle.
const ORACLE_FILES: [&str; 10] = [
    ".sh", ".ps1", ".py", ".rs", ".js", ".ts", ".json", ".toml", ".txt", ".snap",
];

/// The files the acceptance checks judge by: what each runner reads its
/// tests from, and any script or data file a check names.
pub fn oracles(checks: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |g: &str| {
        if !out.iter().any(|o| o == g) {
            out.push(g.to_string());
        }
    };
    for check in checks {
        // A check run in a repository judges by that repository's files.
        let (dir, command) = split_check(check);
        let at = |g: &str| match dir {
            Some(d) => format!("{}/{g}", d.trim_end_matches('/')),
            None => g.to_string(),
        };
        let words = words_of(command);
        let line = words.join(" ").to_ascii_lowercase();
        for (runner, globs) in ORACLES {
            if line == runner || line.starts_with(&format!("{runner} ")) {
                globs.iter().for_each(|g| add(&at(g)));
            }
        }
        for w in words.iter().skip(1).filter(|w| !w.starts_with('-')) {
            let w = w.trim_start_matches("./").replace('\\', "/");
            let named_file = ORACLE_FILES.iter().any(|e| w.ends_with(e));
            if named_file || w.starts_with("tests/") {
                add(&at(&w));
            }
        }
    }
    out
}

/// A path `glob` matches, to test it against other globs: `**/` and
/// wildcards stand for a short name.
fn sample(glob: &str) -> String {
    glob.replace("**/", "")
        .replace("**", "x")
        .replace(['*', '?'], "x")
}

/// Whether `glob` (or the paths it stands for) is protected by `protected`.
fn covered(protected: &[String], glob: &str) -> bool {
    protected.iter().any(|p| p == glob) || protected_glob(protected, &sample(glob), false).is_some()
}

/// What a deterministic lint finds: no checks, oracles the agent could
/// edit, paths both writable and protected, a goal too short to act on,
/// no budget and no scope.
pub fn lint(contract: &Contract) -> Vec<Finding> {
    let mut out = Vec::new();
    let checks: Vec<&String> = contract
        .acceptance
        .checks
        .iter()
        .filter(|c| !c.trim().is_empty())
        .collect();
    if checks.is_empty() {
        out.push(Finding {
            kind: FindingKind::MissingAcceptance,
            severity: Severity::Blocking,
            detail: "no acceptance checks: nothing can tell that it is done".into(),
            protect: None,
        });
    }
    let protected = &contract.constraints.protected;
    let checks: Vec<String> = checks.into_iter().cloned().collect();
    for oracle in oracles(&checks) {
        if !covered(protected, &oracle) {
            out.push(Finding {
                kind: FindingKind::UnprotectedOracle,
                severity: Severity::Warning,
                detail: format!(
                    "the checks judge by {oracle}, which is not protected: the agent could edit it to pass"
                ),
                protect: Some(oracle),
            });
        }
    }
    for rw in &contract.constraints.paths_rw {
        if covered(protected, rw) {
            out.push(Finding {
                kind: FindingKind::Conflict,
                severity: Severity::Blocking,
                detail: format!("{rw} is both writable and protected"),
                protect: None,
            });
        }
    }
    if contract.goal.split_whitespace().count() < VAGUE_WORDS {
        out.push(Finding {
            kind: FindingKind::VagueGoal,
            severity: Severity::Warning,
            detail: "the goal is a few words: say what should be different when it is done".into(),
            protect: None,
        });
    }
    if contract.budget.usd.is_none() && contract.budget.wall_minutes.is_none() {
        out.push(Finding {
            kind: FindingKind::NoBudget,
            severity: Severity::Info,
            detail: "no budget: it may spend until the checks pass".into(),
            protect: None,
        });
    }
    if contract.constraints.paths_rw.is_empty() {
        out.push(Finding {
            kind: FindingKind::Unscoped,
            severity: Severity::Info,
            detail: "no paths named: anything outside the protected ones may change".into(),
            protect: None,
        });
    }
    out
}

/// The finding for a due time that has already passed, when the contract
/// has one. The lint takes the time as an argument, since the domain reads
/// no clock.
pub fn lint_due(contract: &Contract, now: Millis) -> Vec<Finding> {
    match contract.due {
        Some(due) if due <= now => vec![Finding {
            kind: FindingKind::PastDue,
            severity: Severity::Warning,
            detail: format!(
                "due {} has already passed: say when it is needed, or remove the due time",
                crate::date::iso_minute(due)
            ),
            protect: None,
        }],
        _ => Vec::new(),
    }
}

/// What a mission names that its workspace does not have: repositories
/// in `repos` and in the checks' `repo:` prefixes. Outside a workspace
/// (`known` is `None`) naming repositories is an error, while a check's
/// prefix is a subfolder of the repository and is left alone.
pub fn lint_repos(contract: &Contract, known: Option<&[String]>) -> Vec<Finding> {
    let mut out = Vec::new();
    let finding = |detail: String| Finding {
        kind: FindingKind::UnknownRepo,
        severity: Severity::Blocking,
        detail,
        protect: None,
    };
    let Some(known) = known else {
        if !contract.repos.is_empty() {
            out.push(finding(format!(
                "repos = [{}] names repositories, but the mission is not in a workspace",
                contract.repos.join(", ")
            )));
        }
        return out;
    };
    let list = if known.is_empty() {
        "none".to_string()
    } else {
        known.join(", ")
    };
    let mut seen: Vec<&str> = Vec::new();
    for repo in contract.repos.iter().map(|r| r.trim()) {
        if !known.iter().any(|k| k == repo) {
            out.push(finding(format!(
                "repos names {repo:?}, which the workspace does not have (it has {list})"
            )));
        } else if seen.contains(&repo) {
            out.push(finding(format!("repos names {repo:?} twice")));
        }
        seen.push(repo);
    }
    for check in &contract.acceptance.checks {
        if let (Some(repo), _) = split_check(check)
            && !known.iter().any(|k| k == repo)
        {
            out.push(finding(format!(
                "the check {check:?} runs in {repo:?}, which the workspace does not have (it has {list})"
            )));
        }
    }
    out
}

/// Questions the findings raise, for when no model is asked.
pub fn local_questions(findings: &[Finding]) -> Vec<Question> {
    findings
        .iter()
        .filter_map(|f| match f.kind {
            FindingKind::MissingAcceptance => Some(Question {
                question: "Which command shows it is done (a test, a script, a build)?".into(),
                why: "the gate needs something it can run".into(),
            }),
            FindingKind::VagueGoal => Some(Question {
                question: "What should be different when this is done?".into(),
                why: "the goal is too short to act on".into(),
            }),
            _ => None,
        })
        .take(MAX_QUESTIONS)
        .collect()
}

/// The lint of a contract with no model asked.
pub fn lint_report(contract: &Contract) -> LintReport {
    let findings = lint(contract);
    LintReport {
        questions: local_questions(&findings),
        findings,
        source: "local".into(),
        ..LintReport::default()
    }
}

/// Applies every fix the findings suggest: protects the oracles.
pub fn apply_fixes(contract: &mut Contract, findings: &[Finding]) -> usize {
    let mut n = 0;
    for glob in findings.iter().filter_map(|f| f.protect.as_ref()) {
        if !contract.constraints.protected.contains(glob) {
            contract.constraints.protected.push(glob.clone());
            n += 1;
        }
    }
    n
}

/// The done-gate a mission's job runs under, on top of the repository's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionGate {
    pub checks: Vec<GateCheck>,
    pub protected: Vec<String>,
}

/// A check from a command line, run without a shell.
///
/// A `"repo: ..."` line runs in the folder `repo` below the gate's root,
/// and its name keeps the prefix, so checks of different repositories
/// stay apart.
pub fn check_of(line: &str) -> Option<GateCheck> {
    check_in(line, &|d: &str| Some(d.to_string()))
}

/// Like [`check_of`], with the `repo:` prefix turned into the folder the
/// check runs in by `folder` (a workspace repository's name into its
/// path). `None` when `folder` does not know the prefix.
pub fn check_in(line: &str, folder: &dyn Fn(&str) -> Option<String>) -> Option<GateCheck> {
    let (dir, command) = split_check(line);
    let words = words_of(command);
    let (program, args) = words.split_first()?;
    let shown = match dir {
        Some(d) => format!("{d}: {}", words.join(" ")),
        None => words.join(" "),
    };
    let name: String = shown.chars().take(CHECK_NAME_MAX).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let check = GateCheck::new(&name, program, &args);
    match dir {
        Some(d) => Some(check.in_dir(&folder(d)?)),
        None => Some(check),
    }
}

/// A mission compiled: what the job is told and how it is judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub prompt: String,
    pub gate: MissionGate,
}

/// Compiles a contract into a job's prompt and gate. The blocking findings
/// when it can't run as written.
pub fn compile(contract: &Contract) -> Result<Compiled, Vec<Finding>> {
    compile_in(contract, &|d: &str| Some(d.to_string()))
}

/// Like [`compile`], with the checks' `repo:` prefixes turned into folders
/// by `folder` (see [`check_in`]).
pub fn compile_in(
    contract: &Contract,
    folder: &dyn Fn(&str) -> Option<String>,
) -> Result<Compiled, Vec<Finding>> {
    let blocking: Vec<Finding> = lint(contract)
        .into_iter()
        .filter(|f| f.severity == Severity::Blocking)
        .collect();
    if !blocking.is_empty() {
        return Err(blocking);
    }
    let checks: Vec<GateCheck> = contract
        .acceptance
        .checks
        .iter()
        .filter_map(|c| check_in(c, folder))
        .collect();
    let c = contract;
    let mut p = String::new();
    p.push_str(c.goal.trim());
    p.push_str("\n\nDone means every one of these passes. swarmux runs them itself when you stop, and you may not stop until they do:\n");
    for check in &checks {
        match &check.dir {
            Some(dir) => p.push_str(&format!("- `{}` in `{dir}/`\n", check.command_line())),
            None => p.push_str(&format!("- `{}`\n", check.command_line())),
        }
    }
    if !c.repos.is_empty() {
        p.push_str(&format!(
            "\nRepositories to change, producers first: {}\n",
            c.repos.join(", ")
        ));
    }
    if !c.constraints.paths_rw.is_empty() {
        p.push_str(&format!(
            "\nWork in: {}\n",
            c.constraints.paths_rw.join(", ")
        ));
    }
    let mut protected = c.constraints.protected.clone();
    if !protected.iter().any(|g| g == crate::gate::ALWAYS_PROTECTED) {
        protected.push(crate::gate::ALWAYS_PROTECTED.to_string());
    }
    p.push_str(&format!(
        "\nNever change these (writes are refused): {}\n",
        protected.join(", ")
    ));
    p.push('\n');
    p.push_str(crate::verifier::ANTI_EXPLOIT_PREAMBLE);
    p.push('\n');
    let answered: Vec<&Clarification> = c
        .clarifications
        .iter()
        .filter(|q| !q.answer.trim().is_empty())
        .collect();
    if !answered.is_empty() {
        p.push_str("\nWhat the user said when asked:\n");
        for q in answered {
            p.push_str(&format!("- {} {}\n", q.question.trim(), q.answer.trim()));
        }
    }
    if let Some(usd) = c.budget.usd {
        p.push_str(&format!(
            "\nBudget: about ${usd:.2}. Spend it on the work, not on exploring.\n"
        ));
    }
    Ok(Compiled {
        prompt: p.trim_end().to_string(),
        gate: MissionGate {
            checks,
            protected: c.constraints.protected.clone(),
        },
    })
}

/// The phases a mission's budget is split into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionPhase {
    Plan,
    Implement,
    Verify,
    Review,
}

impl MissionPhase {
    /// Each phase's share of the budget. Verification has its own share,
    /// which the implementation can't spend.
    pub const SHARES: [(MissionPhase, f64); 4] = [
        (MissionPhase::Plan, 0.10),
        (MissionPhase::Implement, 0.60),
        (MissionPhase::Verify, 0.20),
        (MissionPhase::Review, 0.10),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MissionPhase::Plan => "plan",
            MissionPhase::Implement => "implement",
            MissionPhase::Verify => "verify",
            MissionPhase::Review => "review",
        }
    }
}

/// What a mission spent, by phase, in dollars.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spend {
    pub plan: f64,
    pub implement: f64,
    pub verify: f64,
    pub review: f64,
}

impl Spend {
    pub fn total(&self) -> f64 {
        self.plan + self.implement + self.verify + self.review
    }

    pub fn of(&self, phase: MissionPhase) -> f64 {
        match phase {
            MissionPhase::Plan => self.plan,
            MissionPhase::Implement => self.implement,
            MissionPhase::Verify => self.verify,
            MissionPhase::Review => self.review,
        }
    }
}

/// One mission and where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Mission {
    /// Unique; names its file and branch.
    pub slug: String,
    /// The folder it runs in (a session's).
    pub root: String,
    pub session: Option<SessionId>,
    /// The workspace it belongs to: it runs across that workspace's
    /// repositories and lands in each of them.
    pub workspace: Option<WorkspaceId>,
    /// How its repositories landed, once its gate passed.
    pub landing: Option<Landing>,
    pub contract: Contract,
    pub state: MissionState,
    /// The job it was compiled into, once queued.
    pub job: Option<JobId>,
    pub lint: Option<LintReport>,
    pub spend: Spend,
    /// Why it was escalated or refused.
    pub note: Option<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

impl Mission {
    pub fn new(slug: &str, root: &str, contract: Contract, now: Millis) -> Mission {
        Mission {
            slug: slug.to_string(),
            root: root.to_string(),
            session: None,
            workspace: None,
            landing: None,
            contract,
            state: MissionState::Draft,
            job: None,
            lint: None,
            spend: Spend::default(),
            note: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Takes `signal`; returns the state it left.
    pub fn apply(&mut self, signal: Signal, now: Millis) -> Result<MissionState, BadTransition> {
        let from = self.state;
        let to = step(from, &signal)?;
        match signal {
            Signal::Edited => {
                self.job = None;
                self.note = None;
                self.lint = None;
                self.landing = None;
            }
            Signal::Queued(job) => {
                self.job = Some(job);
                self.landing = None;
            }
            // The note that explained the escalation is answered.
            Signal::SentBack(job) => {
                self.job = Some(job);
                self.landing = None;
                self.note = None;
            }
            Signal::Exhausted(why) | Signal::Refuse(why) => self.note = Some(why),
            Signal::Finished { ok: false } => {
                self.note = Some("the job did not finish".into());
            }
            Signal::Gate(GateOutcome::Escalate) => {
                self.note = Some("the gate gave up: checks still failing".into());
            }
            _ => {}
        }
        self.state = to;
        self.updated_at = now;
        Ok(from)
    }
}

#[cfg(test)]
mod tests;
