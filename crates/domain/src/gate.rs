//! The done-gate: the checks swarmux runs when an agent says it has
//! finished, what a run found, and whether the agent may stop.
//!
//! An agent's "done" is never taken on its word. When it stops, the gate
//! runs the repository's checks itself, compares protected files with the
//! hashes taken when the session started, and looks for the test runs the
//! agent claims in what it actually executed. Anything wrong blocks the stop
//! with a short digest, up to a limit; after that the user decides.

use std::collections::{BTreeMap, BTreeSet};

use crate::guard::glob::{glob_match, is_windows, relative};
use crate::guard::shell::{collapse, split_commands, strip_wrappers};

/// Blocks before the gate gives up and asks the user.
pub const DEFAULT_MAX_BLOCKS: u32 = 3;
/// How long one check may run.
pub const DEFAULT_CHECK_TIMEOUT_SECS: u64 = 600;
/// Always protected: the gate's own configuration.
pub const ALWAYS_PROTECTED: &str = ".swarmux/**";
/// npm's placeholder test script, which always fails.
const NPM_PLACEHOLDER: &str = "no test specified";

/// Longest digest sent back to the agent, in characters.
const DIGEST_MAX: usize = 2400;
/// Lines of a failure shown from its first failing line.
const EXCERPT_LINES: usize = 8;
/// Longest line kept in a digest, in characters.
const LINE_MAX: usize = 240;

/// One command the gate runs. Run directly, never through a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateCheck {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
}

impl GateCheck {
    pub fn new(name: &str, program: &str, args: &[&str]) -> GateCheck {
        GateCheck {
            name: name.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timeout_secs: DEFAULT_CHECK_TIMEOUT_SECS,
        }
    }

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
}

/// A repository's gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateSpec {
    pub checks: Vec<GateCheck>,
    /// Stops blocked before the user is asked. 0 never blocks.
    pub max_blocks: u32,
    /// Globs relative to the repository root that agents may not write.
    pub protected: Vec<String>,
    /// Whether test runs the agent claims must appear in what it ran.
    pub evidence: bool,
    /// Whether subagents' stops run the checks too (otherwise only the
    /// protected files and the evidence are checked).
    pub subagent_checks: bool,
}

impl Default for GateSpec {
    fn default() -> GateSpec {
        GateSpec {
            checks: Vec::new(),
            max_blocks: DEFAULT_MAX_BLOCKS,
            protected: Vec::new(),
            evidence: true,
            subagent_checks: false,
        }
    }
}

impl GateSpec {
    /// The configured protected globs plus [`ALWAYS_PROTECTED`].
    pub fn protected_globs(&self) -> Vec<String> {
        let mut all = vec![ALWAYS_PROTECTED.to_string()];
        for g in &self.protected {
            if !all.contains(g) {
                all.push(g.clone());
            }
        }
        all
    }
}

/// Where a gate's checks came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateOrigin {
    /// `.swarmux/gates.toml`.
    File,
    /// Guessed from the files at the repository root.
    Detected,
}

impl GateOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            GateOrigin::File => "gates.toml",
            GateOrigin::Detected => "detected",
        }
    }
}

/// What the gate needs to know about a repository to guess its checks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoFacts {
    /// Names of the files at the root.
    pub files: Vec<String>,
    /// `scripts.test` of `package.json`, if any.
    pub npm_test: Option<String>,
    /// `pyproject.toml`, `setup.cfg` or `tox.ini` configures pytest.
    pub pytest_configured: bool,
}

impl RepoFacts {
    fn has(&self, name: &str) -> bool {
        self.files.iter().any(|f| f == name)
    }
}

/// The checks a repository most likely uses: `cargo test`, `go test`,
/// `pnpm test` or `npm test` (when a real test script exists) and `pytest`.
pub fn detect(repo: &RepoFacts) -> Vec<GateCheck> {
    let mut out = Vec::new();
    if repo.has("Cargo.toml") {
        out.push(GateCheck::new(
            "cargo test",
            "cargo",
            &["test", "--workspace", "--quiet"],
        ));
    }
    if repo.has("go.mod") {
        out.push(GateCheck::new("go test", "go", &["test", "./..."]));
    }
    let real_script = repo
        .npm_test
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty() && !s.contains(NPM_PLACEHOLDER));
    if repo.has("package.json") && real_script {
        if repo.has("pnpm-lock.yaml") {
            out.push(GateCheck::new("pnpm test", "pnpm", &["test"]));
        } else {
            out.push(GateCheck::new("npm test", "npm", &["test"]));
        }
    }
    if repo.has("pytest.ini") || repo.has("conftest.py") || repo.pytest_configured {
        out.push(GateCheck::new("pytest", "pytest", &["-q"]));
    }
    out
}

