//! `GateSource` adapter: finds a folder's repository, reads its
//! `.swarmux/gates.toml` or guesses its checks, and hashes protected files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use serde::Deserialize;
use swarmux_application::{GateSource, LoadedGate};
use swarmux_domain::gate::{
    DEFAULT_CHECK_TIMEOUT_SECS, DEFAULT_MAX_BLOCKS, DependencyChanges, GateCheck, GateLevel,
    GateOrigin, GateSpec, Pin, RepoFacts, VerifierSpec, WorktreeChecks, detect, env_denied,
    protected_glob, search_roots,
};
use swarmux_domain::ledger::{hex, sha256};

/// Where a repository's gate lives, relative to its root.
pub const GATES_FILE: &str = ".swarmux/gates.toml";
/// Folders never searched for protected files.
const SKIPPED: [&str; 7] = [
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
];
/// Files below this are swarmux's to change, not the agents'.
const MISSIONS_PREFIX: &str = ".swarmux/missions/";
/// Protected files hashed at most, so a broad glob can't stall a hook.
const PINS_MAX: usize = 5000;
/// Files larger than this are pinned by size, not content.
const PIN_BYTES_MAX: u64 = 64 << 20;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    gate: RawGate,
    #[serde(default)]
    check: Vec<RawCheck>,
    #[serde(default)]
    verifier: RawVerifier,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVerifier {
    /// Run the checks in a clean copy (default) or in the worktree.
    clean: Option<bool>,
    /// `"block"` (default) or `"allow"`: edits to existing tests.
    test_edits: Option<String>,
    /// Run the checks again against the original tests (default: yes).
    base_suite: Option<bool>,
    /// Globs of tests that may be removed or changed on purpose.
    #[serde(default)]
    allow_test_changes: Vec<String>,
    // The keys below are read key by key: one with a value that cannot be
    // used falls back to its (strict) default with a warning, instead of
    // taking the whole file down.
    /// Harness files besides the built-in list.
    substrate: Option<toml::Value>,
    /// Harness files this repository changes on purpose.
    allow_substrate: Option<toml::Value>,
    /// Revert the non-test part of the change and require a test to fail.
    revert_probe: Option<toml::Value>,
    /// `"escalate"` (default) or `"allow"`: a mission whose checks ran in
    /// the worktree.
    worktree_checks: Option<toml::Value>,
    /// `"flag"` (default) or `"block"`: new dependencies.
    dependencies: Option<toml::Value>,
    /// Environment variables every check may see besides the allowlist.
    env: Option<toml::Value>,
    /// A check that flaked this many times...
    flaky_after: Option<toml::Value>,
    /// ...in its last this many runs is called flaky. 0 turns it off.
    flaky_window: Option<toml::Value>,
}

/// The words of a gate level.
const LEVELS: [(&str, GateLevel); 3] = [
    ("full", GateLevel::Full),
    ("fast", GateLevel::Fast),
    ("deep", GateLevel::Deep),
];

