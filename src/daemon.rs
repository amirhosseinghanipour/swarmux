//! `swarmux daemon`: the background service that keeps the work going
//! when no window is open, and `swarmux` attaching a window to it.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use swarmux_application::daemon::{
    Answer, Ask, DaemonStatus, Hello, PROTOCOL, Screen, Shown, skew,
};
use swarmux_application::ports::Clock;
use swarmux_infrastructure::daemon::{
    Holder, LOG_CAP, LockError, ServiceLock, ServiceLog, WindowClient, WindowServer, ask, endpoint,
    holder, is_held, log_path, rotate, spawn_detached,
};
use swarmux_infrastructure::{Paths, SystemClock};
use swarmux_tui::remote::{Detached, Service, ServiceInfo, glyph_hints, window_size};

/// This build's version, compared with the service's on every attach.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How long a starting service has to answer, and a stopping one to go.
const START_WAIT: Duration = Duration::from_secs(20);
const STOP_WAIT: Duration = Duration::from_secs(30);

#[derive(clap::Subcommand)]
pub enum Action {
    /// Start the background service, detached from this terminal. `swarmux`
    /// starts it too when it isn't running.
    Start,
    /// Stop the background service. Nothing new starts; running jobs are
    /// stopped and resume at the next start, or finish first with --wait.
    Stop {
        /// Let running jobs finish first (this returns at once).
        #[arg(long)]
        wait: bool,
    },
    /// Whether the background service runs: its pid, version, uptime, jobs
    /// and attached windows.
    Status,
    /// Run the background service in this terminal, for debugging. Ctrl+C
    /// stops it as `swarmux daemon stop` does.
    Run,
}

pub fn run(action: Action) -> anyhow::Result<ExitCode> {
    let paths = data()?;
    match action {
        Action::Start => start(&paths),
        Action::Stop { wait } => stop(&paths, wait),
        Action::Status => status(&paths),
        Action::Run => serve(&paths),
    }
}

fn data() -> anyhow::Result<Paths> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    Ok(paths)
}

/// What is known of the service of a data folder.
enum Probe {
    Running(DaemonStatus),
    NotRunning,
    /// Something holds the lock but nothing answers.
    Silent(Holder),
}

fn probe(paths: &Paths) -> Probe {
    match ask(&endpoint(&paths.data_dir), Ask::Status) {
        Ok(Answer::Status(s)) => Probe::Running(s),
        _ if is_held(&paths.data_dir) => Probe::Silent(holder(&paths.data_dir)),
        _ => Probe::NotRunning,
    }
}

fn pid_text(h: &Holder) -> String {
    h.pid.map_or_else(|| "—".to_string(), |p| p.to_string())
}

/// The lock the engine takes when it runs in a window (`--no-daemon`):
/// never beside a background service.
pub fn engine_lock(paths: &Paths) -> anyhow::Result<ServiceLock> {
    match ServiceLock::acquire(&paths.data_dir, VERSION, SystemClock.now()) {
        Ok(lock) => Ok(lock),
        Err(LockError::Held(h)) => bail!(
            "swarmux already runs here (pid {}). `swarmux` attaches to the background \
             service; stop it with `swarmux daemon stop` to run without it",
            pid_text(&h)
        ),
        Err(e) => Err(e.into()),
    }
}