/// One run of one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub command: String,
    /// `None` when it was killed or never started.
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// Why it couldn't start.
    pub spawn_error: Option<String>,
    pub duration_ms: u64,
    /// The end of stdout and stderr, interleaved, cut by the runner.
    pub output: String,
    /// It failed, then passed when run again.
    pub flaky: bool,
}

impl CheckRun {
    pub fn passed(&self) -> bool {
        self.exit == Some(0) && !self.timed_out && self.spawn_error.is_none()
    }

    /// `exited 101 after 4.2 s`, `timed out after 600 s`, …
    pub fn status_text(&self) -> String {
        let secs = self.duration_ms as f64 / 1000.0;
        if let Some(e) = &self.spawn_error {
            return format!("could not start: {e}");
        }
        if self.timed_out {
            return format!("timed out after {secs:.0} s");
        }
        match self.exit {
            Some(0) if self.flaky => format!("passed on the second try after {secs:.1} s"),
            Some(0) => format!("passed in {secs:.1} s"),
            Some(code) => format!("exited {code} after {secs:.1} s"),
            None => format!("was killed after {secs:.1} s"),
        }
    }

    /// The first line of the output that says what failed, verbatim, and
    /// the lines after it.
    pub fn failure_excerpt(&self) -> Vec<String> {
        let lines: Vec<&str> = self.output.lines().collect();
        let start = first_failure(&lines);
        let Some(start) = start else {
            let mut tail: Vec<String> = lines
                .iter()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(EXCERPT_LINES)
                .map(|l| cut(l, LINE_MAX))
                .collect();
            tail.reverse();
            return tail;
        };
        lines[start..]
            .iter()
            .take(EXCERPT_LINES)
            .map(|l| cut(l, LINE_MAX))
            .collect()
    }
}

/// Markers of the line that says why a test failed, best first.
const STRONG_MARKERS: [&str; 9] = [
    "panicked at",
    "AssertionError",
    "assertion",
    "--- FAIL",
    "Error:",
    "error[",
    "Traceback",
    "expected",
    "✗",
];
/// Markers of a line that only says that something failed.
const WEAK_MARKERS: [&str; 4] = ["FAILED", "FAIL", "failed", "error"];

fn first_failure(lines: &[&str]) -> Option<usize> {
    let find = |markers: &[&str]| {
        lines
            .iter()
            .position(|l| markers.iter().any(|m| l.contains(m)))
    };
    find(&STRONG_MARKERS).or_else(|| find(&WEAK_MARKERS))
}

/// A protected file's content hash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pin {
    /// Relative to the repository root, `/`-separated.
    pub path: String,
    /// SHA-256, hex.
    pub sha256: String,
}

/// How a protected file differs from its pin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinChange {
    Changed(String),
    Removed(String),
    Added(String),
}

impl PinChange {
    pub fn path(&self) -> &str {
        match self {
            PinChange::Changed(p) | PinChange::Removed(p) | PinChange::Added(p) => p,
        }
    }

    pub fn verb(&self) -> &'static str {
        match self {
            PinChange::Changed(_) => "changed",
            PinChange::Removed(_) => "removed",
            PinChange::Added(_) => "added",
        }
    }
}

/// What changed between the pins taken at the start and now, by path.
pub fn pin_changes(before: &[Pin], after: &[Pin]) -> Vec<PinChange> {
    let old: BTreeMap<&str, &str> = before
        .iter()
        .map(|p| (p.path.as_str(), p.sha256.as_str()))
        .collect();
    let new: BTreeMap<&str, &str> = after
        .iter()
        .map(|p| (p.path.as_str(), p.sha256.as_str()))
        .collect();
    let mut out = Vec::new();
    for (path, hash) in &old {
        match new.get(path) {
            None => out.push(PinChange::Removed(path.to_string())),
            Some(h) if h != hash => out.push(PinChange::Changed(path.to_string())),
            Some(_) => {}
        }
    }
    for path in new.keys().filter(|p| !old.contains_key(*p)) {
        out.push(PinChange::Added(path.to_string()));
    }
    out.sort();
    out
}

