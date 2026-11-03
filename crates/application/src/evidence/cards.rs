//! Run cards: one per job, joined with the mission it belongs to and the
//! bundle that vouches for it.

use swarmux_domain::JobStatus;
use swarmux_domain::evidence::{RunCard, RunEvidence, RunOutcome};
use swarmux_domain::mission::{Mission, MissionState};

use super::Stored;
use crate::JobSummary;

/// Characters of a digest a card shows.
const DIGEST_SHOWN: usize = 8;

/// The card of every job in `rows` (newest first, as given), except the
/// reviewers' own jobs: a review's cost belongs to the job it audits.
pub fn build_cards(rows: &[JobSummary], missions: &[Mission], stored: &[Stored]) -> Vec<RunCard> {
    rows.iter()
        .filter(|r| r.review_of.is_none())
        .map(|r| {
            let mission = missions.iter().find(|m| m.job == Some(r.job_id));
            let bundle =
                mission.and_then(|m| stored.iter().find(|s| s.bundle.contract.slug == m.slug));
            card(r, mission, bundle)
        })
        .collect()
}

fn card(row: &JobSummary, mission: Option<&Mission>, stored: Option<&Stored>) -> RunCard {
    let outcome = match mission {
        Some(m) => match m.state {
            MissionState::Delivered => RunOutcome::Delivered,
            MissionState::Escalated => RunOutcome::Escalated,
            MissionState::Refused => RunOutcome::Refused,
            MissionState::Queued => RunOutcome::Queued,
            MissionState::Running | MissionState::Verifying => RunOutcome::Running,
            MissionState::Draft | MissionState::Linted => of_status(row.status),
        },
        None => of_status(row.status),
    };
    let cost_usd = mission.map_or(row.cost_usd, |m| m.spend.total().max(row.cost_usd));
    let models = match stored {
        Some(s) => s.bundle.cost.by_model.clone(),
        None => vec![(row.model.clone(), row.cost_usd)],
    };
    RunCard {
        job: row.job_id,
        mission: mission.map(|m| m.slug.clone()),
        title: title(row, mission),
        outcome,
        evidence: stored.map(|s| {
            let (passed, total) = s.bundle.check_tally();
            RunEvidence {
                digest: s.digest.chars().take(DIGEST_SHOWN).collect(),
                checks_passed: passed,
                checks_total: total,
                blocks: s.bundle.blocks(),
            }
        }),
        cost_usd,
        models,
        labels: stored.map(|s| s.labels.clone()).unwrap_or_default(),
        at: row.created_at,
    }
}

fn of_status(status: JobStatus) -> RunOutcome {
    match status {
        JobStatus::Queued => RunOutcome::Queued,
        JobStatus::Running => RunOutcome::Running,
        JobStatus::Done => RunOutcome::Done,
        JobStatus::Failed | JobStatus::Interrupted => RunOutcome::Failed,
        JobStatus::Cancelled => RunOutcome::Cancelled,
    }
}

fn title(row: &JobSummary, mission: Option<&Mission>) -> String {
    match mission {
        Some(m) if !m.contract.title.trim().is_empty() => m.contract.title.trim().to_string(),
        Some(m) => m.slug.clone(),
        None => row.preview(80),
    }
}