fn verifier_spec(raw: &RawVerifier) -> Result<VerifierSpec, String> {
    let mut spec = VerifierSpec::default();
    if let Some(clean) = raw.clean {
        spec.clean = clean;
    }
    if let Some(base) = raw.base_suite {
        spec.base_suite = base;
    }
    spec.block_test_edits = match raw.test_edits.as_deref().map(str::trim) {
        None | Some("block") => true,
        Some("allow") => false,
        Some(other) => {
            return Err(format!(
                "verifier.test_edits is \"block\" or \"allow\", not {other:?}"
            ));
        }
    };
    Ok(spec)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGate {
    #[serde(default = "default_max_blocks")]
    max_blocks: u32,
    #[serde(default = "yes")]
    evidence: bool,
    #[serde(default)]
    subagent_checks: bool,
    #[serde(default = "yes")]
    detect: bool,
    #[serde(default)]
    protected: Vec<String>,
    /// `"fast"`, `"full"` (default) or `"deep"`: which checks run.
    level: Option<toml::Value>,
}

impl Default for RawGate {
    fn default() -> RawGate {
        RawGate {
            max_blocks: DEFAULT_MAX_BLOCKS,
            evidence: true,
            subagent_checks: false,
            detect: true,
            protected: Vec::new(),
            level: None,
        }
    }
}

fn default_max_blocks() -> u32 {
    DEFAULT_MAX_BLOCKS
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    name: Option<String>,
    run: toml::Value,
    timeout_secs: Option<u64>,
    /// `true` (default): a failure blocks. `false`: it is reported only.
    required: Option<toml::Value>,
    /// `"fast"`, `"full"` (default) or `"deep"`: from which gate level it runs.
    level: Option<toml::Value>,
    /// `"fail"`, the only value: a check that cannot run never passes.
    on_error: Option<toml::Value>,
    /// Environment variables this check may see besides the allowlist.
    env: Option<toml::Value>,
}

/// A list of strings from a value that should be one; what is not a
/// string is left out and said.
fn string_list(key: &str, value: Option<&toml::Value>, warnings: &mut Vec<String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        warnings.push(format!("{key} is a list of strings; it is ignored"));
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        match item.as_str() {
            Some(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
            _ => warnings.push(format!("{key}: {item} is not a string; it is left out")),
        }
    }
    out
}

/// A glob list that may not climb out of the repository.
fn glob_list(key: &str, value: Option<&toml::Value>, warnings: &mut Vec<String>) -> Vec<String> {
    string_list(key, value, warnings)
        .into_iter()
        .filter(|g| {
            let bad = g.split(['/', '\\']).any(|s| s == "..");
            if bad {
                warnings.push(format!("{key}: {g:?} may not contain ..; it is left out"));
            }
            !bad
        })
        .collect()
}

/// Environment variable names a check may see, without the ones no list
/// can allow.
fn env_list(key: &str, value: Option<&toml::Value>, warnings: &mut Vec<String>) -> Vec<String> {
    string_list(key, value, warnings)
        .into_iter()
        .filter(|n| {
            let denied = env_denied(n);
            if denied {
                warnings.push(format!(
                    "{key}: {n} is never passed to a check (credentials stay out); it is left out"
                ));
            }
            !denied
        })
        .collect()
}

/// A choice between words, or `default` with a warning.
fn choice<T: Copy>(
    key: &str,
    value: Option<&toml::Value>,
    options: &[(&str, T)],
    default: T,
    warnings: &mut Vec<String>,
) -> T {
    let Some(value) = value else {
        return default;
    };
    let words: Vec<&str> = options.iter().map(|(w, _)| *w).collect();
    match value
        .as_str()
        .and_then(|v| options.iter().find(|(w, _)| *w == v.trim()))
    {
        Some((_, found)) => *found,
        None => {
            warnings.push(format!(
                "{key} is one of {}, not {value}; the default ({}) is used",
                words.join(", "),
                words[0]
            ));
            default
        }
    }
}

fn flag(key: &str, value: Option<&toml::Value>, default: bool, warnings: &mut Vec<String>) -> bool {
    match value {
        None => default,
        Some(v) => v.as_bool().unwrap_or_else(|| {
            warnings.push(format!(
                "{key} is true or false, not {v}; {default} is used"
            ));
            default
        }),
    }
}

fn count(key: &str, value: Option<&toml::Value>, default: u32, warnings: &mut Vec<String>) -> u32 {
    match value {
        None => default,
        Some(v) => v
            .as_integer()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or_else(|| {
                warnings.push(format!(
                    "{key} is a number from 0 up, not {v}; {default} is used"
                ));
                default
            }),
    }
}

/// Parses a `gates.toml`. With no `[[check]]` and `detect` on, the checks
/// are guessed from `repo`.
pub fn parse_gates(text: &str, repo: &RepoFacts) -> Result<GateSpec, String> {
    let raw: RawFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    let mut warnings = Vec::new();
    let mut checks = Vec::new();
    for (i, c) in raw.check.iter().enumerate() {
        let args: Vec<String> = match &c.run {
            toml::Value::Array(items) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("check {}: run must list strings", i + 1))?,
            _ => {
                return Err(format!(
                    "check {}: run is a list of arguments, e.g. run = [\"cargo\", \"test\"] \
                     (checks never go through a shell)",
                    i + 1
                ));
            }
        };
        let Some((program, rest)) = args.split_first() else {
            return Err(format!("check {}: run is empty", i + 1));
        };
        if program.trim().is_empty() {
            return Err(format!("check {}: run starts with an empty program", i + 1));
        }
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        let mut check =
            GateCheck::new(c.name.as_deref().unwrap_or(&args.join(" ")), program, &rest);
        check.timeout_secs = match c.timeout_secs {
            Some(0) => return Err(format!("check {}: timeout_secs must be above 0", i + 1)),
            Some(t) => t,
            None => DEFAULT_CHECK_TIMEOUT_SECS,
        };
        let key = |k: &str| format!("check {} ({}) {k}", i + 1, check.name);
        check.required = flag(&key("required"), c.required.as_ref(), true, &mut warnings);
        check.level = choice(
            &key("level"),
            c.level.as_ref(),
            &LEVELS,
            GateLevel::Full,
            &mut warnings,
        );
        // The only value: a check that cannot run (no program, no folder,
        // a timeout) never passes. Anything else asks for a fail-open
        // check, which is refused with a warning.
        if let Some(v) = &c.on_error
            && v.as_str().map(str::trim) != Some("fail")
        {
            warnings.push(format!(
                "{} is \"fail\", the only value: a check that cannot run never passes; {v} is ignored",
                key("on_error")
            ));
        }
        check.env = env_list(&key("env"), c.env.as_ref(), &mut warnings);
        checks.push(check);
    }
    if checks.is_empty() && raw.gate.detect {
        checks = detect(repo);
    }
    let v = &raw.verifier;
    let level = choice(
        "gate level",
        raw.gate.level.as_ref(),
        &LEVELS,
        GateLevel::Full,
        &mut warnings,
    );
    let mut env = env_list("verifier env", v.env.as_ref(), &mut warnings);
    env.sort();
    env.dedup();
    for c in &mut checks {
        for name in &env {
            if !c.env.contains(name) {
                c.env.push(name.clone());
            }
        }
    }
    if let Some(bad) = raw
        .gate
        .protected
        .iter()
        .find(|g| g.split('/').any(|s| s == ".."))
    {
        return Err(format!("protected path {bad:?} may not contain .."));
    }
    let mut verifier = verifier_spec(&raw.verifier)?;
    verifier.revert_probe = flag(
        "verifier revert_probe",
        v.revert_probe.as_ref(),
        false,
        &mut warnings,
    );
    verifier.worktree_checks = choice(
        "verifier worktree_checks",
        v.worktree_checks.as_ref(),
        &[
            ("escalate", WorktreeChecks::Escalate),
            ("allow", WorktreeChecks::Allow),
        ],
        WorktreeChecks::Escalate,
        &mut warnings,
    );
    verifier.dependencies = choice(
        "verifier dependencies",
        v.dependencies.as_ref(),
        &[
            ("flag", DependencyChanges::Flag),
            ("block", DependencyChanges::Block),
        ],
        DependencyChanges::Flag,
        &mut warnings,
    );
    verifier.flaky_after = count(
        "verifier flaky_after",
        v.flaky_after.as_ref(),
        verifier.flaky_after,
        &mut warnings,
    );
    verifier.flaky_window = count(
        "verifier flaky_window",
        v.flaky_window.as_ref(),
        verifier.flaky_window,
        &mut warnings,
    )
    .max(1);
    let substrate = glob_list("verifier substrate", v.substrate.as_ref(), &mut warnings);
    let allow_substrate = glob_list(
        "verifier allow_substrate",
        v.allow_substrate.as_ref(),
        &mut warnings,
    );
    Ok(GateSpec {
        checks,
        max_blocks: raw.gate.max_blocks,
        protected: raw.gate.protected,
        evidence: raw.gate.evidence,
        subagent_checks: raw.gate.subagent_checks,
        verifier,
        allow_test_changes: raw.verifier.allow_test_changes,
        level,
        substrate,
        allow_substrate,
        env,
        warnings,
    })
}