/// A protected glob as matched against root-relative paths: gitignore-like,
/// a bare name matches at any depth and a trailing `/` means everything
/// below.
fn anchored(glob: &str) -> String {
    let g = glob.trim().replace('\\', "/");
    let g = g.strip_prefix("./").unwrap_or(&g);
    let g = g.trim_start_matches('/');
    if let Some(dir) = g.strip_suffix('/') {
        return format!("{dir}/**");
    }
    if g.contains('/') {
        g.to_string()
    } else {
        format!("**/{g}")
    }
}

/// The folders, relative to the root, below which `globs` can match: the
/// literal start of each glob, leaving out folders inside another. `""` is
/// the root itself.
pub fn search_roots(globs: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = globs
        .iter()
        .map(|g| {
            let a = anchored(g);
            let segs: Vec<&str> = a.split('/').collect();
            let literal: Vec<&str> = segs[..segs.len().saturating_sub(1)]
                .iter()
                .take_while(|s| !s.contains(['*', '?', '[']))
                .copied()
                .collect();
            literal.join("/")
        })
        .collect();
    roots.sort();
    roots.dedup();
    let all = roots.clone();
    roots.retain(|r| {
        !all.iter()
            .any(|o| o != r && (o.is_empty() || r.starts_with(&format!("{o}/"))))
    });
    roots
}

/// Whether `rel` (relative to the root, `/`-separated) is protected; the
/// glob that protects it.
pub fn protected_glob<'a>(globs: &'a [String], rel: &str, fold: bool) -> Option<&'a str> {
    globs
        .iter()
        .find(|g| glob_match(&anchored(g), rel, fold))
        .map(String::as_str)
}

/// Whether `path` (absolute, or relative to `cwd`) lies in a protected part
/// of the repository at `root`; the glob that protects it.
pub fn protected_path<'a>(
    globs: &'a [String],
    root: &str,
    path: &str,
    cwd: &str,
) -> Option<&'a str> {
    let abs = crate::guard::glob::absolute(path, cwd)?;
    let rel = relative(&abs, root)?;
    let fold = is_windows(root) || is_windows(&abs);
    protected_glob(globs, &rel, fold)
}

/// Tools whose subject is a file they write.
const WRITE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
/// Programs that change every path they are given.
const WRITERS: [&str; 26] = [
    "rm",
    "rmdir",
    "mv",
    "tee",
    "truncate",
    "touch",
    "chmod",
    "chown",
    "ln",
    "dd",
    "unlink",
    "shred",
    "install",
    "patch",
    "del",
    "erase",
    "ren",
    "move",
    "remove-item",
    "move-item",
    "rename-item",
    "set-content",
    "add-content",
    "clear-content",
    "out-file",
    "new-item",
];
/// Programs that change only the last path they are given.
const COPIERS: [&str; 6] = ["cp", "copy", "copy-item", "rsync", "xcopy", "robocopy"];

