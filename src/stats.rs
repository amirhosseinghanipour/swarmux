//! `swarmux stats alarms`: how the inbox has been doing, from its alarm log.

use anyhow::Context;
use swarmux_application::alarm_report;
use swarmux_application::ports::Clock;
use swarmux_infrastructure::{Paths, SqliteStore, SystemClock};

/// The alarm key figures of the last `days` days.
pub fn alarms(days: i64) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = SqliteStore::open(&paths.db)?;
    let now = SystemClock.now();
    let k = alarm_report(&store, now, days)?;
    let raised: usize = k.distribution.iter().sum();
    println!(
        "alarms, last {days} day{}",
        if days == 1 { "" } else { "s" }
    );
    if raised == 0 {
        println!("  nothing was raised");
        return Ok(());
    }
    let [p1, p2, p3] = k.shares();
    println!("  raised      {raised}");
    println!(
        "  priorities  P1 {} ({p1:.0}%)  P2 {} ({p2:.0}%)  P3 {} ({p3:.0}%)   target 5 / 15 / 80",
        k.distribution[0], k.distribution[1], k.distribution[2]
    );
    println!(
        "  urgent rate {:.1} P1+P2 per 10 min   target at most 1",
        k.urgent_avg_10min
    );
    println!(
        "  peak        {} items in 10 min   a flood is {} or more",
        k.peak_10min,
        swarmux_domain::alarm::FLOOD_AT
    );
    match k.ttd_median {
        Some(ms) => println!(
            "  time to look  median {}s over {} items",
            ms / 1000,
            k.attended
        ),
        None => println!("  time to look  no item was looked at yet"),
    }
    Ok(())
}

/// `swarmux stats cache`: how much of what the agents read came from the
/// prompt cache over the last `days` days.
pub fn cache(days: i64) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    let store = SqliteStore::open(&paths.db)?;
    let report = swarmux_application::cache::cache_report(&store, SystemClock.now(), days)?;
    let pct = |r: Option<f64>| r.map_or_else(|| "—".to_string(), |r| format!("{:.0}%", r * 100.0));
    println!(
        "prompt cache, last {} day{}",
        report.days,
        if report.days == 1 { "" } else { "s" }
    );
    let all = report.all;
    if all.jobs == 0 {
        println!("  no finished jobs");
        return Ok(());
    }
    println!(
        "  cache-read  {}   of input, over {} of {} jobs that reported it",
        pct(all.ratio()),
        all.known,
        all.jobs
    );
    println!(
        "  tokens      {} read from cache, {} written to it, {} not cached",
        all.cache_read, all.cache_creation, all.input
    );
    for (model, t) in &report.models {
        println!(
            "  {:<24} {:>5}   {} job{}",
            model,
            pct(t.ratio()),
            t.jobs,
            if t.jobs == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// `swarmux stats verification`: how verification has gone over the last
/// hours (`[console] metrics_hours` unless `--hours` says), worked out from
/// the ledger, as the Work screen's footer shows it.
pub fn verification(hours: Option<u32>) -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let hours = match hours {
        Some(h) => h,
        None => Ok::<_, std::convert::Infallible>(swarmux_infrastructure::load_effective_settings(
            &paths,
        ))
        .map(|s| s.console.metrics_hours)
        .unwrap_or(swarmux_application::metrics::DEFAULT_HOURS),
    }
    .clamp(1, swarmux_application::metrics::MAX_HOURS);
    let ledger = swarmux_infrastructure::SqliteLedger::open(&paths.db)?;
    let window = i64::from(hours) * 3_600_000;
    let m = swarmux_application::metrics::read_metrics(&ledger, SystemClock.now(), window)?;
    for line in describe(&m, hours) {
        println!("{line}");
    }
    Ok(())
}

/// The figures as lines of text; a figure with nothing behind it says so
/// instead of showing zero.
fn describe(m: &swarmux_domain::console::Metrics, hours: u32) -> Vec<String> {
    let secs = |ms: u64| match ms / 1000 {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    };
    let none = "—";
    let mut out = vec![format!(
        "verification, last {hours} hour{} ({} ledger entries{})",
        if hours == 1 { "" } else { "s" },
        m.entries,
        if m.complete {
            ""
        } else {
            ", the oldest of the window not read"
        }
    )];
    out.push(format!(
        "  first-run gate passes  {}",
        m.first_run.map_or_else(
            || none.to_string(),
            |f| format!(
                "{:.0}%  ({} of {} jobs)",
                f.rate() * 100.0,
                f.passed,
                f.judged
            )
        )
    ));
    out.push(format!(
        "  claim to verified      {}",
        m.claim_to_verified.map_or_else(
            || none.to_string(),
            |s| format!(
                "median {}, p90 {}  (over {} verified claims)",
                secs(s.median_ms),
                secs(s.p90_ms),
                s.samples
            )
        )
    ));
    out.push(format!(
        "  verified               {}{}",
        m.verified,
        m.verified_per_hour
            .map_or_else(String::new, |r| format!("  ({r:.2} an hour)"))
    ));
    out.push(format!("  checks run             {}", m.checks_run));
    out.push(format!("  bundles sealed         {}", m.bundles_sealed));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::console::{FirstRun, Metrics, Spread};

    #[test]
    fn figures_with_nothing_behind_them_say_so_instead_of_zero() {
        let m = Metrics::none(1_000_000, 24 * 3_600_000);
        let text = describe(&m, 24).join("\n");
        assert!(text.contains("first-run gate passes  —"), "{text}");
        assert!(text.contains("claim to verified      —"), "{text}");
        assert!(text.contains("verified               0\n"), "{text}");
        assert!(!text.contains("an hour"), "{text}");
    }

    #[test]
    fn figures_are_written_with_their_samples() {
        let mut m = Metrics::none(0, 3_600_000);
        m.first_run = Some(FirstRun {
            passed: 3,
            judged: 4,
        });
        m.claim_to_verified = Some(Spread {
            median_ms: 130_000,
            p90_ms: 560_000,
            samples: 4,
        });
        m.verified = 4;
        m.verified_per_hour = Some(4.0);
        m.checks_run = 17;
        m.bundles_sealed = 2;
        m.complete = false;
        let text = describe(&m, 1).join("\n");
        for needle in [
            "last 1 hour ",
            "the oldest of the window not read",
            "75%  (3 of 4 jobs)",
            "median 2m10s, p90 9m20s  (over 4 verified claims)",
            "4  (4.00 an hour)",
            "checks run             17",
            "bundles sealed         2",
        ] {
            assert!(text.contains(needle), "{needle}\n{text}");
        }
    }
}