type Stamp = (Option<SystemTime>, Option<SystemTime>);

/// Reads gates from the file system, re-reading a repository's gate only
/// when its `gates.toml` or its root folder changed.
#[derive(Default)]
pub struct FsGateSource {
    cache: Mutex<HashMap<PathBuf, (Stamp, LoadedGate)>>,
}

impl FsGateSource {
    pub fn new() -> FsGateSource {
        FsGateSource::default()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The repository `dir` lies in: a worktree set's task folder, else the
/// nearest folder with a `gates.toml`, else the nearest with `.git`, else
/// `dir`.
///
/// The root is written the way `dir` is (not canonicalised), so paths the
/// agent's tools name relative to their working folder compare with it.
fn root_of(dir: &Path) -> PathBuf {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    // A workspace job's task folder holds a worktree per repository: it
    // is the root its checks run from.
    if let Some(task) = dir
        .ancestors()
        .find(|a| a.join(crate::worktree::TASK_MARKER).is_file())
    {
        return task.to_path_buf();
    }
    if let Some(found) = dir.ancestors().find(|a| a.join(GATES_FILE).is_file()) {
        return found.to_path_buf();
    }
    dir.ancestors()
        .find(|a| a.join(".git").exists())
        .unwrap_or(&dir)
        .to_path_buf()
}

/// What detection needs to know about the files at `root`.
fn facts_of(root: &Path) -> RepoFacts {
    let files: Vec<String> = fs::read_dir(root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let read = |name: &str| fs::read_to_string(root.join(name)).unwrap_or_default();
    let npm_test = serde_json::from_str::<serde_json::Value>(&read("package.json"))
        .ok()
        .and_then(|v| v["scripts"]["test"].as_str().map(str::to_string));
    let pytest_configured = read("pyproject.toml").contains("[tool.pytest")
        || read("setup.cfg").contains("[tool:pytest]")
        || read("tox.ini").contains("[pytest]");
    RepoFacts {
        files,
        npm_test,
        pytest_configured,
    }
}

fn load_at(root: &Path) -> LoadedGate {
    let file = root.join(GATES_FILE);
    let facts = facts_of(root);
    let (spec, origin, problem) = match fs::read_to_string(&file) {
        Ok(text) => match parse_gates(&text, &facts) {
            Ok(spec) => (spec, GateOrigin::File, None),
            Err(e) => (
                GateSpec::default(),
                GateOrigin::File,
                Some(format!("{GATES_FILE}: {e}")),
            ),
        },
        Err(_) => (
            GateSpec {
                checks: detect(&facts),
                ..GateSpec::default()
            },
            GateOrigin::Detected,
            None,
        ),
    };
    LoadedGate {
        root: root.to_path_buf(),
        spec,
        origin,
        problem,
    }
}

impl GateSource for FsGateSource {
    fn load(&self, dir: &Path) -> LoadedGate {
        let root = root_of(dir);
        let stamp = (modified(&root.join(GATES_FILE)), modified(&root));
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, gate)) = cache.get(&root)
            && *at == stamp
        {
            return gate.clone();
        }
        let gate = load_at(&root);
        cache.insert(root, (stamp, gate.clone()));
        gate
    }

    fn pins(&self, root: &Path, globs: &[String]) -> Vec<Pin> {
        let fold = cfg!(windows);
        let mut out = Vec::new();
        for start in search_roots(globs) {
            let dir = if start.is_empty() {
                root.to_path_buf()
            } else {
                root.join(&start)
            };
            walk(&dir, &start, &mut |rel, path| {
                // swarmux itself writes mission contracts there while
                // agents run; writes by agents are still refused.
                let ours = rel.starts_with(MISSIONS_PREFIX);
                if out.len() < PINS_MAX && !ours && protected_glob(globs, rel, fold).is_some() {
                    out.push(Pin {
                        path: rel.to_string(),
                        sha256: hash_file(path),
                    });
                }
            });
        }
        out.sort();
        out
    }
}

/// Calls `found` with each file below `dir` and its path relative to the
/// root (`rel` is `dir`'s). Links are not followed.
fn walk(dir: &Path, rel: &str, found: &mut dyn FnMut(&str, &Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIPPED.contains(&name.as_str()) {
                walk(&entry.path(), &child, found);
            }
        } else if kind.is_file() {
            found(&child, &entry.path());
        }
    }
}

