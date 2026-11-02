//! Background telemetry collectors behind the `TelemetryFeed` port.
//!
//! Each source runs on its own thread at its own cadence and writes into a
//! shared snapshot; [`TelemetryCollector::snapshot`] only clones it. A source
//! that fails keeps its last data and reports the error; nothing panics.

pub mod battery;
pub mod cli;
pub mod github;
pub mod localtime;
pub mod repo;
pub mod system;
pub mod transcripts;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use swarmux_application::ports::{Clock, TelemetryFeed};
use swarmux_application::{Collected, Settings, TelemetrySettings, TelemetrySnapshot};
use swarmux_domain::telemetry::PriceTable;

use crate::{Paths, SystemClock};
use cli::CliError;
use system::SystemCollector;
use transcripts::TranscriptIndex;

/// Where and how the collectors work.
#[derive(Clone, Debug)]
pub struct CollectorConfig {
    pub settings: TelemetrySettings,
    pub pricing: PriceTable,
    /// The transcripts folder (`~/.claude/projects` unless configured).
    pub transcripts: PathBuf,
    /// The read-position cache; `None` reads everything on each start.
    pub usage_cache: Option<PathBuf>,
    pub home: PathBuf,
    pub git: PathBuf,
    pub gh: PathBuf,
}

impl CollectorConfig {
    pub fn new(settings: &Settings, paths: &Paths) -> CollectorConfig {
        CollectorConfig {
            settings: settings.telemetry.clone(),
            pricing: settings.pricing.clone(),
            transcripts: settings
                .telemetry
                .transcripts_path
                .clone()
                .unwrap_or_else(|| default_transcripts(&paths.home)),
            usage_cache: Some(paths.data_dir.join("usage.db")),
            home: paths.home.clone(),
            git: "git".into(),
            gh: "gh".into(),
        }
    }
}

/// `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
pub fn default_transcripts(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".claude"), PathBuf::from)
        .join("projects")
}

