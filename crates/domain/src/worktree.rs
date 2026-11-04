//! Worktrees and checkpoints: where a job works, which ports it owns, and
//! the resumable points taken along the way.

use std::path::PathBuf;

use crate::ledger::Payload;
use crate::{JobId, Millis, SessionId};

/// Longest mission part of a branch name.
const SLUG_MAX: usize = 40;

/// The part of a branch name that names the mission (the session).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') && c.is_ascii() {
            out.push('-');
        }
        if out.len() >= SLUG_MAX {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

/// `swarmux/<mission>/<n>`.
pub fn branch_name(mission: &str, n: JobId) -> String {
    format!("swarmux/{}/{}", slug(mission), n.0)
}

/// How port blocks are handed out: slot `n` owns the ports
/// `base + n * block ..` (`block` of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortPlan {
    pub base: u16,
    pub block: u16,
    pub slots: u16,
}

impl Default for PortPlan {
    fn default() -> PortPlan {
        PortPlan {
            base: 20_000,
            block: 20,
            slots: 500,
        }
    }
}

impl PortPlan {
    /// The first port of `slot`, or `None` past the plan or the port range.
    pub fn port_base(&self, slot: u32) -> Option<u16> {
        if slot >= u32::from(self.slots) || self.block == 0 {
            return None;
        }
        let first = u32::from(self.base) + slot * u32::from(self.block);
        let last = first + u32::from(self.block) - 1;
        u16::try_from(last).ok()?;
        u16::try_from(first).ok()
    }

    /// The lowest slot not in `used`.
    pub fn lowest_free(&self, used: &[u32]) -> Option<u32> {
        (0..u32::from(self.slots))
            .find(|s| !used.contains(s))
            .filter(|s| self.port_base(*s).is_some())
    }
}

/// A job's worktree, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub job: JobId,
    pub session: SessionId,
    /// The repository's top folder.
    pub repo: PathBuf,
    /// The worktree's top folder.
    pub path: PathBuf,
    pub branch: String,
    pub slot: u32,
    pub port_base: u16,
    pub port_count: u16,
    pub created_at: Millis,
    /// Set once the folder was reclaimed.
    pub removed_at: Option<Millis>,
}

/// Why a checkpoint was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointReason {
    GatePass,
    PreCompact,
    Manual,
    /// The watchdog handed the work to a new session.
    Recovery,
}

impl CheckpointReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointReason::GatePass => "gate_pass",
            CheckpointReason::PreCompact => "pre_compact",
            CheckpointReason::Manual => "manual",
            CheckpointReason::Recovery => "recovery",
        }
    }

    pub fn parse(s: &str) -> Option<CheckpointReason> {
        [
            CheckpointReason::GatePass,
            CheckpointReason::PreCompact,
            CheckpointReason::Manual,
            CheckpointReason::Recovery,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

/// What a successor session needs to carry on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handoff {
    pub goal: String,
    pub state: String,
    pub decisions: Vec<String>,
    pub tried_and_failed: Vec<String>,
    pub next_step: String,
    pub open_questions: Vec<String>,
}

impl Handoff {
    /// The `handoff.json` text.
    pub fn to_json(&self) -> String {
        Payload::new()
            .str("goal", &self.goal)
            .str("state", &self.state)
            .strs("decisions", &self.decisions)
            .strs("tried_and_failed", &self.tried_and_failed)
            .str("next_step", &self.next_step)
            .strs("open_questions", &self.open_questions)
            .build()
    }

    /// A short briefing for the start of the next session.
    pub fn briefing(&self) -> String {
        let mut out = format!("Handoff from the previous session.\nGoal: {}\n", self.goal);
        if !self.state.is_empty() {
            out.push_str(&format!("State: {}\n", self.state));
        }
        for (title, items) in [
            ("Decisions", &self.decisions),
            ("Tried and failed", &self.tried_and_failed),
            ("Open questions", &self.open_questions),
        ] {
            if !items.is_empty() {
                out.push_str(&format!("{title}:\n"));
                for item in items {
                    out.push_str(&format!("- {item}\n"));
                }
            }
        }
        if !self.next_step.is_empty() {
            out.push_str(&format!("Next step: {}\n", self.next_step));
        }
        out
    }
}

/// A resumable point of a job: the Claude session, the commit holding the
/// worktree's files, and where the ledger stood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub id: i64,
    pub job: JobId,
    /// Claude Code's session id.
    pub session_id: String,
    pub commit: String,
    pub worktree: PathBuf,
    pub branch: String,
    pub reason: CheckpointReason,
    /// The ledger entry that records this checkpoint.
    pub ledger_seq: Option<u64>,
    pub handoff: Option<Handoff>,
    pub at: Millis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_branch_safe() {
        assert_eq!(slug("  My Project: v2! "), "my-project-v2");
        assert_eq!(slug("日本語"), "session");
        assert_eq!(slug(&"a".repeat(100)).len(), SLUG_MAX);
        assert_eq!(branch_name("Demo app", JobId(7)), "swarmux/demo-app/7");
    }

    #[test]
    fn slots_map_to_disjoint_blocks() {
        let plan = PortPlan::default();
        assert_eq!(plan.port_base(0), Some(20_000));
        assert_eq!(plan.port_base(1), Some(20_020));
        assert_eq!(plan.port_base(500), None);
        let high = PortPlan {
            base: 65_000,
            block: 100,
            slots: 50,
        };
        assert_eq!(high.port_base(4), Some(65_400));
        assert_eq!(high.port_base(5), None);
    }

    #[test]
    fn lowest_free_slot_skips_used() {
        let plan = PortPlan {
            base: 30_000,
            block: 10,
            slots: 3,
        };
        assert_eq!(plan.lowest_free(&[]), Some(0));
        assert_eq!(plan.lowest_free(&[0, 2]), Some(1));
        assert_eq!(plan.lowest_free(&[0, 1, 2]), None);
    }

    #[test]
    fn handoff_json_and_briefing() {
        let h = Handoff {
            goal: "ship \"it\"".into(),
            next_step: "run tests".into(),
            tried_and_failed: vec!["cargo test failed".into()],
            ..Handoff::default()
        };
        let json = h.to_json();
        assert!(json.contains(r#""goal":"ship \"it\"""#), "{json}");
        assert!(json.contains(r#""decisions":[]"#));
        let text = h.briefing();
        assert!(text.contains("Next step: run tests"));
        assert!(text.contains("- cargo test failed"));
    }

    #[test]
    fn reasons_round_trip() {
        for r in [
            CheckpointReason::GatePass,
            CheckpointReason::PreCompact,
            CheckpointReason::Manual,
        ] {
            assert_eq!(CheckpointReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(CheckpointReason::parse("x"), None);
    }
}
