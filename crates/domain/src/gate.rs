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
use crate::verifier::{BaseSuite, HiddenTally};

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
    /// The folder it runs in, relative to the gate's root with `/` (a
    /// repository of a workspace); the root itself when `None`.
    pub dir: Option<String>,
    /// A failing check blocks the stop. An optional one is reported and
    /// blocks nothing.
    pub required: bool,
    /// The gate level from which the check runs.
    pub level: GateLevel,
    /// Environment variables the check may see besides the built-in
    /// allowlist ([`ENV_ALLOWED`]).
    pub env: Vec<String>,
}

impl GateCheck {
    pub fn new(name: &str, program: &str, args: &[&str]) -> GateCheck {
        GateCheck {
            name: name.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timeout_secs: DEFAULT_CHECK_TIMEOUT_SECS,
            dir: None,
            required: true,
            level: GateLevel::Full,
            env: Vec::new(),
        }
    }

    /// The same check, run in `dir` below the gate's root.
    pub fn in_dir(mut self, dir: &str) -> GateCheck {
        let dir = dir.trim().trim_matches('/');
        self.dir = (!dir.is_empty() && dir != ".").then(|| dir.to_string());
        self
    }

    /// The folder it runs in below `root`.
    pub fn workdir(&self, root: &std::path::Path) -> std::path::PathBuf {
        match &self.dir {
            Some(d) => d.split('/').fold(root.to_path_buf(), |p, c| p.join(c)),
            None => root.to_path_buf(),
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

/// How far the gate goes: a check runs when the gate's level is at or
/// above the check's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GateLevel {
    /// What is quick enough to run at every stop.
    Fast,
    /// The repository's usual checks (the default).
    Full,
    /// Everything, slow checks included.
    Deep,
}

impl GateLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            GateLevel::Fast => "fast",
            GateLevel::Full => "full",
            GateLevel::Deep => "deep",
        }
    }

    pub fn parse(s: &str) -> Option<GateLevel> {
        match s.trim() {
            "fast" => Some(GateLevel::Fast),
            "full" => Some(GateLevel::Full),
            "deep" => Some(GateLevel::Deep),
            _ => None,
        }
    }
}

/// What the verifier does when the checks cannot run in a clean copy and
/// run in the worktree instead (checks that name a folder, `clean = false`,
/// a copy that could not be made).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeChecks {
    /// A mission's verdict is Escalate: nothing that ran next to the
    /// agent's files is trusted unattended. The default.
    Escalate,
    /// The verdict stands, and the bundle says `ran_in = "worktree"`.
    Allow,
}

/// What happens to new or changed dependencies in a manifest or lockfile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyChanges {
    /// Listed with the package names; the stop is not blocked. The default.
    Flag,
    /// Blocks the stop until a person looks.
    Block,
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
    pub verifier: VerifierSpec,
    /// Globs of tests the repository says may be removed or changed: the
    /// weakening detector skips them.
    pub allow_test_changes: Vec<String>,
    /// How far the gate goes: checks above this level are not run (and
    /// the record says so).
    pub level: GateLevel,
    /// Harness files besides the built-in list
    /// ([`crate::verifier::substrate::DEFAULT_SUBSTRATE`]).
    pub substrate: Vec<String>,
    /// Harness files the repository changes on purpose.
    pub allow_substrate: Vec<String>,
    /// Environment variables every check may see besides the allowlist.
    pub env: Vec<String>,
    /// Lines of `gates.toml` that were read in a way the user should know
    /// (a value that fell back to its default).
    pub warnings: Vec<String>,
}

/// How the independent verifier treats a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifierSpec {
    /// Run the checks in a clean copy of the work made by swarmux, not in
    /// the agent's worktree (when the repository is a git repository).
    pub clean: bool,
    /// Removing, changing or switching off existing tests blocks the stop.
    pub block_test_edits: bool,
    /// Run the checks again against the original versions of the protected
    /// and test files that the job changed.
    pub base_suite: bool,
    /// Revert the non-test part of the change and require a new or changed
    /// test to fail: the test proves the fix.
    pub revert_probe: bool,
    pub worktree_checks: WorktreeChecks,
    pub dependencies: DependencyChanges,
    /// A check that failed and then passed when run again in this many of
    /// its last [`VerifierSpec::flaky_window`] runs is marked flaky. 0 is off.
    pub flaky_after: u32,
    pub flaky_window: u32,
}

