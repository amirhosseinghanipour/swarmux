//! The background service end to end: the real `swarmux` binary started
//! detached in a throwaway home, the fake `claude` as its agent, and
//! windows attached over the real local socket.
//!
//! Run with `cargo test --features fake-claude` from the workspace (the
//! infrastructure crate builds the fake). Every test stops the service it
//! started, also when it fails.
#![cfg(feature = "fake-claude")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use swarmux_application::daemon::{
    Answer, Ask, DaemonStatus, Frame, GlyphHints, Hello, KeyInput, KeyName, PROTOCOL, Screen,
    Shown, WindowInput,
};
use swarmux_infrastructure::daemon::{WindowClient, ask, endpoint, holder};

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A throwaway home with swarmux's data folder, a folder to work in and
/// the fake `claude`.
struct Home {
    dir: tempfile::TempDir,
    work: PathBuf,
    fake: PathBuf,
    /// Extra environment for the service (and so for its agents).
    env: Vec<(String, String)>,
}

fn home(env: &[(&str, String)]) -> Option<Home> {
    home_with(env, "")
}

/// Like [`home`], with more of `config.toml`.
fn home_with(env: &[(&str, String)], config: &str) -> Option<Home> {
    let fake =
        Path::new(EXE).with_file_name(format!("fake_claude{}", std::env::consts::EXE_SUFFIX));
    if !fake.is_file() {
        eprintln!("{} is not built; skipping", fake.display());
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join(".swarmux");
    std::fs::create_dir_all(&data).unwrap();
    // No toasts on the machine running the tests.
    std::fs::write(
        data.join("config.toml"),
        format!("[notify]\nenabled = false\n{config}"),
    )
    .unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/infrastructure/tests/fixtures/stream_basic.jsonl");
    let mut all = vec![(
        "FAKE_CLAUDE_FIXTURE".to_string(),
        fixture.display().to_string(),
    )];
    all.extend(env.iter().map(|(k, v)| (k.to_string(), v.clone())));
    Some(Home {
        dir,
        work,
        fake,
        env: all,
    })
}

impl Home {
    fn data(&self) -> PathBuf {
        self.dir.path().join(".swarmux")
    }

    fn endpoint(&self) -> String {
        endpoint(&self.data())
    }

    fn swarmux(&self, args: &[&str]) -> Output {
        Command::new(EXE)
            .args(args)
            .current_dir(&self.work)
            .env("USERPROFILE", self.dir.path())
            .env("HOME", self.dir.path())
            .env("SWARMUX_CLAUDE_BIN", &self.fake)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .output()
            .unwrap()
    }

    /// Starts the service; stopped again when the guard drops.
    fn start(&self) -> Running<'_> {
        let out = self.swarmux(&["daemon", "start"]);
        assert!(out.status.success(), "{}", text(&out));
        Running(self)
    }

    fn status(&self) -> Option<DaemonStatus> {
        match ask(&self.endpoint(), Ask::Status) {
            Ok(Answer::Status(s)) => Some(s),
            _ => None,
        }
    }

    /// Waits until the service's status passes `pred`.
    fn until(&self, what: &str, pred: impl Fn(&DaemonStatus) -> bool) -> DaemonStatus {
        let start = Instant::now();
        loop {
            if let Some(s) = self.status().filter(|s| pred(s)) {
                return s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "{what}: never came; last {:?}\n{}",
                self.status(),
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.data().join("daemon.log")).unwrap_or_default()
    }

    fn window(&self) -> Window {
        let mut w = Window::attach(&self.endpoint(), &self.work);
        w.log = self.data().join("daemon.log");
        w
    }
}

/// A started service, stopped when this drops.
struct Running<'a>(&'a Home);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let _ = self.0.swarmux(&["daemon", "stop"]);
        // A service that would not stop is ended, so no test leaves one.
        if let Some(pid) = holder(&self.0.data()).pid {
            kill(pid);
        }
    }
}