struct Shared {
    snap: Mutex<TelemetrySnapshot>,
    /// The focused folder and how often it changed.
    focus: Mutex<(u64, Option<PathBuf>)>,
    stop: AtomicBool,
    bell: Condvar,
    bell_lock: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A collector that panicked mid-update leaves usable data behind.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut TelemetrySnapshot)) {
        f(&mut lock(&self.snap));
    }

    fn focus(&self) -> (u64, Option<PathBuf>) {
        lock(&self.focus).clone()
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Waits `every`, or less when stopping or (with `on_focus`) when the
    /// focus changes.
    fn sleep(&self, every: Duration, on_focus: Option<u64>) {
        let deadline = Instant::now() + every;
        let mut guard = lock(&self.bell_lock);
        loop {
            if self.stopped() || on_focus.is_some_and(|g| self.focus().0 != g) {
                return;
            }
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            guard = match self.bell.wait_timeout(guard, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
    }

    fn ring(&self) {
        let _guard = lock(&self.bell_lock);
        self.bell.notify_all();
    }
}

/// An enabled source waiting for its first reading, or a disabled one.
fn pending<T>(on: bool) -> Collected<T> {
    if on {
        Collected::pending()
    } else {
        Collected::default()
    }
}

/// Runs the enabled collectors until dropped.
pub struct TelemetryCollector {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl TelemetryCollector {
    pub fn start(config: CollectorConfig) -> TelemetryCollector {
        let s = &config.settings;
        let snap = TelemetrySnapshot {
            offset_min: localtime::utc_offset_minutes(),
            usage: pending(s.usage),
            system: pending(s.system),
            git: pending(s.git),
            github: pending(s.github),
            ..TelemetrySnapshot::default()
        };
        let shared = Arc::new(Shared {
            snap: Mutex::new(snap),
            focus: Mutex::new((0, None)),
            stop: AtomicBool::new(false),
            bell: Condvar::new(),
            bell_lock: Mutex::new(()),
        });
        let mut threads = Vec::new();
        let secs = Duration::from_secs;
        if s.usage {
            threads.extend(spawn(
                &shared,
                "usage",
                secs(s.usage_secs),
                false,
                usage_worker(&config),
            ));
        }
        if s.system {
            threads.extend(spawn(
                &shared,
                "system",
                secs(s.system_secs),
                false,
                system_worker(&config),
            ));
        }
        if s.git {
            threads.extend(spawn(
                &shared,
                "git",
                secs(s.git_secs),
                true,
                git_worker(&config),
            ));
        }
        if s.github {
            threads.extend(spawn(
                &shared,
                "github",
                secs(s.github_secs),
                true,
                github_worker(&config),
            ));
        }
        TelemetryCollector { shared, threads }
    }
}

impl TelemetryFeed for TelemetryCollector {
    fn snapshot(&self) -> TelemetrySnapshot {
        lock(&self.shared.snap).clone()
    }

    fn focus(&self, cwd: &Path) {
        {
            let mut f = lock(&self.shared.focus);
            if f.1.as_deref() == Some(cwd) {
                return;
            }
            *f = (f.0 + 1, Some(cwd.to_path_buf()));
        }
        self.shared.ring();
    }
}

impl Drop for TelemetryCollector {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.ring();
        for t in self.threads.drain(..) {
            // A collector that panicked has nothing left to clean up.
            let _ = t.join();
        }
    }
}

type Worker = Box<dyn FnMut(&Shared) + Send>;

/// Runs `work` every `every` on a thread of its own.
fn spawn(
    shared: &Arc<Shared>,
    name: &str,
    every: Duration,
    on_focus: bool,
    mut work: Worker,
) -> Option<JoinHandle<()>> {
    let shared = shared.clone();
    std::thread::Builder::new()
        .name(format!("swarmux-telemetry-{name}"))
        .spawn(move || {
            while !shared.stopped() {
                let generation = shared.focus().0;
                work(&shared);
                shared.sleep(every, on_focus.then_some(generation));
            }
        })
        .ok()
}

fn now() -> i64 {
    SystemClock.now()
}

fn elapsed_ms(since: Instant) -> Option<u64> {
    Some(since.elapsed().as_millis() as u64)
}

fn usage_worker(config: &CollectorConfig) -> Worker {
    let root = config.transcripts.clone();
    let cache = config.usage_cache.clone();
    let pricing = config.pricing.clone();
    let mut index: Option<TranscriptIndex> = None;
    Box::new(move |shared: &Shared| {
        let started = Instant::now();
        let index = index.get_or_insert_with(|| TranscriptIndex::open(&root, cache.as_deref()));
        let result = index.refresh(&shared.stop);
        let offset = localtime::utc_offset_minutes();
        let at = now();
        let report = result
            .as_ref()
            .ok()
            .map(|_| index.book().report(&pricing, at, offset));
        let cache_error = index.cache_error.clone();
        shared.update(|snap| {
            snap.offset_min = offset;
            let u = &mut snap.usage;
            u.took_ms = elapsed_ms(started);
            match (result, report) {
                (Ok(_), Some(report)) => {
                    u.data = Some(report);
                    u.updated_at = Some(at);
                    u.error = cache_error;
                    u.hint = None;
                }
                (Err(e), _) => {
                    u.error = Some(format!("transcripts {}: {e}", root.display()));
                    u.hint = Some("set [telemetry] transcripts_path in config.toml".into());
                }
                (Ok(_), None) => {}
            }
        });
    })
}

fn system_worker(config: &CollectorConfig) -> Worker {
    let home = config.home.clone();
    let mut collector: Option<SystemCollector> = None;
    let mut tried = false;
    Box::new(move |shared: &Shared| {
        if !tried {
            collector = SystemCollector::new(&home);
            tried = true;
        }
        let started = Instant::now();
        let stats = collector.as_mut().map(SystemCollector::collect);
        let at = now();
        shared.update(|snap| {
            let s = &mut snap.system;
            s.took_ms = elapsed_ms(started);
            match stats {
                Some(stats) => {
                    s.data = Some(stats);
                    s.updated_at = Some(at);
                    s.error = None;
                }
                None => s.error = Some("this system can't be queried".into()),
            }
        });
    })
}

fn git_worker(config: &CollectorConfig) -> Worker {
    let git = config.git.clone();
    let mut last: Option<PathBuf> = None;
    Box::new(move |shared: &Shared| {
        let (_, focus) = shared.focus();
        let Some(cwd) = focus else {
            shared.update(|snap| {
                snap.git.hint = Some("select a session to follow its folder".into())
            });
            return;
        };
        let moved = last.as_deref() != Some(cwd.as_path());
        last = Some(cwd.clone());
        let started = Instant::now();
        let result = repo::status(&git, &cwd);
        let at = now();
        shared.update(|snap| {
            let g = &mut snap.git;
            g.took_ms = elapsed_ms(started);
            if moved {
                g.data = None;
            }
            match result {
                Ok(status) => {
                    g.data = Some(status);
                    g.updated_at = Some(at);
                    g.error = None;
                    g.hint = None;
                }
                Err(e) => {
                    g.data = None;
                    g.hint = match &e {
                        CliError::NotFound => Some("install git to see repository state".into()),
                        CliError::Failed(m) if m.contains("not a git repository") => {
                            Some("the session folder is not a git repository".into())
                        }
                        _ => None,
                    };
                    g.error = Some(format!("git: {e}"));
                }
            }
        });
    })
}

fn github_worker(config: &CollectorConfig) -> Worker {
    let gh = config.gh.clone();
    let mut last: Option<PathBuf> = None;
    Box::new(move |shared: &Shared| {
        let (_, focus) = shared.focus();
        let Some(cwd) = focus else {
            return;
        };
        let moved = last.as_deref() != Some(cwd.as_path());
        last = Some(cwd.clone());
        // The branch git found for this folder, when it has looked already.
        let branch = {
            let snap = lock(&shared.snap);
            snap.git
                .data
                .as_ref()
                .filter(|g| Path::new(&g.cwd) == cwd)
                .and_then(|g| g.branch.clone())
        };
        let started = Instant::now();
        let result = github::status(&gh, &cwd, branch.as_deref());
        let at = now();
        shared.update(|snap| {
            let h = &mut snap.github;
            h.took_ms = elapsed_ms(started);
            if moved {
                h.data = None;
            }
            match result {
                Ok(status) => {
                    h.data = Some(status);
                    h.updated_at = Some(at);
                    h.error = None;
                    h.hint = None;
                }
                Err(e) => {
                    h.data = None;
                    h.error = Some(format!("gh: {}", e.message));
                    h.hint = e.hint;
                }
            }
        });
    })
}

#[cfg(test)]
mod tests;