/// Whether a tool call would write a protected file of the repository at
/// `root`. Returns the protecting glob.
///
/// Edits are matched by path. Shell lines are matched per command: a
/// redirect into a protected file, or a program that writes the paths it
/// is given. Reading or running protected files is never refused.
pub fn protected_write(
    globs: &[String],
    root: &str,
    tool: &str,
    subject: &str,
    cwd: &str,
) -> Option<String> {
    if WRITE_TOOLS.contains(&tool) {
        return protected_path(globs, root, subject, cwd).map(str::to_string);
    }
    if !matches!(tool, "Bash" | "PowerShell") {
        return None;
    }
    let hit = |path: &str| protected_path(globs, root, path, cwd).map(str::to_string);
    for command in split_commands(subject) {
        let words = words_of(strip_wrappers(&command));
        for target in redirect_targets(&words) {
            if let Some(g) = hit(&target) {
                return Some(g);
            }
        }
        let Some(program) = words.first().map(|w| program_name(w)) else {
            continue;
        };
        let args: Vec<&String> = words[1..]
            .iter()
            .filter(|w| !w.starts_with('-') && !w.contains('>'))
            .collect();
        let in_place = words[1..]
            .iter()
            .any(|w| w.starts_with("-i") || w == "--in-place");
        let targets: Vec<&String> = if WRITERS.contains(&program.as_str())
            || (matches!(program.as_str(), "sed" | "perl") && in_place)
        {
            args
        } else if COPIERS.contains(&program.as_str()) {
            args.last().copied().into_iter().collect()
        } else if program == "git" {
            match args.first().map(|s| s.as_str()) {
                Some("rm" | "mv" | "apply") => args[1..].to_vec(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        if let Some(g) = targets.into_iter().find_map(|t| hit(t)) {
            return Some(g);
        }
    }
    None
}

/// Words of one simple command with their quotes removed.
pub(crate) fn words_of(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Files written by `>`, `>>`, `2>` and the like.
fn redirect_targets(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut next_is_target = false;
    for w in words {
        if next_is_target {
            out.push(w.clone());
            next_is_target = false;
            continue;
        }
        let Some(at) = w.find('>') else { continue };
        let rest = &w[at..];
        if rest.contains(">&") {
            // `2>&1`: into another stream, not a file.
            continue;
        }
        let target = rest.trim_start_matches(['>', '|']);
        if target.is_empty() {
            next_is_target = true;
        } else {
            out.push(target.to_string());
        }
    }
    out
}

/// `C:\tools\rm.exe` → `rm`, `Set-Content` → `set-content`.
fn program_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let base = base.to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

/// A command the agent ran, and whether it succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executed {
    pub command: String,
    pub ok: bool,
}

/// Test runners recognised in claims and in what ran.
const RUNNERS: [&str; 12] = [
    "cargo test",
    "cargo nextest",
    "npm test",
    "npm run test",
    "pnpm test",
    "pnpm run test",
    "yarn test",
    "python -m pytest",
    "py -m pytest",
    "pytest",
    "go test",
    "dotnet test",
];
/// Phrases that claim tests or checks pass.
const CLAIMS: [&str; 14] = [
    "tests pass",
    "tests passed",
    "tests are passing",
    "tests now pass",
    "tests all pass",
    "all tests",
    "test suite passes",
    "suite passes",
    "tests succeed",
    "tests are green",
    "checks pass",
    "all green",
    "everything passes",
    "passes all",
];
/// Words that turn a claim into its opposite.
const NEGATIONS: [&str; 7] = [
    "not ",
    "n't",
    "unable",
    "cannot",
    "could not",
    "without",
    "skipp",
];

/// What the agent claims it ran or saw pass, without evidence: each claim
/// whose command never ran, or whose latest run failed.
///
/// A named runner (`cargo test`, a configured check) must have run with
/// exit 0 as its latest run; a bare "all tests pass" needs any test run
/// that succeeded last.
pub fn evidence_gaps(message: &str, executed: &[Executed], checks: &[GateCheck]) -> Vec<String> {
    let mut known: Vec<String> = RUNNERS.iter().map(|r| r.to_string()).collect();
    for c in checks {
        let line = collapse(&c.command_line().to_ascii_lowercase());
        if !known.contains(&line) {
            known.push(line);
        }
    }
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut generic: Option<String> = None;
    for sentence in message.split(['\n', '.', '!', '?', ';']) {
        let low = sentence.to_ascii_lowercase();
        if NEGATIONS.iter().any(|n| low.contains(n)) {
            continue;
        }
        let mentioned: Vec<&String> = known.iter().filter(|k| mentions(&low, k)).collect();
        for k in &mentioned {
            // `cargo test --quiet` named: `cargo test` is what must have run.
            if !mentioned
                .iter()
                .any(|o| o.len() < k.len() && k.starts_with(o.as_str()))
            {
                named.insert((*k).clone());
            }
        }
        if generic.is_none() && CLAIMS.iter().any(|c| low.contains(c)) {
            generic = Some(sentence.trim().to_string());
        }
    }
    let latest = |runner: &str| {
        executed
            .iter()
            .rev()
            .find(|e| runs(&e.command, runner))
            .map(|e| e.ok)
    };
    let mut gaps = Vec::new();
    for runner in &named {
        match latest(runner) {
            Some(true) => {}
            Some(false) => gaps.push(format!(
                "you cite `{runner}`, but its latest run in this session failed"
            )),
            None => gaps.push(format!(
                "you cite `{runner}`, but it never ran in this session"
            )),
        }
    }
    if let Some(claim) = generic.filter(|_| named.is_empty()) {
        let last_test = executed
            .iter()
            .rev()
            .find(|e| known.iter().any(|k| runs(&e.command, k)));
        if !last_test.is_some_and(|e| e.ok) {
            gaps.push(format!(
                "you said \"{}\", but no test run succeeded in this session",
                cut(&claim, 120)
            ));
        }
    }
    gaps
}

/// Whether `text` names `command` as words of their own (`go test` is not
/// in `cargo test`).
fn mentions(text: &str, command: &str) -> bool {
    text.match_indices(command).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + command.len()..].chars().next();
        let free = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        free(before) && free(after)
    })
}

