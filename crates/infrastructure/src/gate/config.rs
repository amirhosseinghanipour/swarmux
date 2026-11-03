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
    DEFAULT_CHECK_TIMEOUT_SECS, DEFAULT_MAX_BLOCKS, GateCheck, GateOrigin, GateSpec, Pin,
    RepoFacts, detect, protected_glob, search_roots,
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
}

impl Default for RawGate {
    fn default() -> RawGate {
        RawGate {
            max_blocks: DEFAULT_MAX_BLOCKS,
            evidence: true,
            subagent_checks: false,
            detect: true,
            protected: Vec::new(),
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
}

/// Parses a `gates.toml`. With no `[[check]]` and `detect` on, the checks
/// are guessed from `repo`.
pub fn parse_gates(text: &str, repo: &RepoFacts) -> Result<GateSpec, String> {
    let raw: RawFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
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
        checks.push(check);
    }
    if checks.is_empty() && raw.gate.detect {
        checks = detect(repo);
    }
    if let Some(bad) = raw
        .gate
        .protected
        .iter()
        .find(|g| g.split('/').any(|s| s == ".."))
    {
        return Err(format!("protected path {bad:?} may not contain .."));
    }
    Ok(GateSpec {
        checks,
        max_blocks: raw.gate.max_blocks,
        protected: raw.gate.protected,
        evidence: raw.gate.evidence,
        subagent_checks: raw.gate.subagent_checks,
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

/// The repository `dir` lies in: the nearest folder with a `gates.toml`,
/// else the nearest with `.git`, else `dir`.
///
/// The root is written the way `dir` is (not canonicalised), so paths the
/// agent's tools name relative to their working folder compare with it.
fn root_of(dir: &Path) -> PathBuf {
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
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
}
