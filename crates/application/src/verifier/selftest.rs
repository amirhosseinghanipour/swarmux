//! The gate's canary: known-bad changes made in scratch copies of a
//! repository, which the gate has to refuse.
//!
//! A gate that has never been seen to say no proves nothing by saying yes.
//! `swarmux gate selftest` makes the changes an agent that wanted its work
//! to count would make: it plants a harness file that forces the tests to
//! pass, and it deletes an assertion from a test. Each goes into a copy of
//! the repository of its own (the repository's files are never touched),
//! and the gate judges that copy with the repository's own `gates.toml`. A
//! change the gate lets through is reported with why (typically a glob in
//! `allow_substrate` or `test_edits = "allow"`).

use swarmux_domain::verifier::is_test_path;

use super::*;

/// One known-bad change and what the gate made of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    /// What was done, like `planted conftest.py`.
    pub name: String,
    /// The gate refused it.
    pub caught: bool,
    /// What the gate said (the first finding), or why it let the change
    /// through.
    pub detail: String,
}

/// What the canary found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelfTest {
    pub probes: Vec<Probe>,
    /// Probes that could not be made, with why.
    pub skipped: Vec<(String, String)>,
}

impl SelfTest {
    /// Every probe was refused, and at least one was made.
    pub fn passed(&self) -> bool {
        !self.probes.is_empty() && self.probes.iter().all(|p| p.caught)
    }
}

/// A nine-line `conftest.py` that makes every pytest test pass.
const PLANTED_CONFTEST: &str = "import pytest\n\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    report = outcome.get_result()\n    report.outcome = \"passed\"\n    report.longrepr = None\n";

/// What a planted harness file is, for the repository's ecosystem: the
/// path, the text to add and a name for the probe.
fn planted(files: &[String]) -> (&'static str, &'static str, &'static str) {
    let has = |name: &str| files.iter().any(|f| f == name);
    if has("Cargo.toml") {
        (
            ".cargo/config.toml",
            "\n[target.'cfg(all())']\nrunner = \"true\"\n",
            "planted .cargo/config.toml with a runner that runs nothing",
        )
    } else if has("package.json") {
        (
            "jest.config.js",
            "\nmodule.exports = { testPathIgnorePatterns: ['/'] };\n",
            "planted jest.config.js that ignores every test",
        )
    } else {
        (
            "conftest.py",
            PLANTED_CONFTEST,
            "planted conftest.py that forces every test to pass",
        )
    }
}

/// The first test file with an assertion on a line of its own, and that
/// line's index.
fn an_assertion(copy: &dyn CleanCopy, files: &[String]) -> Option<(String, String, usize)> {
    let markers = ["assert", "expect(", ".should", "require("];
    files.iter().filter(|f| is_test_path(f)).find_map(|path| {
        let text = String::from_utf8(copy.read(path)?).ok()?;
        let at = text
            .lines()
            .position(|l| markers.iter().any(|m| l.contains(m)))?;
        Some((path.clone(), text, at))
    })
}

impl Verifier {
    /// Makes each known-bad change in a scratch copy of the repository
    /// `dir` lies in and judges it with `spec`. An error when no copy
    /// could be made at all.
    pub fn selftest(&self, dir: &Path, spec: &GateSpec) -> Result<SelfTest, String> {
        let mut out = SelfTest::default();
        let first = self.checkouts.snapshot(dir).map_err(|e| e.to_string())?;
        let files = first.files().map_err(|e| e.to_string())?;
        let assertion = an_assertion(&*first, &files);
        let (path, add, name) = planted(&files);
        let existing = first.read(path);
        drop(first);

        let mut judge = |name: &str, change: &dyn Fn(&dyn CleanCopy) -> AppResult<()>| {
            let scratch = self.checkouts.snapshot(dir).map_err(|e| e.to_string())?;
            change(&*scratch).map_err(|e| e.to_string())?;
            let base = self.checkouts.head(scratch.path());
            let v = self
                .verify(&VerifyRequest {
                    dir: scratch.path(),
                    spec,
                    base: base.as_deref(),
                    visible: false,
                    hidden: false,
                })
                .map_err(|e| e.to_string())?;
            let found = v.into_findings(spec, true);
            let caught = !found.clean();
            let detail = found
                .substrate
                .first()
                .or_else(|| found.tampering.first())
                .cloned()
                .unwrap_or_else(|| "the gate found nothing wrong".to_string());
            out.probes.push(Probe {
                name: name.to_string(),
                caught,
                detail,
            });
            Ok::<(), String>(())
        };

        judge(name, &|copy| {
            let mut bytes = existing.clone().unwrap_or_default();
            bytes.extend_from_slice(add.as_bytes());
            copy.write(path, &bytes)
        })?;
        match assertion {
            Some((test, text, at)) => {
                let label = format!("deleted an assertion from {test}");
                let kept: Vec<&str> = text
                    .lines()
                    .enumerate()
                    .filter(|(i, _)| *i != at)
                    .map(|(_, l)| l)
                    .collect();
                let cut = format!("{}\n", kept.join("\n"));
                judge(&label, &|copy| copy.write(&test, cut.as_bytes()))?;
            }
            None => out.skipped.push((
                "deleted an assertion".into(),
                "no test file with an assertion was found".into(),
            )),
        }
        Ok(out)
    }
}