/// Whether the shell line `command` runs `runner` in one of its commands.
fn runs(command: &str, runner: &str) -> bool {
    split_commands(&command.to_ascii_lowercase())
        .iter()
        .any(|c| {
            let c = strip_wrappers(c);
            c == runner
                || c.strip_prefix(runner)
                    .is_some_and(|rest| rest.starts_with(' '))
        })
}

/// Everything one stop found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GateFindings {
    pub runs: Vec<CheckRun>,
    pub pins: Vec<PinChange>,
    pub gaps: Vec<String>,
}

impl GateFindings {
    /// Nothing wrong.
    pub fn clean(&self) -> bool {
        self.runs.iter().all(CheckRun::passed) && self.pins.is_empty() && self.gaps.is_empty()
    }

    pub fn failed(&self) -> impl Iterator<Item = &CheckRun> {
        self.runs.iter().filter(|r| !r.passed())
    }
}

/// What the gate does with a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateOutcome {
    /// Let the agent stop.
    Pass,
    /// Make it continue with the digest.
    Block,
    /// Still wrong after the last allowed block: let it stop and ask the user.
    Escalate,
}

impl GateOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            GateOutcome::Pass => "pass",
            GateOutcome::Block => "block",
            GateOutcome::Escalate => "escalate",
        }
    }
}

/// The outcome of a stop after `blocks` earlier blocks.
pub fn judge(findings: &GateFindings, blocks: u32, max_blocks: u32) -> GateOutcome {
    if findings.clean() {
        GateOutcome::Pass
    } else if blocks < max_blocks {
        GateOutcome::Block
    } else {
        GateOutcome::Escalate
    }
}

/// What the agent (on a block) or the user (on an escalation) is told: the
/// failing checks with their first failing lines verbatim, the protected
/// files that changed and the claims nothing backs.
pub fn digest(
    findings: &GateFindings,
    outcome: GateOutcome,
    blocks: u32,
    max_blocks: u32,
    protected: &[String],
) -> String {
    let mut out = match outcome {
        GateOutcome::Pass => {
            let n = findings.runs.len();
            return format!(
                "swarmux gate: {n} check{} passed",
                if n == 1 { "" } else { "s" }
            );
        }
        GateOutcome::Block => format!(
            "swarmux gate: not done yet (block {} of {max_blocks}). \
             Fix what follows, then finish again.\n",
            blocks + 1
        ),
        GateOutcome::Escalate => {
            format!("swarmux gate: still failing after {max_blocks} blocks.\n")
        }
    };
    for run in findings.failed() {
        out.push_str(&format!(
            "✗ {}: `{}` {}\n",
            run.name,
            run.command,
            run.status_text()
        ));
        if run.spawn_error.is_none() {
            for line in run.failure_excerpt() {
                out.push_str("    ");
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    for change in &findings.pins {
        out.push_str(&format!(
            "⛔ protected file {}: {}. Put it back as it was; protected files are not yours to change.\n",
            change.verb(),
            change.path()
        ));
    }
    for gap in &findings.gaps {
        out.push_str(&format!("⚑ {gap}. Run it and report what it printed.\n"));
    }
    if outcome == GateOutcome::Block && !protected.is_empty() {
        out.push_str(&format!(
            "Protected (writes are refused): {}\n",
            protected.join(", ")
        ));
    }
    cut(out.trim_end(), DIGEST_MAX)
}

/// `s` cut to `max` characters, with `…` when cut.
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests;
