//! Wiring for the telemetry stations and their `swarmux doctor` summary.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use swarmux_application::ports::{Clock, SessionRepository};
use swarmux_application::{Settings, TelemetryService};
use swarmux_domain::telemetry::{PriceTable, local_clock};
use swarmux_infrastructure::Paths;
use swarmux_infrastructure::telemetry::cli::{self, CliError};
use swarmux_infrastructure::telemetry::transcripts::TranscriptIndex;
use swarmux_infrastructure::telemetry::{
    CollectorConfig, TelemetryCollector, battery, github, localtime, system::SystemCollector,
};

const GB: f64 = (1u64 << 30) as f64;

/// Starts the background collectors and the service the stations read from.
pub fn start(
    settings: &Settings,
    paths: &Paths,
    sessions: Rc<dyn SessionRepository>,
    clock: Rc<dyn Clock>,
) -> TelemetryService {
    let collector = TelemetryCollector::start(CollectorConfig::new(settings, paths));
    TelemetryService::new(Rc::new(collector), sessions, clock)
}

/// Prints what each telemetry source sees, and how long indexing took.
pub fn doctor(settings: &Settings, paths: &Paths, now: i64) {
    let config = CollectorConfig::new(settings, paths);
    let t = &settings.telemetry;
    if t.usage {
        usage_summary(
            &config.transcripts,
            config.usage_cache.as_deref(),
            &settings.pricing,
            now,
        );
    } else {
        println!("ok   usage telemetry off ([telemetry] usage = false)");
    }
    tool_check("git", &config.git, &["--version"], t.git);
    if t.github {
        match cli::run(
            &config.gh,
            &["auth", "status"],
            None,
            Duration::from_secs(15),
        ) {
            Ok(_) => println!("ok   gh is logged in: pull requests and CI are shown"),
            Err(e) => {
                let e = github::explain(e);
                let hint = e.hint.unwrap_or_else(|| "run `gh auth status`".into());
                println!("warn gh unavailable ({}): {hint}", e.message);
            }
        }
    } else {
        println!("ok   GitHub telemetry off ([telemetry] github = false)");
    }
    if t.system {
        system_summary(&config.home);
    } else {
        println!("ok   system telemetry off ([telemetry] system = false)");
    }
}

fn usage_summary(root: &Path, cache: Option<&Path>, prices: &PriceTable, now: i64) {
    let started = Instant::now();
    let mut index = TranscriptIndex::open(root, cache);
    let loaded = started.elapsed();
    let stats = match index.refresh(&AtomicBool::new(false)) {
        Ok(stats) => stats,
        Err(e) => {
            return println!(
                "warn transcripts {}: {e} (set [telemetry] transcripts_path)",
                root.display()
            );
        }
    };
    let took = started.elapsed();
    let offset = localtime::utc_offset_minutes();
    let r = index.book().report(prices, now, offset);
    println!(
        "ok   transcripts {}: {} files, indexed in {:.2} s (cache load {:.2} s, {:.1} MB new), {} messages",
        root.display(),
        stats.files,
        took.as_secs_f64(),
        loaded.as_secs_f64(),
        stats.bytes_read as f64 / 1e6,
        r.messages
    );
    if let Some(e) = &index.cache_error {
        println!("warn {e}");
    }
    println!(
        "ok   sessions found: {} ({} live in the last 2 minutes)",
        r.sessions_total, r.live_sessions
    );
    println!(
        "ok   spend today ${:.2} ({} messages), this week ${:.2}, cache hit today {}",
        r.today.cost,
        r.today.messages,
        r.week.cost,
        r.cache_hit_ratio_today
            .map_or("—".to_string(), |c| format!("{:.0}%", c * 100.0))
    );
    match (&r.active_block, r.burn) {
        (Some(b), Some(burn)) => println!(
            "ok   burn ${:.2}/h, {:.0} tokens/min; this 5-hour block ${:.2} so far, ends {}",
            burn.usd_per_hour,
            burn.tokens_per_minute,
            b.cost,
            local_clock(b.end, offset)
        ),
        (Some(b), None) => println!(
            "ok   5-hour block ${:.2} so far, ends {} (too early for a burn rate)",
            b.cost,
            local_clock(b.end, offset)
        ),
        _ => println!("ok   no active 5-hour block"),
    }
    if !r.unpriced_models.is_empty() {
        println!(
            "warn no price for {}: add them under [pricing]",
            r.unpriced_models.join(", ")
        );
    }
}

fn tool_check(name: &str, bin: &Path, args: &[&str], enabled: bool) {
    if !enabled {
        return println!("ok   {name} telemetry off ([telemetry] {name} = false)");
    }
    match cli::run(bin, args, None, Duration::from_secs(10)) {
        Ok(out) => println!("ok   {}", out.lines().next().unwrap_or(name).trim()),
        Err(CliError::NotFound) => println!("warn {name} not found: repository state is hidden"),
        Err(e) => println!("warn {name}: {e}"),
    }
}

fn system_summary(home: &Path) {
    let Some(mut c) = SystemCollector::new(home) else {
        return println!("warn system readings unavailable on this machine");
    };
    let started = Instant::now();
    c.collect();
    std::thread::sleep(Duration::from_millis(300));
    let s = c.collect();
    let took = started.elapsed().saturating_sub(Duration::from_millis(300));
    println!(
        "ok   system: {} cores at {:.0}%, RAM {:.1}/{:.1} GB, uptime {} h (read in {} ms)",
        s.cores.len(),
        s.cpu,
        s.mem_used as f64 / GB,
        s.mem_total as f64 / GB,
        s.uptime_secs / 3600,
        took.as_millis()
    );
    match &s.disk {
        Some(d) => println!(
            "ok   disk {}: {:.0}/{:.0} GB used",
            d.mount,
            d.used as f64 / GB,
            d.total as f64 / GB
        ),
        None => println!("warn disk usage of the home folder unknown"),
    }
    match battery::read() {
        Some(b) => println!(
            "ok   battery {:.0}%{}",
            b.percent,
            if b.charging {
                ", charging"
            } else if b.plugged_in {
                ", plugged in"
            } else {
                ""
            }
        ),
        None => println!("ok   no battery"),
    }
    println!(
        "ok   claude processes running: {}",
        s.claude_processes.len()
    );
}
