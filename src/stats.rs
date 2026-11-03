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