fn kill(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
    #[cfg(not(windows))]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// An attached window and the screen it was sent, as text.
struct Window {
    client: WindowClient,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<String>>,
    /// The service's log, shown when a wait fails.
    log: PathBuf,
}

impl Window {
    fn attach(name: &str, cwd: &Path) -> Window {
        let hello = Hello {
            version: VERSION.into(),
            protocol: PROTOCOL,
            cols: 120,
            rows: 40,
            glyphs: GlyphHints::default(),
            splash: false,
            cwd: Some(cwd.display().to_string()),
        };
        let mut w = Window {
            client: WindowClient::attach(name, &hello).unwrap(),
            cols: 120,
            rows: 40,
            cells: Vec::new(),
            log: PathBuf::new(),
        };
        let start = Instant::now();
        loop {
            match w.client.recv(Duration::from_millis(100)).unwrap() {
                Some(Shown::Welcome(welcome)) => {
                    assert!(welcome.attached, "{welcome:?}");
                    break;
                }
                Some(other) => w.take(other),
                None => assert!(start.elapsed() < Duration::from_secs(10), "no welcome"),
            }
        }
        w
    }

    fn take(&mut self, shown: Shown) {
        if let Shown::Frame(f) = shown {
            self.apply(&f);
        }
    }

    fn apply(&mut self, f: &Frame) {
        if f.full || self.cells.is_empty() {
            self.cols = usize::from(f.cols);
            self.rows = usize::from(f.rows);
            self.cells = vec![vec![" ".to_string(); self.cols]; self.rows];
        }
        for run in &f.runs {
            for (i, s) in run.cells.iter().enumerate() {
                let (x, y) = (usize::from(run.x) + i, usize::from(run.y));
                if x < self.cols && y < self.rows {
                    self.cells[y][x] = s.clone();
                }
            }
        }
    }

    fn screen(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn keys(&mut self, keys: &str) {
        for c in keys.chars() {
            self.key(KeyName::Char(c));
        }
    }

    fn key(&mut self, key: KeyName) {
        self.client
            .send(WindowInput::Key(KeyInput::plain(key)))
            .unwrap();
    }

    /// Takes frames until the screen no longer shows `what`.
    fn until_gone(&mut self, what: &str) -> String {
        let start = Instant::now();
        loop {
            while let Some(s) = self.client.recv(Duration::from_millis(50)).unwrap() {
                self.take(s);
            }
            let screen = self.screen();
            if !screen.contains(what) {
                return screen;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "still shows {what:?}:\n{screen}"
            );
        }
    }

    /// Takes frames until the screen shows `what`.
    fn until_shows(&mut self, what: &str) -> String {
        let start = Instant::now();
        loop {
            while let Some(s) = self.client.recv(Duration::from_millis(50)).unwrap() {
                self.take(s);
            }
            let screen = self.screen();
            if screen.contains(what) {
                return screen;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "never showed {what:?}:\n{screen}\n{}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
        }
    }
}

/// Acceptance: a window dispatches a multi-step job and leaves; the job
/// goes on and finishes with no window attached; a new window sees it
/// finished, and its ledger entries are there.
#[test]
fn work_goes_on_with_no_window_and_a_new_one_sees_it_done() {
    let Some(home) = home(&[("FAKE_CLAUDE_LINE_DELAY_MS", "400".into())]) else {
        return;
    };
    let _service = home.start();
    let s = home.status().unwrap();
    assert_eq!((s.version.as_str(), s.windows), (VERSION, 0));

    let mut w = home.window();
    w.keys("6n");
    w.keys("demo");
    w.key(KeyName::Enter);
    w.until_shows("demo");
    w.keys("1p");
    w.keys("write the weekly report");
    w.key(KeyName::Enter);
    home.until("the job starts", |s| s.running == 1);
    // The window leaves while the job is half way through its steps.
    w.key(KeyName::Char('q'));
    let bye = {
        let start = Instant::now();
        loop {
            match w.client.recv(Duration::from_millis(100)) {
                Ok(Some(Shown::Bye(text))) => break text,
                Ok(_) => assert!(start.elapsed() < Duration::from_secs(10), "no bye"),
                Err(e) => panic!("{e}"),
            }
        }
    };
    assert_eq!(
        bye,
        "1 job keeps running in the background · `swarmux daemon stop` to stop them"
    );
    drop(w);
    let s = home.until("the window is gone", |s| s.windows == 0);
    assert_eq!(s.running, 1, "the job still runs with no window");
    home.until("the job finishes", |s| s.running == 0 && s.queued == 0);

    let mut again = home.window();
    again.keys("4");
    let screen = again.until_shows("write the weekly report");
    assert!(screen.contains("done"), "{screen}");
    drop(again);

    let audit = home.swarmux(&["audit", "tail", "-n", "100"]);
    let audit = text(&audit);
    assert!(audit.contains("job"), "{audit}");
    assert!(
        audit.contains("done") || audit.contains("finished"),
        "{audit}"
    );
    let verify = home.swarmux(&["audit", "verify"]);
    assert!(verify.status.success(), "{}", text(&verify));
}

/// Acceptance: stopping the service mid-job and starting it again picks
/// the job up in its own Claude session, and it finishes.
#[test]
fn a_job_stopped_with_the_service_resumes_when_it_starts_again() {
    let dir = tempfile::tempdir().unwrap();
    let starts = dir.path().join("starts");
    let echo = dir.path().join("echo.json");
    let Some(home) = home(&[
        ("FAKE_CLAUDE_LINE_DELAY_MS", "700".into()),
        ("FAKE_CLAUDE_ARGS_FILE", starts.display().to_string()),
        ("FAKE_CLAUDE_ECHO_FILE", echo.display().to_string()),
    ]) else {
        return;
    };
    let service = home.start();
    let mut w = home.window();
    w.keys("6n");
    w.keys("demo");
    w.key(KeyName::Enter);
    w.until_shows("demo");
    w.keys("1p");
    w.keys("migrate the database");
    w.key(KeyName::Enter);
    home.until("the job starts", |s| s.running == 1);
    // Give the agent time to report its session, so it can be resumed.
    std::thread::sleep(Duration::from_millis(2500));
    drop(w);

    let out = home.swarmux(&["daemon", "stop"]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(
        said.contains("1 job stopped; they resume at the next start"),
        "{said}"
    );
    assert!(home.status().is_none());
    std::mem::forget(service);

    let _service = home.start();
    let s = home.until("the job resumes", |s| s.running == 1);
    assert_eq!(s.windows, 0);
    home.until("the resumed job finishes", |s| s.running == 0);
    let started = std::fs::read_to_string(&starts).unwrap();
    // The job's runs (`-p`), not other calls to `claude` such as the agent
    // catalog: started, then resumed once in the same session.
    let runs: Vec<&str> = started.lines().filter(|l| l.starts_with("-p ")).collect();
    assert_eq!(runs.len(), 2, "{started}");
    let last: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&echo).unwrap()).unwrap();
    let args: Vec<String> = last["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        args.iter().any(|a| a == "--resume"),
        "the same conversation goes on: {args:?}"
    );
    let log = home.log();
    assert!(
        log.contains("stopped 1 job; they resume at the next start"),
        "{log}"
    );
}

/// One service per data folder: a second start says so, a second `run`
/// refuses, and a pid file a crashed service left does not get in the way.
#[test]
fn one_service_per_data_folder_and_a_stale_lock_is_taken_over() {
    let Some(home) = home(&[]) else {
        return;
    };
    // A crash left its pid file behind; nobody holds the lock.
    std::fs::write(home.data().join("daemon.pid"), "4294967\n0.0.1\n1\n").unwrap();
    let _service = home.start();
    let s = home.status().unwrap();
    assert_ne!(s.pid, 4_294_967);
    assert_eq!(holder(&home.data()).pid, Some(s.pid));

    let again = home.swarmux(&["daemon", "start"]);
    assert!(again.status.success());
    assert!(text(&again).contains("already runs"), "{}", text(&again));
    let run = home.swarmux(&["daemon", "run"]);
    assert!(!run.status.success());
    assert!(text(&run).contains("already runs here"), "{}", text(&run));
    let here = home.swarmux(&["--no-daemon"]);
    assert!(!here.status.success());
    assert!(text(&here).contains("already runs here"), "{}", text(&here));
    assert_eq!(home.status().unwrap().pid, s.pid, "still the first one");

    let status = home.swarmux(&["daemon", "status"]);
    assert!(status.status.success());
    let shown = text(&status);
    assert!(shown.contains(&format!("pid        {}", s.pid)), "{shown}");
    assert!(shown.contains("0 running · 0 queued"), "{shown}");
    let doctor = text(&home.swarmux(&["doctor"]));
    assert!(
        doctor.contains(&format!("ok   background service runs (pid {}", s.pid)),
        "{doctor}"
    );
}

/// Acceptance: two windows attached; an agent's call waits for approval
/// in both; answered in one, it is gone from the other and the agent gets
/// the answer.
#[test]
fn an_approval_answered_in_one_window_is_gone_from_the_other() {
    let Some(home) = home_with(&[], "[guard]\nunmatched = \"ask\"\n") else {
        return;
    };
    let _service = home.start();
    let pid = home.status().unwrap().pid;
    let mut a = home.window();
    let mut b = home.window();
    home.until("both windows attach", |s| s.windows == 2);
    // An agent of the service asks to run something no rule covers.
    let pipe = swarmux_infrastructure::hook::endpoint_for(pid);
    let agent = std::thread::spawn(move || {
        let input = serde_json::json!({
            "session_id": "s",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "make deploy"},
            "cwd": "/w",
        })
        .to_string();
        swarmux_infrastructure::hook::pre_tool(&input, Some(&pipe), None)
    });
    a.until_shows("make deploy");
    b.until_shows("make deploy");
    a.keys("y");
    let other = b.until_gone("make deploy");
    assert!(!other.contains("approval"), "{other}");
    let answer = agent.join().unwrap().expect("the agent is answered");
    assert!(answer.contains("allow"), "{answer}");
}

#[test]
fn status_and_stop_when_nothing_runs() {
    let Some(home) = home(&[]) else {
        return;
    };
    let status = home.swarmux(&["daemon", "status"]);
    assert!(!status.status.success());
    assert!(text(&status).contains("not running"), "{}", text(&status));
    let stop = home.swarmux(&["daemon", "stop"]);
    assert!(stop.status.success());
    assert!(text(&stop).contains("not running"), "{}", text(&stop));
    let doctor = text(&home.swarmux(&["doctor"]));
    assert!(
        doctor.contains("background service not running"),
        "{doctor}"
    );
}