impl Default for VerifierSpec {
    fn default() -> VerifierSpec {
        VerifierSpec {
            clean: true,
            block_test_edits: true,
            base_suite: true,
            revert_probe: false,
            worktree_checks: WorktreeChecks::Escalate,
            dependencies: DependencyChanges::Flag,
            flaky_after: 3,
            flaky_window: 10,
        }
    }
}

impl Default for GateSpec {
    fn default() -> GateSpec {
        GateSpec {
            checks: Vec::new(),
            max_blocks: DEFAULT_MAX_BLOCKS,
            protected: Vec::new(),
            evidence: true,
            subagent_checks: false,
            verifier: VerifierSpec::default(),
            allow_test_changes: Vec::new(),
            level: GateLevel::Full,
            substrate: Vec::new(),
            allow_substrate: Vec::new(),
            env: Vec::new(),
            warnings: Vec::new(),
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

    /// Every harness glob: the built-in list, the repository's own and the
    /// check programs that are paths inside it.
    pub fn substrate_globs(&self) -> Vec<String> {
        crate::verifier::substrate::substrate_globs(&self.substrate, &self.checks)
    }

    /// The checks that run at this spec's level, and those left out.
    pub fn planned(&self) -> (Vec<&GateCheck>, Vec<&GateCheck>) {
        self.checks.iter().partition(|c| c.level <= self.level)
    }

    /// The spec as one canonical text: what decides a verdict. Its hash
    /// goes into every ledger entry the gate writes, so an entry says which
    /// rules judged it.
    pub fn policy_text(&self) -> String {
        let mut t = String::new();
        let list = |t: &mut String, name: &str, items: &[String]| {
            t.push_str(&format!("{name}={}\n", items.join("\u{1f}")));
        };
        for c in &self.checks {
            t.push_str(&format!(
                "check={}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\n",
                c.name,
                c.program,
                c.args.join("\u{1e}"),
                c.timeout_secs,
                c.dir.as_deref().unwrap_or(""),
                c.required,
                c.level.as_str(),
                c.env.join("\u{1e}")
            ));
        }
        t.push_str(&format!(
            "max_blocks={}\nevidence={}\nsubagent_checks={}\nlevel={}\n",
            self.max_blocks,
            self.evidence,
            self.subagent_checks,
            self.level.as_str()
        ));
        let v = &self.verifier;
        t.push_str(&format!(
            "clean={}\nblock_test_edits={}\nbase_suite={}\nrevert_probe={}\nworktree_checks={}\ndependencies={}\n",
            v.clean,
            v.block_test_edits,
            v.base_suite,
            v.revert_probe,
            matches!(v.worktree_checks, WorktreeChecks::Allow),
            matches!(v.dependencies, DependencyChanges::Block),
        ));
        list(&mut t, "protected", &self.protected);
        list(&mut t, "allow_test_changes", &self.allow_test_changes);
        list(&mut t, "substrate", &self.substrate);
        list(&mut t, "allow_substrate", &self.allow_substrate);
        list(&mut t, "env", &self.env);
        t
    }

    /// The hash of [`GateSpec::policy_text`], hex.
    pub fn policy_sha256(&self) -> String {
        crate::ledger::hex(&crate::ledger::sha256(self.policy_text().as_bytes()))
    }
}

/// Environment variables a check sees: the program must find its tools and
/// its home, and nothing more. Names ending in `*` match by prefix.
pub const ENV_ALLOWED: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "USER",
    "USERNAME",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "LC_*",
    "TZ",
    "TERM",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "SystemRoot",
    "SYSTEMDRIVE",
    "WINDIR",
    "windir",
    "COMSPEC",
    "PATHEXT",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "CARGO_INCREMENTAL",
    "CARGO_BUILD_JOBS",
    "CARGO_NET_OFFLINE",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "RUST_BACKTRACE",
    "NODE_PATH",
    "NVM_DIR",
    "VOLTA_HOME",
    "PNPM_HOME",
    "VIRTUAL_ENV",
    "PYENV_ROOT",
    "PYTHONPATH",
    "UV_CACHE_DIR",
    "GOPATH",
    "GOROOT",
    "GOCACHE",
    "GOFLAGS",
    "GOPROXY",
    "GOMODCACHE",
    "PUB_CACHE",
    "FLUTTER_ROOT",
    "DART_SDK",
    "JAVA_HOME",
    "DOTNET_ROOT",
    "GRADLE_USER_HOME",
];

/// Names never passed to a check, whatever the configuration says: what
/// swarmux and the model providers use to authenticate, and the usual
/// names of secrets. Compared without case; `*` is a wildcard at the end
/// or the start.
pub const ENV_DENIED: &[&str] = &[
    "SWARMUX_*",
    "ANTHROPIC_*",
    "CLAUDE_*",
    "CLAUDECODE",
    "OPENAI_*",
    "GEMINI_*",
    "GOOGLE_*",
    "OPENROUTER_*",
    "MISTRAL_*",
    "COHERE_*",
    "HF_*",
    "AWS_*",
    "AZURE_*",
    "GCP_*",
    "GITHUB_*",
    "GH_*",
    "GITLAB_*",
    "NPM_TOKEN",
    "*_TOKEN",
    "*_SECRET",
    "*_PASSWORD",
    "*_API_KEY",
    "*_APIKEY",
    "*_CREDENTIALS",
];

fn name_matches(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    match (p.strip_prefix('*'), p.strip_suffix('*')) {
        (Some(tail), _) => n.ends_with(tail),
        (_, Some(head)) => n.starts_with(head),
        _ => p == n,
    }
}

/// Whether `name` is one no configuration can let through.
pub fn env_denied(name: &str) -> bool {
    ENV_DENIED.iter().any(|d| name_matches(d, name))
}

/// Whether an environment variable may reach a check that lists
/// `extra`: the allowlist and the check's own names, minus
/// [`ENV_DENIED`] (which no list can override).
pub fn env_passes(name: &str, extra: &[String]) -> bool {
    if env_denied(name) {
        return false;
    }
    ENV_ALLOWED.iter().any(|a| name_matches(a, name))
        || extra.iter().any(|e| e.eq_ignore_ascii_case(name))
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
    written_paths(tool, subject)
        .iter()
        .find_map(|path| protected_path(globs, root, path, cwd).map(str::to_string))
}

/// The paths a tool call writes, as it names them (relative to its folder
/// or absolute): the file of an edit, or for a shell line the redirect
/// targets and the paths given to programs that change files. Empty for
/// calls that write nothing this can see.
pub fn written_paths(tool: &str, subject: &str) -> Vec<String> {
    if WRITE_TOOLS.contains(&tool) {
        return vec![subject.to_string()];
    }
    if !matches!(tool, "Bash" | "PowerShell") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for command in split_commands(subject) {
        let words = words_of(strip_wrappers(&command));
        out.extend(redirect_targets(&words));
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
        out.extend(targets.into_iter().cloned());
    }
    out
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

/// Where the visible checks ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RanIn {
    /// In a clean copy of the work that swarmux made and owns.
    CleanCopy,
    /// In the agent's own worktree, next to whatever it left there.
    Worktree,
}

impl RanIn {
    pub fn as_str(self) -> &'static str {
        match self {
            RanIn::CleanCopy => "clean_copy",
            RanIn::Worktree => "worktree",
        }
    }