fn hash_file(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(m) if m.len() > PIN_BYTES_MAX => format!("size:{}", m.len()),
        Ok(_) => fs::read(path).map_or_else(|e| format!("unreadable:{e}"), |b| hex(&sha256(&b))),
        Err(e) => format!("unreadable:{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(files: &[&str]) -> RepoFacts {
        RepoFacts {
            files: files.iter().map(|f| f.to_string()).collect(),
            ..RepoFacts::default()
        }
    }

    #[test]
    fn a_full_file() {
        let spec = parse_gates(
            r#"
[gate]
max_blocks = 2
evidence = false
subagent_checks = true
protected = ["tests/oracles/**"]

[[check]]
name = "unit"
run = ["cargo", "test", "--workspace"]
timeout_secs = 900

[[check]]
run = ["cargo", "clippy"]
"#,
            &facts(&["Cargo.toml"]),
        )
        .unwrap();
        assert_eq!(spec.max_blocks, 2);
        assert!(!spec.evidence);
        assert!(spec.subagent_checks);
        assert_eq!(spec.protected, ["tests/oracles/**"]);
        assert_eq!(spec.checks.len(), 2, "listed checks replace detection");
        assert_eq!(spec.checks[0].name, "unit");
        assert_eq!(spec.checks[0].command_line(), "cargo test --workspace");
        assert_eq!(spec.checks[0].timeout_secs, 900);
        assert_eq!(spec.checks[1].name, "cargo clippy");
        assert_eq!(spec.checks[1].timeout_secs, DEFAULT_CHECK_TIMEOUT_SECS);
    }

    #[test]
    fn an_empty_file_detects_and_detection_can_be_off() {
        let spec = parse_gates("", &facts(&["go.mod"])).unwrap();
        assert_eq!(spec.checks[0].command_line(), "go test ./...");
        assert_eq!(spec.max_blocks, DEFAULT_MAX_BLOCKS);
        assert!(spec.evidence);
        let off = parse_gates("[gate]\ndetect = false\n", &facts(&["go.mod"])).unwrap();
        assert!(off.checks.is_empty());
    }

    #[test]
    fn mistakes_are_explained() {
        let f = facts(&[]);
        let err = parse_gates("[[check]]\nrun = \"cargo test\"\n", &f).unwrap_err();
        assert!(err.contains("never go through a shell"), "{err}");
        assert!(
            parse_gates("[[check]]\nrun = []\n", &f)
                .unwrap_err()
                .contains("empty")
        );
        assert!(parse_gates("[[check]]\nrun = [1]\n", &f).is_err());
        assert!(parse_gates("[gate]\nmax_block = 2\n", &f).is_err(), "typos");
        assert!(parse_gates("[[check]]\nrun = [\"x\"]\ntimeout_secs = 0\n", &f).is_err());
        assert!(parse_gates("[gate]\nprotected = [\"../x\"]\n", &f).is_err());
    }

    #[test]
    fn loads_from_the_repository_root_and_pins_protected_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join("tests/oracles")).unwrap();
        fs::create_dir_all(root.join("target/tests/oracles")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.join("tests/oracles/expected.txt"), "42").unwrap();
        fs::write(root.join("target/tests/oracles/x"), "skip").unwrap();
        let source = FsGateSource::new();

        let detected = source.load(&root.join("src/deep"));
        assert_eq!(detected.origin, GateOrigin::Detected);
        assert_eq!(detected.root, root_of(root));
        assert_eq!(detected.spec.checks[0].program, "cargo");

        fs::create_dir_all(root.join(".swarmux")).unwrap();
        fs::write(
            root.join(GATES_FILE),
            "[gate]\nprotected = [\"tests/oracles/**\"]\n",
        )
        .unwrap();
        let gate = source.load(&root.join("src"));
        assert_eq!(gate.origin, GateOrigin::File);
        assert_eq!(gate.spec.protected, ["tests/oracles/**"]);

        // Mission contracts are swarmux's to write while agents run.
        fs::create_dir_all(root.join(".swarmux/missions")).unwrap();
        fs::write(root.join(".swarmux/missions/m.toml"), "goal = \"g\"").unwrap();
        let pins = source.pins(&gate.root, &gate.spec.protected_globs());
        let paths: Vec<&str> = pins.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, [".swarmux/gates.toml", "tests/oracles/expected.txt"]);
        assert_eq!(pins[1].sha256, hex(&sha256(b"42")));

        fs::write(root.join(GATES_FILE), "[gate\n").unwrap();
        let broken = source.load(root);
        assert!(broken.problem.is_some_and(|p| p.starts_with(GATES_FILE)));
        assert!(broken.spec.checks.is_empty());
    }

    #[test]
    fn npm_and_pytest_facts_come_from_their_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"vitest run"}}"#,
        )
        .unwrap();
        fs::write(root.join("pyproject.toml"), "[tool.pytest.ini_options]\n").unwrap();
        let f = facts_of(root);
        assert_eq!(f.npm_test.as_deref(), Some("vitest run"));
        assert!(f.pytest_configured);
        let names: Vec<String> = detect(&f).iter().map(GateCheck::command_line).collect();
        assert_eq!(names, ["npm test", "pytest -q"]);
    }

    #[test]
    fn the_verifier_section_is_read() {
        let f = facts(&[]);
        let spec = parse_gates("", &f).unwrap();
        assert_eq!(spec.verifier, VerifierSpec::default());
        assert!(spec.verifier.clean && spec.verifier.block_test_edits);
        let spec = parse_gates("[verifier]\nclean = false\ntest_edits = \"allow\"\n", &f).unwrap();
        assert!(!spec.verifier.clean);
        assert!(!spec.verifier.block_test_edits);
        let err = parse_gates("[verifier]\ntest_edits = \"maybe\"\n", &f).unwrap_err();
        assert!(err.contains("block"), "{err}");
        assert!(
            parse_gates("[verifier]\nhidden = true\n", &f).is_err(),
            "typos"
        );
    }
}