fn start(paths: &Paths) -> anyhow::Result<ExitCode> {
    match probe(paths) {
        Probe::Running(s) => {
            println!(
                "the background service already runs (pid {}, swarmux {})",
                s.pid, s.version
            );
            return Ok(ExitCode::SUCCESS);
        }
        Probe::Silent(h) => bail!(silent(&h)),
        Probe::NotRunning => {}
    }
    let s = launch(paths)?;
    println!(
        "started the background service (pid {}) · log {}",
        s.pid,
        log_path(&paths.data_dir).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn silent(h: &Holder) -> String {
    format!(
        "pid {} holds the background service's lock but does not answer. If it is stuck, \
         end that process, then try again",
        pid_text(h)
    )
}

/// Starts the service detached and waits until it answers.
fn launch(paths: &Paths) -> anyhow::Result<DaemonStatus> {
    let log = log_path(&paths.data_dir);
    let _ = rotate(&log, LOG_CAP);
    let exe = std::env::current_exe().context("cannot find the swarmux executable")?;
    spawn_detached(&exe, &["daemon", "run"], &paths.data_dir, &log)
        .context("cannot start the background service")?;
    let start = Instant::now();
    while start.elapsed() < START_WAIT {
        if let Probe::Running(s) = probe(paths) {
            return Ok(s);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let tail = std::fs::read_to_string(&log)
        .ok()
        .map(|t| {
            let lines: Vec<&str> = t.lines().collect();
            lines[lines.len().saturating_sub(5)..].join("\n")
        })
        .unwrap_or_default();
    bail!(
        "the background service did not answer within {} s. The end of {}:\n{tail}",
        START_WAIT.as_secs(),
        log.display()
    )
}

fn stop(paths: &Paths, wait: bool) -> anyhow::Result<ExitCode> {
    let name = endpoint(&paths.data_dir);
    let running = match ask(&name, Ask::Stop { wait }) {
        Ok(Answer::Stopping { running, .. }) => running,
        _ => match probe(paths) {
            Probe::Silent(h) => bail!(silent(&h)),
            _ => {
                println!("the background service is not running");
                return Ok(ExitCode::SUCCESS);
            }
        },
    };
    if wait && running > 0 {
        println!(
            "the background service stops once {} finished; nothing new starts",
            jobs(running, "running job has", "running jobs have")
        );
        return Ok(ExitCode::SUCCESS);
    }
    gone(paths)?;
    if running > 0 {
        println!(
            "stopped the background service · {} stopped; they resume at the next start",
            jobs(running, "job", "jobs")
        );
    } else {
        println!("stopped the background service");
    }
    Ok(ExitCode::SUCCESS)
}

/// Stops the service when it runs, as `daemon stop` does: before swarmux
/// is removed, its executable must not be in use.
pub fn stop_if_running() -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    match probe(&paths) {
        Probe::NotRunning => Ok(()),
        Probe::Silent(h) => bail!(silent(&h)),
        Probe::Running(_) => stop(&paths, false).map(drop),
    }
}

fn jobs(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Waits until nothing holds the service's lock.
fn gone(paths: &Paths) -> anyhow::Result<()> {
    let start = Instant::now();
    while start.elapsed() < STOP_WAIT {
        if !is_held(&paths.data_dir) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "the background service is still stopping after {} s; see {}",
        STOP_WAIT.as_secs(),
        log_path(&paths.data_dir).display()
    )
}

fn status(paths: &Paths) -> anyhow::Result<ExitCode> {
    match probe(paths) {
        Probe::Running(s) => {
            println!("swarmux daemon: running");
            println!("  pid        {}", s.pid);
            println!("  version    {}", s.version);
            println!("  up         {}", uptime(SystemClock.now() - s.started_at));
            println!("  jobs       {} running · {} queued", s.running, s.queued);
            println!("  windows    {} attached", s.windows);
            if s.draining {
                println!("  stopping   once the running jobs have finished");
            }
            println!("  log        {}", log_path(&paths.data_dir).display());
            Ok(ExitCode::SUCCESS)
        }
        Probe::NotRunning => {
            println!("swarmux daemon: not running · `swarmux` or `swarmux daemon start` starts it");
            Ok(ExitCode::FAILURE)
        }
        Probe::Silent(h) => {
            println!(
                "swarmux daemon: not answering (pid {} holds its lock)",
                pid_text(&h)
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

/// `2h 13m`, `4m`, `12s`.
fn uptime(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// The service itself: takes the lock, listens, runs the engine and the
/// windows attached until it is asked to stop.
fn serve(paths: &Paths) -> anyhow::Result<ExitCode> {
    let started_at = SystemClock.now();
    let lock = match ServiceLock::acquire(&paths.data_dir, VERSION, started_at) {
        Ok(lock) => lock,
        Err(LockError::Held(h)) => bail!(
            "swarmux already runs here (pid {}); `swarmux daemon status` shows it",
            pid_text(&h)
        ),
        Err(e) => return Err(e.into()),
    };
    let log = Say {
        log: ServiceLog::open(&log_path(&paths.data_dir), LOG_CAP),
        echo: std::io::stderr().is_terminal(),
    };
    let pid = std::process::id();
    log.line(
        started_at,
        &format!("swarmux {VERSION} service started (pid {pid})"),
    );
    let rt = crate::runtime()?;
    let name = endpoint(&paths.data_dir);
    let server = Arc::new(
        WindowServer::start(rt.handle(), &name)
            .with_context(|| format!("cannot listen on {name}"))?,
    );
    let interrupted = Arc::new(AtomicBool::new(false));
    rt.spawn(on_signal(interrupted.clone()));
    let mut app = crate::build_app(&rt, paths, false, Some(server.clone()))?;
    let mut service = Service::new(
        &mut app,
        ServiceInfo {
            pid,
            version: VERSION.to_string(),
            started_at,
        },
    );
    let finished_first = loop {
        if let Some(wait) = service.step(&mut app, &*server) {
            break wait;
        }
        for line in service.take_log() {
            log.line(SystemClock.now(), &line);
        }
        if interrupted.load(Ordering::Relaxed) {
            log.line(SystemClock.now(), "interrupted");
            break false;
        }
    };
    for line in service.take_log() {
        log.line(SystemClock.now(), &line);
    }
    if !finished_first {
        let stopped = app.engine_mut().stop_for_restart()?;
        if !stopped.is_empty() {
            log.line(
                SystemClock.now(),
                &format!(
                    "stopped {}; they resume at the next start",
                    jobs(stopped.len(), "job", "jobs")
                ),
            );
        }
    }
    // The engine goes first (its process trees with it), then the socket,
    // then the lock.
    drop(app);
    drop(server);
    rt.shutdown_timeout(Duration::from_secs(5));
    log.line(SystemClock.now(), "service stopped");
    drop(lock);
    Ok(ExitCode::SUCCESS)
}

/// The service's log, also shown in the terminal when it runs in one.
struct Say {
    log: ServiceLog,
    echo: bool,
}

impl Say {
    fn line(&self, now: i64, text: &str) {
        self.log.line(now, text);
        if self.echo {
            eprintln!("{text}");
        }
    }
}

/// Ctrl+C (and, on Unix, SIGTERM) stop the service as `daemon stop` does.
async fn on_signal(flag: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            flag.store(true, Ordering::Relaxed);
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    flag.store(true, Ordering::Relaxed);
}

/// `swarmux`: attaches this window to the background service, starting it
/// first when it isn't running.
pub fn attach(no_splash: bool) -> anyhow::Result<ExitCode> {
    let paths = data()?;
    for restarted in [false, true] {
        match probe(&paths) {
            Probe::Running(_) => {}
            Probe::NotRunning => {
                eprintln!("starting the background service…");
                launch(&paths)?;
            }
            Probe::Silent(h) => bail!(silent(&h)),
        }
        let (cols, rows) = window_size();
        let hello = Hello {
            version: VERSION.to_string(),
            protocol: PROTOCOL,
            cols,
            rows,
            glyphs: glyph_hints(),
            splash: !no_splash,
            cwd: std::env::current_dir()
                .ok()
                .map(|d| d.display().to_string()),
        };
        let name = endpoint(&paths.data_dir);
        let mut client =
            WindowClient::attach(&name, &hello).context("cannot reach the background service")?;
        let welcome = welcome(&mut client)?;
        if let Some(skew) = skew(VERSION, &welcome) {
            eprintln!("{}", skew.message);
            if skew.can_restart && !restarted && confirm("Restart it with this version now? [y/N] ")
            {
                drop(client);
                if let Ok(Answer::Stopping { .. }) = ask(&name, Ask::Stop { wait: false }) {
                    gone(&paths)?;
                }
                continue;
            }
            return Ok(ExitCode::FAILURE);
        }
        if !welcome.attached {
            bail!(
                "the background service did not take this window: {}",
                welcome.refused.as_deref().unwrap_or("—")
            );
        }
        return match swarmux_tui::remote::attach(&mut client, cols, rows)? {
            Detached::Bye(text) => {
                println!("{text}");
                Ok(ExitCode::SUCCESS)
            }
            Detached::Lost(why) => {
                eprintln!(
                    "lost the background service ({why}). `swarmux daemon status` says whether \
                     it runs; its log is {}",
                    log_path(&paths.data_dir).display()
                );
                Ok(ExitCode::FAILURE)
            }
        };
    }
    Ok(ExitCode::FAILURE)
}

/// The service's welcome, the first thing it sends.
fn welcome(client: &mut WindowClient) -> anyhow::Result<swarmux_application::daemon::Welcome> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        match client.recv(Duration::from_millis(100)) {
            Ok(Some(Shown::Welcome(w))) => return Ok(w),
            Ok(_) => {}
            Err(e) => bail!("the background service closed the line: {e}"),
        }
    }
    bail!("the background service did not say hello")
}

/// Asks a yes/no question on the terminal; no when there is none.
fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok()
        && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// `swarmux doctor`'s line about the service.
pub fn doctor_line(paths: &Paths) -> String {
    match probe(paths) {
        Probe::Running(s) if s.version == VERSION => format!(
            "ok   background service runs (pid {}, {} running, {} queued, {} windows)",
            s.pid, s.running, s.queued, s.windows
        ),
        Probe::Running(s) => format!(
            "warn background service runs swarmux {} (pid {}), this is {VERSION}: \
             `swarmux daemon stop`, then `swarmux` starts this version",
            s.version, s.pid
        ),
        Probe::NotRunning => {
            "info background service not running (`swarmux` starts it)".to_string()
        }
        Probe::Silent(h) => format!("fail {}", silent(&h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_reads_at_a_glance() {
        assert_eq!(uptime(12_000), "12s");
        assert_eq!(uptime(4 * 60_000 + 5_000), "4m");
        assert_eq!(uptime((2 * 3600 + 13 * 60) * 1000), "2h 13m");
        assert_eq!(uptime(3 * 86_400_000 + 3_600_000), "3d 1h");
        assert_eq!(uptime(-5), "0s");
    }
}