    pub fn parse(s: &str) -> Option<RanIn> {
        match s {
            "clean_copy" => Some(RanIn::CleanCopy),
            "worktree" => Some(RanIn::Worktree),
            _ => None,
        }
    }
}

/// Hidden checks that exist for the repository and did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HiddenNotRun {
    /// How many there are. Never which.
    pub planned: u32,
    /// Why they did not run, for the user.
    pub reason: String,
}

impl HiddenNotRun {
    /// `hidden checks: not run (no clean copy)`.
    pub fn text(&self) -> String {
        format!("hidden checks: not run ({})", self.reason)
    }
}

/// The revert probe: the change without its non-test part, run against
/// the tests the job added or changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertProbe {
    /// At least one new or changed test failed without the change.
    pub proved: bool,
    /// What was reverted and what the tests did, in a line.
    pub detail: String,
}

/// Everything one stop found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GateFindings {
    pub runs: Vec<CheckRun>,
    pub pins: Vec<PinChange>,
    pub gaps: Vec<String>,
    /// How the hidden checks went, when any ran: counts only.
    pub hidden: Option<HiddenTally>,
    /// Changes that make the tests easier to pass: the evidence lines.
    pub tampering: Vec<String>,
    /// Whether any of them touches what the mission's checks run.
    pub checks_weakened: bool,
    /// The checks run against the original tests, when that was done.
    pub base_suite: Option<BaseSuite>,
    /// Harness files (test runner configuration, build scripts, plugins)
    /// the job created, changed or removed: the evidence lines.
    pub substrate: Vec<String>,
    /// New or changed dependencies, one line per manifest or lockfile.
    pub dependencies: Vec<String>,
    /// Whether new dependencies block (otherwise they are listed).
    pub block_dependencies: bool,
    /// Hidden checks that exist and did not run.
    pub hidden_not_run: Option<HiddenNotRun>,
    /// Where the visible checks ran; `None` when none ran.
    pub ran_in: Option<RanIn>,
    /// The names of the checks the gate planned to run at its level.
    pub planned: Vec<String>,
    /// Checks the gate left out, with why (above the gate's level).
    pub left_out: Vec<(String, String)>,
    /// Names of the checks whose failure blocks nothing.
    pub optional: Vec<String>,
    pub revert_probe: Option<RevertProbe>,
    /// Why the change could not be read to scan it for tampering (no diff).
    pub unscanned: Option<String>,
    /// Why the work could not be verified at all after the job ended.
    pub unverified: Option<String>,
    /// The hash of the gate's configuration that judged: see
    /// [`GateSpec::policy_sha256`].
    pub policy_sha256: String,
    /// `(after, window)` of [`VerifierSpec`]: when a check counts as
    /// flaky. `(0, 0)`: not tracked.
    pub flake_policy: (u32, u32),
    /// Lines of `gates.toml` that were read with a fallback to a default.
    pub warnings: Vec<String>,
    /// Why `.swarmux/gates.toml` could not be used. The gate never passes
    /// on a configuration it could not read.
    pub config_problem: Option<String>,
    /// The job has nobody watching: a mission or a background job.
    pub unattended: bool,
    /// The repository allows checks that ran in the worktree to count.
    pub worktree_allowed: bool,
}

