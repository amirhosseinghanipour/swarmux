//! `swarmux audit`: checks and shows the ledger.

use std::path::Path;
use std::rc::Rc;

use anyhow::Context;
use swarmux_application::anchor::AnchorService;
use swarmux_application::ledger::{Ledger, VerifyReport, tail, verify_all};
use swarmux_domain::anchor::Checkpoint;
use swarmux_domain::iso_minute;
use swarmux_domain::ledger::{LedgerEntry, hex};
use swarmux_infrastructure::{MinisignSigner, Paths, SqliteLedger, SqliteStore, SystemClock};

/// Payload characters shown per `tail` line.
const PAYLOAD_PREVIEW: usize = 100;

fn open() -> anyhow::Result<SqliteLedger> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    Ok(SqliteLedger::open(&paths.db)?)
}

/// The anchoring service on the database `paths` names: checkpoints are
/// kept in the same database, the key in the OS keychain (or a file).
pub fn service(paths: &Paths) -> anyhow::Result<AnchorService> {
    let store = Rc::new(SqliteStore::open(&paths.db)?);
    let ledger = Rc::new(SqliteLedger::open(&paths.db)?);
    let signer = Rc::new(MinisignSigner::open(paths.data_dir.join("audit.key")));
    Ok(AnchorService::new(
        ledger,
        store,
        signer,
        Rc::new(SystemClock),
    ))
}

fn open_service() -> anyhow::Result<AnchorService> {
    let paths = Paths::default_home()?;
    std::fs::create_dir_all(&paths.data_dir)
        .with_context(|| format!("cannot create {}", paths.data_dir.display()))?;
    service(&paths)
}

/// Checkpoints kept elsewhere, one JSON object per line.
fn read_checkpoints(path: &Path) -> anyhow::Result<Vec<Checkpoint>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| Checkpoint::from_json(l).map_err(|e| anyhow::anyhow!("line {}: {e}", i + 1)))
        .collect()
}

/// Walks the whole chain and compares it with every checkpoint (and with
/// the ones in `extra`, kept elsewhere). Returns false when anything fails.
pub fn verify(extra: Option<&Path>) -> anyhow::Result<bool> {
    let extra = extra.map(read_checkpoints).transpose()?.unwrap_or_default();
    let report = open_service()?.audit(&extra)?;
    println!("verified {} entries", report.entries);
    match report.head {
        Some((seq, hash)) => println!("head    #{seq} {}", hex(&hash)),
        None => println!("head    — (empty ledger)"),
    }
    if report.checkpoints.is_empty() {
        println!("anchors — none yet (swarmux audit checkpoint)");
    }
    for j in &report.checkpoints {
        let c = &j.checkpoint;
        println!(
            "anchor  #{} {} {}{}",
            c.seq,
            iso_minute(c.at),
            if j.ok() { "ok" } else { "FAILED" },
            if j.own_key { "" } else { " (other key)" }
        );
    }
    if report.unanchored > 0 && !report.checkpoints.is_empty() {
        println!(
            "        {} entries since the last checkpoint are not anchored",
            report.unanchored
        );
    }
    for problem in report.problems() {
        println!("BROKEN  {problem}");
    }
    if report.is_ok() {
        println!("OK");
    }
    Ok(report.is_ok())
}

/// Signs the ledger's head and keeps the checkpoint.
pub fn checkpoint() -> anyhow::Result<()> {
    let service = open_service()?;
    let c = service.checkpoint()?;
    let (key, protection) = service.key()?;
    println!(
        "checkpoint #{} {} {}",
        c.seq,
        iso_minute(c.at),
        hex(&c.head)
    );
    println!("key      {key}");
    println!("held in  {}", protection.text());
    println!("Keep a copy somewhere an agent cannot write: swarmux audit export-checkpoints");
    Ok(())
}

/// Prints every checkpoint, one JSON object per line.
pub fn export_checkpoints() -> anyhow::Result<()> {
    for c in open_service()?.list()? {
        println!("{}", c.to_json());
    }
    Ok(())
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
