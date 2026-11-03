//! `swarmux verify` as a user runs it: the real binary, a real repository
//! and a real database, with the checks re-run in a clean checkout.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::rc::Rc;

use swarmux_application::ledger::LedgerService;
use swarmux_application::ports::NewJob;
use swarmux_application::{EvidenceStore, HashSigner, Signer};
use swarmux_domain::evidence::{BUNDLE_SCHEMA, Bundle, CheckResult, LedgerRange};
use swarmux_domain::ledger::hex;
use swarmux_domain::{JobId, SessionId};
use swarmux_infrastructure::{Paths, SqliteLedger, SqliteStore, SystemClock};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A repository whose first commit has `ok.txt` and whose second removed it.
struct World {
    home: tempfile::TempDir,
    repo: tempfile::TempDir,
    with_file: String,
    without_file: String,
}

fn world() -> Option<World> {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    git(p, &["init", "-q", "-b", "main"])?;
    std::fs::write(p.join("ok.txt"), "fine\n").unwrap();
    git(p, &["add", "ok.txt"])?;
    git(p, &["commit", "-q", "-m", "with the file"])?;
    let with_file = git(p, &["rev-parse", "HEAD"])?;
    git(p, &["rm", "-q", "ok.txt"])?;
    git(p, &["commit", "-q", "-m", "without the file"])?;
    let without_file = git(p, &["rev-parse", "HEAD"])?;
    Some(World {
        home: tempfile::tempdir().unwrap(),
        repo,
        with_file,
        without_file,
    })
}

fn check(passed: bool) -> CheckResult {
    CheckResult {
        name: "ok.txt".into(),
        program: "git".into(),
        args: ["cat-file", "-e", "HEAD:ok.txt"].map(String::from).to_vec(),
        timeout_secs: 60,
        exit: Some(if passed { 0 } else { 128 }),
        passed,
        flaky: false,
        duration_ms: 12,
        first_failure: String::new(),
    }
}

impl World {
    /// Seals a bundle for `slug` that says the check `passed` at `head`,
    /// with the ledger and database a real run would have left.
    fn seal(&self, slug: &str, head: &str, passed: bool) -> PathBuf {
        let paths = Paths::from_home(self.home.path());
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let clock = Rc::new(SystemClock);
        let ledger = LedgerService::new(Rc::new(SqliteLedger::open(&paths.db).unwrap()), clock);
        let job = JobId(1);
        ledger
            .record_job_queued(
                job,
                &NewJob {
                    session_id: SessionId::random(),
                    prompt_id: None,
                    prompt: "do it".into(),
                    model: "sonnet".into(),
                    priority: 0,
                    fork: false,
                    wants_review: false,
                    review_of: None,
                },
                1,
            )
            .unwrap();
        let tail = swarmux_application::ledger::tail(ledger.ledger(), 1).unwrap();
        let last = tail.last().unwrap();
        let mut bundle = Bundle {
            schema: BUNDLE_SCHEMA,
            job: job.0,
            attempt: 1,
            repo: self.repo.path().display().to_string(),
            head_sha: head.into(),
            checks: vec![check(passed)],
            ledger: LedgerRange {
                from_seq: last.seq,
                to_seq: last.seq,
                head_hash: hex(&last.hash),
            },
            sealed_at: 1,
            ..Bundle::default()
        };
        bundle.contract.slug = slug.into();
        bundle.contract.title = slug.into();
        bundle.contract.root = self.repo.path().display().to_string();
        bundle.signature = Some(HashSigner.sign(&bundle.digest()));
        let seq = ledger
            .record_evidence(slug, Some(job), &bundle.digest_hex(), head, last.seq, 2)
            .unwrap();
        let store = SqliteStore::open(&paths.db).unwrap();
        store.save(&bundle, Some(seq), 2).unwrap();
        let file = self.home.path().join(format!("{slug}.json"));
        std::fs::write(&file, bundle.to_json()).unwrap();
        file
    }

    fn run(&self, args: &[&str]) -> Output {
        let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        Command::new(EXE)
            .args(args)
            .env(var, self.home.path())
            .output()
            .unwrap()
    }

    fn worktrees(&self) -> usize {
        git(self.repo.path(), &["worktree", "list"]).map_or(0, |s| s.lines().count())
    }
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn verify_matches_when_the_checks_hold_at_head() {
    let Some(w) = world() else {
        return eprintln!("git not available; skipping");
    };
    w.seal("held", &w.with_file, true);
    let out = w.run(&["verify", "held"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.trim_end().ends_with("match"), "{said}");
    assert!(said.contains("ok       clean checkout"), "{said}");
    assert!(said.contains("ok       check ok.txt: pass"), "{said}");
    assert!(said.contains("ok       ledger chain"), "{said}");
    assert!(said.contains("ok       signature"), "{said}");
    // The working folder is at the second commit, without the file: only a
    // clean checkout of the first could pass. And it was cleaned up.
    assert!(!w.repo.path().join("ok.txt").exists());
    assert_eq!(w.worktrees(), 1);
}

#[test]
fn verify_reports_a_mismatch_and_exits_1() {
    let Some(w) = world() else {
        return eprintln!("git not available; skipping");
    };
    // The bundle claims the check passed at the commit where the file is gone.
    w.seal("lied", &w.without_file, true);
    let out = w.run(&["verify", "lied"]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(
        said.contains("MISMATCH check ok.txt: expected pass, found fail"),
        "{said}"
    );
    assert!(said.trim_end().ends_with("mismatch"), "{said}");
    assert_eq!(w.worktrees(), 1);
}

#[test]
fn verify_reads_a_bundle_file_and_catches_an_edit() {
    let Some(w) = world() else {
        return eprintln!("git not available; skipping");
    };
    let file = w.seal("filed", &w.with_file, true);
    let out = w.run(&["verify", &file.display().to_string()]);
    assert!(out.status.success(), "{}", text(&out));

    // Someone edits the file: the digest no longer matches.
    let edited = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\"attempt\":1", "\"attempt\":7");
    std::fs::write(&file, edited).unwrap();
    let out = w.run(&["verify", &file.display().to_string()]);
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "{said}");
    assert!(said.contains("MISMATCH signature"), "{said}");
    assert!(said.contains("MISMATCH stored digest"), "{said}");
}

#[test]
fn verify_of_an_unknown_mission_says_so() {
    let Some(w) = world() else {
        return eprintln!("git not available; skipping");
    };
    let out = w.run(&["verify", "nope"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("no bundle for"), "{err}");
}