impl GateFindings {
    /// Nothing wrong with the change: every required check passed and no
    /// finding is left.
    pub fn clean(&self) -> bool {
        self.runs
            .iter()
            .all(|r| r.passed() || self.optional.contains(&r.name))
            && self.pins.is_empty()
            && self.gaps.is_empty()
            && self.hidden.is_none_or(|h| h.clean())
            && self.tampering.is_empty()
            && self.substrate.is_empty()
            && self.base_suite.as_ref().is_none_or(BaseSuite::clean)
            && (self.dependencies.is_empty() || !self.block_dependencies)
            && self.revert_probe.as_ref().is_none_or(|p| p.proved)
    }

    /// What keeps the gate from vouching for the work even though nothing
    /// is wrong with it: the checks that should have been the judge did not
    /// run as they should. A mission's verdict is Escalate then, never Pass.
    pub fn incomplete(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = &self.config_problem {
            out.push(format!("gates.toml could not be used: {p}"));
        }
        if let Some(u) = &self.unverified {
            out.push(format!("unverified: {u}"));
        }
        if self.unattended {
            if let Some(u) = &self.unscanned {
                out.push(format!("not scanned for tampering: {u}"));
            }
            if let Some(h) = &self.hidden_not_run {
                out.push(h.text());
            }
            if self.ran_in == Some(RanIn::Worktree) && !self.worktree_allowed {
                out.push(
                    "the checks ran in the worktree, not in a clean copy of the work".to_string(),
                );
            }
        }
        out
    }

    /// Whether any check ran, visible or hidden.
    pub fn checked(&self) -> bool {
        !self.runs.is_empty() || self.hidden.is_some_and(|h| h.ran > 0)
    }

    /// The required checks that did not pass.
    pub fn failed(&self) -> impl Iterator<Item = &CheckRun> {
        self.runs
            .iter()
            .filter(|r| !r.passed() && !self.optional.contains(&r.name))
    }

    /// The optional checks that did not pass.
    pub fn failed_optional(&self) -> impl Iterator<Item = &CheckRun> {
        self.runs
            .iter()
            .filter(|r| !r.passed() && self.optional.contains(&r.name))
    }
}