/// The keys X2 added to `gates.toml`: each is enforced, and one with a
/// value that cannot be used falls back to its strict default with a
/// warning, never a refusal of the file.
#[cfg(test)]
mod harness_keys {
    use super::*;

    fn facts() -> RepoFacts {
        RepoFacts::default()
    }

    fn parse(text: &str) -> GateSpec {
        parse_gates(text, &facts()).unwrap()
    }

    #[test]
    fn the_defaults_are_the_strict_ones() {
        let spec = parse("");
        assert_eq!(spec.level, GateLevel::Full);
        assert!(spec.substrate.is_empty() && spec.allow_substrate.is_empty());
        assert!(spec.env.is_empty() && spec.warnings.is_empty());
        let v = spec.verifier;
        assert!(!v.revert_probe);
        assert_eq!(v.worktree_checks, WorktreeChecks::Escalate);
        assert_eq!(v.dependencies, DependencyChanges::Flag);
        assert_eq!((v.flaky_after, v.flaky_window), (3, 10));
    }

    #[test]
    fn every_new_key_is_read_and_reaches_the_spec() {
        let spec = parse(
            r#"
[gate]
level = "deep"

[[check]]
name = "unit"
run = ["cargo", "test"]
required = false
level = "fast"
on_error = "fail"
env = ["DATABASE_URL"]

[[check]]
name = "lint"
run = ["cargo", "clippy"]

[verifier]
substrate = ["scripts/ci/**"]
allow_substrate = ["Makefile"]
revert_probe = true
worktree_checks = "allow"
dependencies = "block"
env = ["API_BASE", "DATABASE_URL"]
flaky_after = 2
flaky_window = 5
"#,
        );
        assert_eq!(spec.level, GateLevel::Deep);
        assert_eq!(spec.substrate, ["scripts/ci/**"]);
        assert_eq!(spec.allow_substrate, ["Makefile"]);
        assert!(spec.verifier.revert_probe);
        assert_eq!(spec.verifier.worktree_checks, WorktreeChecks::Allow);
        assert_eq!(spec.verifier.dependencies, DependencyChanges::Block);
        assert_eq!(
            (spec.verifier.flaky_after, spec.verifier.flaky_window),
            (2, 5)
        );
        assert!(spec.warnings.is_empty(), "{:?}", spec.warnings);
        let unit = &spec.checks[0];
        assert!(!unit.required);
        assert_eq!(unit.level, GateLevel::Fast);
        // The check's own names and the verifier's, once each.
        assert_eq!(unit.env, ["DATABASE_URL", "API_BASE"]);
        let lint = &spec.checks[1];
        assert!(lint.required);
        assert_eq!(lint.level, GateLevel::Full);
        assert_eq!(lint.env, ["API_BASE", "DATABASE_URL"]);
    }

