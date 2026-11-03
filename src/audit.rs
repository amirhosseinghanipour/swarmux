//! `swarmux audit`: checks and shows the ledger.

use anyhow::Context;
use swarmux_application::ledger::{Ledger, VerifyReport, tail, verify_all};
use swarmux_domain::iso_minute;
use swarmux_domain::ledger::{LedgerEntry, hex};
use swarmux_infrastructure::{Paths, SqliteLedger};

/// Payload characters shown per `tail` line.
const PAYLOAD_PREVIEW: usize = 100;

fn open() -> anyhow::Result<SqliteLedger> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    Ok(SqliteLedger::open(&paths.db)?)
}

/// Walks the whole chain. Returns false when it is broken.
pub fn verify() -> anyhow::Result<bool> {
    let ledger = open()?;
    let report = verify_all(&ledger)?;
    println!("verified {} entries", report.entries);
    match report.head {
        Some((seq, hash)) => println!("head    #{seq} {}", hex(&hash)),
        None => println!("head    — (empty ledger)"),
    }
    match &report.broken {
        None => println!("OK"),
        Some(b) => println!("BROKEN at #{}: {}", b.seq, b.reason),
    }
    Ok(report.is_ok())
}

/// Prints the newest `n` entries, oldest first.
pub fn tail_cmd(n: usize) -> anyhow::Result<()> {
    let ledger = open()?;
    let entries = tail(&ledger, n)?;
    if entries.is_empty() {
        println!("the ledger is empty");
    }
    for e in &entries {
        println!("{}", line(e));
    }
    Ok(())
}

fn line(e: &LedgerEntry) -> String {
    let d = &e.draft;
    let job = d.job.map_or("—".to_string(), |j| format!("#{j}"));
    let mut payload: String = d.payload_json.chars().take(PAYLOAD_PREVIEW).collect();
    if payload.len() < d.payload_json.len() {
        payload.push('…');
    }
    format!(
        "{:>6}  {}  {:<14}  {:<14}  {:>5}  {}  {}",
        e.seq,
        iso_minute(d.ts),
        d.kind.as_str(),
        short(&d.actor, 14),
        job,
        &hex(&e.hash)[..12],
        payload
    )
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The `doctor` line for the ledger.
pub fn doctor(ledger: &dyn Ledger) {
    match verify_all(ledger) {
        Ok(VerifyReport {
            head: None,
            broken: None,
            ..
        }) => println!("ok   ledger empty (it starts with the first job)"),
        Ok(VerifyReport {
            entries,
            head: Some((seq, hash)),
            broken: None,
        }) => println!(
            "ok   ledger verified: {entries} entries, head #{seq} {}",
            hex(&hash)
        ),
        Ok(VerifyReport {
            broken: Some(b), ..
        }) => println!(
            "fail ledger chain broken at #{}: {} (swarmux audit verify)",
            b.seq, b.reason
        ),
        Err(e) => println!("fail ledger: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::JobId;
    use swarmux_domain::ledger::{LedgerDraft, LedgerKind};

    #[test]
    fn tail_line_shows_the_essentials() {
        let e = LedgerEntry::next(
            None,
            LedgerDraft {
                ts: 0,
                kind: LedgerKind::JobQueued,
                actor: "swarmux".into(),
                job: Some(JobId(42)),
                session: None,
                payload_json: format!(r#"{{"prompt":"{}"}}"#, "x".repeat(200)),
            },
        );
        let l = line(&e);
        assert!(l.starts_with("     1  1970-01-01 00:00  job_queued"), "{l}");
        assert!(l.contains("  #42  "), "{l}");
        assert!(l.ends_with('…'), "{l}");
        assert_eq!(short("abcdef", 4), "abc…");
        assert_eq!(short("abc", 4), "abc");
    }
}