/// What the gate does with a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateOutcome {
    /// Let the agent stop.
    Pass,
    /// Make it continue with the digest.
    Block,
    /// Still wrong after the last allowed block, or not able to vouch for
    /// the work: let it stop and ask the user.
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
    if !findings.clean() {
        if blocks < max_blocks {
            GateOutcome::Block
        } else {
            GateOutcome::Escalate
        }
    } else if !findings.incomplete().is_empty() {
        GateOutcome::Escalate
    } else {
        GateOutcome::Pass
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
    // What the user should know about how the checks ran, whatever the
    // outcome. Dependencies that block are told with the findings instead.
    let notes = |out: &mut String| {
        if let Some(h) = &findings.hidden_not_run {
            out.push_str(&format!("⚠ {}\n", h.text()));
        }
        if findings.ran_in == Some(RanIn::Worktree) {
            out.push_str(
                "⚠ the checks ran in the worktree, not in a clean copy: files the agent left there may have been in play.\n",
            );
        }
        if !findings.block_dependencies {
            for d in &findings.dependencies {
                out.push_str(&format!("⚠ new or changed dependencies: {d}\n"));
            }
        }
        for (name, why) in &findings.left_out {
            out.push_str(&format!("⚠ check {name} not run ({why})\n"));
        }
        if let Some(p) = &findings.config_problem {
            out.push_str(&format!(
                "⛔ gates.toml could not be used: {p}. Until it is fixed the gate cannot pass.\n"
            ));
        }
        if let Some(u) = &findings.unverified {
            out.push_str(&format!("⚠ unverified: {u}\n"));
        }
        for w in &findings.warnings {
            out.push_str(&format!("⚠ gates.toml: {w}\n"));
        }
        if let Some(u) = &findings.unscanned {
            out.push_str(&format!("⚠ not scanned for tampering: {u}\n"));
        }
        if let Some(p) = findings.revert_probe.as_ref().filter(|p| p.proved) {
            out.push_str(&format!("revert probe: {}\n", p.detail));
        }
    };
    let mut out = match outcome {
        GateOutcome::Pass => {
            let n = findings.runs.len();
            let hidden = findings
                .hidden
                .filter(|h| h.ran > 0)
                .map_or_else(String::new, |h| format!(", {} hidden", h.ran));
            let mut line = format!(
                "swarmux gate: {n} check{} passed{hidden}\n",
                if n == 1 { "" } else { "s" }
            );
            notes(&mut line);
            return cut(line.trim_end(), DIGEST_MAX);
        }
        GateOutcome::Block => format!(
            "swarmux gate: not done yet (block {} of {max_blocks}). \
             Fix what follows, then finish again.\n",
            blocks + 1
        ),
        GateOutcome::Escalate if findings.clean() => {
            "swarmux gate: cannot vouch for this work.\n".to_string()
        }
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
    for run in findings.failed_optional() {
        out.push_str(&format!(
            "~ optional check {}: {} (does not block)\n",
            run.name,
            run.status_text()
        ));
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
    for t in &findings.tampering {
        out.push_str(&format!(
            "⛔ tests weakened: {t}. Tests are the referee, not yours to change: put them back and fix the code.\n"
        ));
    }
    for s in &findings.substrate {
        out.push_str(&format!(
            "⛔ test harness changed: {s}. The files that find, build and run the tests are the referee's: put them back as they were.\n"
        ));
    }
    if let Some(b) = findings.base_suite.as_ref().filter(|b| !b.clean()) {
        out.push_str(&format!("⛔ {}.\n", b.text()));
        for (name, line) in &b.failed {
            out.push_str(&format!("    {name}: {line}\n"));
        }
        out.push_str(
            "The original tests are the referee: your code has to pass them as they were, not as you changed them.\n",
        );
    }
    if let Some(h) = findings.hidden.filter(|h| !h.clean()) {
        out.push_str(&format!(
            "⛔ hidden checks: {} of {} failed. They are kept from you on purpose; make the change correct in general, not only for the tests you can see.\n",
            h.failed(),
            h.ran
        ));
    }
    if let Some(p) = findings.revert_probe.as_ref().filter(|p| !p.proved) {
        out.push_str(&format!(
            "⛔ revert probe: {}. A test that passes without the change does not prove the change: add or sharpen a test that fails without it.\n",
            p.detail
        ));
    }
    if findings.block_dependencies {
        for d in &findings.dependencies {
            out.push_str(&format!(
                "⛔ new or changed dependencies: {d}. A person has to look at these first.\n"
            ));
        }
    }
    if outcome != GateOutcome::Block {
        notes(&mut out);
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

pub mod flake;

#[cfg(test)]
mod tests;