    #[test]
    fn a_value_that_cannot_be_used_falls_back_to_the_strict_default_with_a_warning() {
        let spec = parse(
            r#"
[gate]
level = "slowest"

[[check]]
run = ["cargo", "test"]
required = "yes"
level = 3
on_error = "warn"
env = "PATH"

[verifier]
substrate = "scripts"
allow_substrate = ["ok/**", 5, "../up/**"]
revert_probe = "sometimes"
worktree_checks = "whenever"
dependencies = "ignore"
env = ["ANTHROPIC_API_KEY", "SWARMUX_JOB_ID", "MY_SERVICE_TOKEN", "FINE", 7]
flaky_after = -1
flaky_window = "ten"
"#,
        );
        assert_eq!(spec.level, GateLevel::Full);
        let c = &spec.checks[0];
        assert!(c.required, "required falls back to true");
        assert_eq!(c.level, GateLevel::Full);
        assert!(spec.substrate.is_empty());
        assert_eq!(spec.allow_substrate, ["ok/**"], "bad entries are left out");
        let v = spec.verifier;
        assert!(!v.revert_probe);
        assert_eq!(v.worktree_checks, WorktreeChecks::Escalate);
        assert_eq!(v.dependencies, DependencyChanges::Flag);
        assert_eq!((v.flaky_after, v.flaky_window), (3, 10));
        // Credentials are refused, the rest kept.
        assert_eq!(spec.env, ["FINE"]);
        let all = spec.warnings.join("\n");
        for says in [
            "gate level is one of full, fast, deep",
            "check 1 (cargo test) required is true or false",
            "check 1 (cargo test) level is one of",
            "check 1 (cargo test) on_error is \"fail\", the only value",
            "check 1 (cargo test) env is a list of strings",
            "verifier substrate is a list of strings",
            "verifier allow_substrate: 5 is not a string",
            "verifier allow_substrate: \"../up/**\" may not contain ..",
            "verifier revert_probe is true or false",
            "verifier worktree_checks is one of escalate, allow",
            "verifier dependencies is one of flag, block",
            "verifier env: ANTHROPIC_API_KEY is never passed to a check",
            "verifier env: SWARMUX_JOB_ID is never passed to a check",
            "verifier env: MY_SERVICE_TOKEN is never passed to a check",
            "verifier flaky_after is a number from 0 up",
            "verifier flaky_window is a number from 0 up",
        ] {
            assert!(all.contains(says), "no warning {says:?} in:\n{all}");
        }
    }

