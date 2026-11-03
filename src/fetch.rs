//! `swarmux fetch`: gathers the facts the card shows and prints it. Reads
//! only; nothing is created on a machine swarmux has not run on.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use swarmux_application::Settings;
use swarmux_application::ledger::Ledger;
use swarmux_application::ports::{Clock, SessionRepository, TelemetryStore};
use swarmux_domain::ledger::hex;
use swarmux_domain::telemetry::UsageReport;
use swarmux_infrastructure::telemetry::CollectorConfig;
use swarmux_infrastructure::telemetry::localtime;
use swarmux_infrastructure::telemetry::transcripts::TranscriptIndex;
use swarmux_infrastructure::{Paths, SqliteLedger, SqliteStore, SystemClock, load_settings};
use swarmux_tui::fetch::{ColorChoice, Depth, FetchInfo, render, terminal_columns};

use crate::{doctor, evidence, theme};

/// Columns assumed when stdout is not a terminal.
const DEFAULT_COLUMNS: u16 = 120;
/// A model with less than this share of the spend is not worth a segment.
const MIN_SHARE: f64 = 0.005;
/// Models named in the bar before the rest are folded into one segment.
const MODELS_SHOWN: usize = 4;

/// Prints the card.
pub fn run(theme_name: Option<&str>, color: &str) -> anyhow::Result<()> {
    let choice = ColorChoice::parse(color)
        .ok_or_else(|| anyhow::anyhow!("--color must be auto, always or never"))?;
    let theme = theme::find(theme_name)?;
    let depth = Depth::from_env(choice, std::io::stdout().is_terminal());
    let paths = Paths::default_home()?;
    let settings = load_settings(&paths.config)?;
    let info = gather(&settings, &paths, theme.name);
    let width = terminal_columns().unwrap_or(DEFAULT_COLUMNS);
    print!("{}", render(&info, &theme, depth, width));
    Ok(())
}

/// Everything the card says. A fact that cannot be read is left out, not
/// guessed.
fn gather(settings: &Settings, paths: &Paths, theme: &str) -> FetchInfo {
    let now = SystemClock.now();
    let mut info = FetchInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        theme: theme.to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        claude: doctor::claude_version(&doctor::claude_bin()).map(|v| first_word(&v)),
        ..FetchInfo::default()
    };
    if paths.db.exists() {
        if let Ok(store) = SqliteStore::open(&paths.db) {
            info.sessions = store.list_active().map_or(0, |s| s.len());
            let (five, seven) = limits(&store, now);
            info.limit_5h = five;
            info.limit_7d = seven;
        }
        info.ledger_head = SqliteLedger::open(&paths.db)
            .ok()
            .and_then(|l| l.head().ok().flatten())
            .map(|e| hex(&e.hash).chars().take(8).collect());
        info.missions_delivered = evidence::delivered_count().unwrap_or(0);
    }
    if settings.telemetry.usage
        && let Some(report) = usage(settings, paths, now)
    {
        info.spend_today = Some(report.today.cost);
        info.burn_per_hour = report.burn.map(|b| b.usd_per_hour);
        info.agents_running = report.live_sessions;
        let (title, rows) = if report.models_today.is_empty() {
            ("spend by model", &report.models)
        } else {
            ("spend today by model", &report.models_today)
        };
        info.mix_title = title.to_string();
        info.mix = fold(rows.iter().map(|m| (m.name.clone(), m.share)).collect());
    }
    info
}

/// The newest reading of the 5-hour and 7-day windows, unless the window
/// has reset since.
fn limits(store: &SqliteStore, now: i64) -> (Option<f64>, Option<f64>) {
    let readings = store.limits().unwrap_or_default();
    let used = |window: &str| {
        readings
            .iter()
            .find(|(limit, _)| limit.window == window)
            .filter(|(limit, _)| limit.resets_at.is_none_or(|at| at > now))
            .and_then(|(limit, _)| limit.utilization)
    };
    (used("five_hour"), used("seven_day"))
}

/// Reads the transcripts the way `swarmux doctor` does.
fn usage(settings: &Settings, paths: &Paths, now: i64) -> Option<UsageReport> {
    let config = CollectorConfig::new(settings, paths);
    if !Path::new(&config.transcripts).exists() {
        return None;
    }
    let mut index = TranscriptIndex::open(&config.transcripts, config.usage_cache.as_deref());
    index.refresh(&AtomicBool::new(false)).ok()?;
    Some(
        index
            .book()
            .report(&settings.pricing, now, localtime::utc_offset_minutes()),
    )
}

/// The biggest `MODELS_SHOWN - 1` models and the rest as `other`.
fn fold(mut rows: Vec<(String, f64)>) -> Vec<(String, f64)> {
    rows.retain(|(_, share)| *share >= MIN_SHARE);
    if rows.len() <= MODELS_SHOWN {
        return rows;
    }
    let rest: f64 = rows
        .split_off(MODELS_SHOWN - 1)
        .iter()
        .map(|(_, s)| s)
        .sum();
    rows.push(("other".to_string(), rest));
    rows
}

/// `2.1.3 (Claude Code)` becomes `2.1.3`.
fn first_word(version: &str) -> String {
    version
        .split_whitespace()
        .next()
        .unwrap_or(version)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_beyond_the_fourth_are_folded_into_other() {
        let rows: Vec<(String, f64)> = (0..6)
            .map(|i| (format!("m{i}"), 0.3 - 0.04 * i as f64))
            .collect();
        let folded = fold(rows);
        assert_eq!(folded.len(), 4);
        assert_eq!(folded[3].0, "other");
        assert!((folded[3].1 - (0.18 + 0.14 + 0.10)).abs() < 1e-9);
        assert_eq!(fold(vec![("a".into(), 0.0)]), Vec::<(String, f64)>::new());
    }

    #[test]
    fn the_claude_version_is_its_first_word() {
        assert_eq!(first_word("2.1.3 (Claude Code)"), "2.1.3");
        assert_eq!(first_word("2.1.3"), "2.1.3");
    }
}
