//! What became of a branch: its pull request and whether the merge was
//! reverted, asked of the GitHub CLI.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use swarmux_application::{OutcomeProbe, ProbeReply, ProbeRequest, ProbeSink};
use swarmux_domain::evidence::{PrOutcome, PrState};
use swarmux_domain::parse_iso_millis;

use crate::telemetry::cli;
use crate::telemetry::github::explain;

const LIMIT: Duration = Duration::from_secs(25);

/// Asks `gh`; each question runs on a thread of its own.
pub struct GhOutcomes {
    bin: PathBuf,
}

impl GhOutcomes {
    pub fn new(bin: PathBuf) -> GhOutcomes {
        GhOutcomes { bin }
    }
}

impl Default for GhOutcomes {
    fn default() -> GhOutcomes {
        GhOutcomes::new("gh".into())
    }
}

impl OutcomeProbe for GhOutcomes {
    fn probe(&self, request: ProbeRequest, sink: ProbeSink) {
        let bin = self.bin.clone();
        let spawned = std::thread::Builder::new()
            .name("swarmux-outcome".into())
            .spawn(move || sink(ask(&bin, &request.repo, &request.branch)));
        // A thread that could not start leaves the sink unanswered; the
        // labels simply stay as they were.
        drop(spawned);
    }
}

#[derive(Debug, Deserialize)]
struct PrRow {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(rename = "mergedAt", default)]
    merged_at: Option<String>,
    #[serde(default)]
    url: String,
}

fn rows(text: &str) -> Result<Vec<PrRow>, String> {
    serde_json::from_str(text).map_err(|e| format!("unexpected gh output: {e}"))
}

fn ask(bin: &Path, repo: &Path, branch: &str) -> ProbeReply {
    let run =
        |args: &[&str]| cli::run(bin, args, Some(repo), LIMIT).map_err(|e| explain(e).message);
    let fields = "number,title,state,mergedAt,url";
    let found = rows(&run(&[
        "pr", "list", "--head", branch, "--state", "all", "--limit", "5", "--json", fields,
    ])?)?;
    let Some(pr) = choose(&found) else {
        return Ok(None);
    };
    let merged = merged_at(pr);
    let reverts = match (merged, pr.title.is_empty()) {
        (Some(_), false) => {
            let query = format!("\"Revert \\\"{}\\\"\" in:title", pr.title);
            rows(&run(&[
                "pr", "list", "--state", "merged", "--search", &query, "--limit", "10", "--json",
                fields,
            ])?)?
        }
        _ => Vec::new(),
    };
    Ok(Some(outcome(pr, &reverts)))
}

/// The pull request that counts: a merged one, else the newest.
fn choose(found: &[PrRow]) -> Option<&PrRow> {
    found
        .iter()
        .find(|p| p.state.eq_ignore_ascii_case("MERGED"))
        .or_else(|| found.first())
}

fn merged_at(pr: &PrRow) -> Option<i64> {
    pr.merged_at.as_deref().and_then(parse_iso_millis)
}

/// The pull request as an outcome, with the earliest merged revert of it
/// found among `reverts`.
fn outcome(pr: &PrRow, reverts: &[PrRow]) -> PrOutcome {
    let merged = merged_at(pr);
    let state = match (pr.state.to_ascii_uppercase().as_str(), merged) {
        ("MERGED", Some(at)) => PrState::Merged { at },
        ("OPEN", _) => PrState::Open,
        _ => PrState::Closed,
    };
    let wanted = format!("revert \"{}\"", pr.title.to_lowercase());
    let reverted_at = merged.and_then(|at| {
        reverts
            .iter()
            .filter(|r| r.number != pr.number && r.title.to_lowercase().starts_with(&wanted))
            .filter_map(merged_at)
            .filter(|r| *r >= at)
            .min()
    });
    PrOutcome {
        number: pr.number,
        state,
        url: pr.url.clone(),
        reverted_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<PrRow> {
        rows(text).unwrap()
    }

    #[test]
    fn merged_open_and_closed() {
        let merged = parse(
            r#"[{"number":4,"title":"Fix parser","state":"MERGED","mergedAt":"2026-11-02T10:00:00Z","url":"u"},
                {"number":3,"title":"Fix parser","state":"CLOSED","mergedAt":null,"url":"v"}]"#,
        );
        let pr = choose(&merged).unwrap();
        assert_eq!(pr.number, 4, "a merged one wins over a newer closed one");
        let o = outcome(pr, &[]);
        assert_eq!(
            o.state,
            PrState::Merged {
                at: parse_iso_millis("2026-11-02T10:00:00Z").unwrap()
            }
        );
        assert_eq!(o.reverted_at, None);
        let open = parse(r#"[{"number":5,"title":"x","state":"OPEN","mergedAt":null,"url":""}]"#);
        assert_eq!(outcome(&open[0], &[]).state, PrState::Open);
        let closed =
            parse(r#"[{"number":6,"title":"x","state":"CLOSED","mergedAt":null,"url":""}]"#);
        assert_eq!(outcome(&closed[0], &[]).state, PrState::Closed);
        assert!(choose(&[]).is_none());
    }

    #[test]
    fn a_merged_revert_after_the_merge_is_found() {
        let pr = parse(
            r#"[{"number":4,"title":"Fix parser","state":"MERGED","mergedAt":"2026-11-02T10:00:00Z","url":"u"}]"#,
        );
        let reverts = parse(
            r#"[{"number":9,"title":"Revert \"Fix parser\"","state":"MERGED","mergedAt":"2026-11-03T10:00:00Z","url":""},
                {"number":8,"title":"Revert \"Something else\"","state":"MERGED","mergedAt":"2026-11-02T12:00:00Z","url":""},
                {"number":7,"title":"Revert \"Fix parser\"","state":"MERGED","mergedAt":"2026-11-01T10:00:00Z","url":""}]"#,
        );
        let o = outcome(&pr[0], &reverts);
        assert_eq!(
            o.reverted_at,
            parse_iso_millis("2026-11-03T10:00:00Z"),
            "the revert before the merge and the other title do not count"
        );
    }

    #[test]
    fn bad_output_is_reported() {
        assert!(
            rows("not json")
                .unwrap_err()
                .contains("unexpected gh output")
        );
    }

    #[test]
    fn a_missing_gh_is_reported_not_panicked() {
        let dir = tempfile::tempdir().unwrap();
        let r = ask(&dir.path().join("no-such-gh"), dir.path(), "main");
        assert!(r.is_err());
    }
}