    #[test]
    fn a_key_this_version_does_not_know_is_an_error_not_an_ignored_line() {
        for bad in [
            "[verifier]\nsubstrates = [\"x\"]\n",
            "[gate]\nlevels = \"fast\"\n",
            "[[check]]\nrun = [\"x\"]\nrequired_ = true\n",
        ] {
            assert!(parse_gates(bad, &facts()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_zero_window_still_looks_at_one_run() {
        let spec = parse("[verifier]\nflaky_window = 0\nflaky_after = 0\n");
        assert_eq!(spec.verifier.flaky_window, 1);
        assert_eq!(spec.verifier.flaky_after, 0, "off");
    }

    #[test]
    fn an_old_gates_file_reads_as_it_always_did() {
        // A file written before X2: nothing in it is new, and nothing it
        // says changes meaning.
        let spec = parse(
            r#"
[gate]
max_blocks = 2
protected = ["tests/oracles/**"]

[[check]]
name = "unit"
run = ["cargo", "test"]
timeout_secs = 300

[verifier]
clean = true
test_edits = "block"
base_suite = true
allow_test_changes = ["tests/legacy/**"]
"#,
        );
        assert_eq!(spec.max_blocks, 2);
        assert_eq!(spec.checks.len(), 1);
        assert!(spec.checks[0].required);
        assert_eq!(spec.checks[0].timeout_secs, 300);
        assert!(spec.warnings.is_empty());
        assert_eq!(spec.allow_test_changes, ["tests/legacy/**"]);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_reported_as_a_problem_not_swallowed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".swarmux")).unwrap();
        fs::write(root.join(GATES_FILE), "[gate\nmax_blocks = 1\n").unwrap();
        let gate = FsGateSource::new().load(root);
        let problem = gate.problem.expect("a problem");
        assert!(problem.starts_with(GATES_FILE), "{problem}");
        assert_eq!(gate.origin, GateOrigin::File);
        // The same through a typo in an old key.
        fs::write(root.join(GATES_FILE), "[gate]\nmax_block = 1\n").unwrap();
        let again = FsGateSource::new().load(root);
        assert!(again.problem.is_some_and(|p| p.contains("max_block")));
    }
}
