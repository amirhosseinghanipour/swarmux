//! Ranking: which of the Work queue's four groups an outcome belongs to,
//! and the order inside each group.
//!
//! The groups, in the order they are shown:
//!
//! 1. **Decide**: the outcome needs the user. A held tool call, a
//!    question, a failed job, a gate that gave up, a recovery that did
//!    not work, a plan waiting for approval, a merge that cannot happen,
//!    a reviewer's concerns, a build that strayed from its plan; or a
//!    mission that was escalated (the user decides; at least P2, the
//!    priority the inbox gives a gate that gave up). The priorities are
//!    the attention module's own (P1, P2, P3); nothing new is invented.
//! 2. **Ready to land**: the gate passed and the work has not landed yet.
//! 3. **Running**: a job is out for it, or it waits for a slot (queued).
//! 4. **Landed**: delivered with proof.
//!
//! An outcome with nothing to say (a finished job nobody judged and
//! nobody asks about, a mission still a draft) is on no queue: [`place`]
//! answers `None`.
//!
//! The order inside a group is total and deterministic (the outcome's id
//! breaks every tie), so two clients never disagree:
//!
//! - Decide: priority (P1 first), then the earliest mission due time (an
//!   outcome with none last: a due time is only a tiebreak after priority,
//!   and no due time is ever made up), then the earliest auto-deny deadline
//!   (an ask without one last), then the oldest ask: the order the inbox's
//!   own ranking gives its items.
//! - Ready to land: waiting longest first.
//! - Running: verifying first (nearest to needing the user), then
//!   running, then queued; oldest first within each.
//! - Landed: newest first.

use crate::{AttentionItem, AttentionKind, Millis, Priority};

/// What an escalated mission is at least: the inbox's priority for a gate
/// that gave up.
const ESCALATED: Priority = Priority::P2;

/// The Work queue's groups, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Decide,
    ReadyToLand,
    Running,
    Landed,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::Decide,
        Group::ReadyToLand,
        Group::Running,
        Group::Landed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::Decide => "decide",
            Group::ReadyToLand => "ready_to_land",
            Group::Running => "running",
            Group::Landed => "landed",
        }
    }
}

/// How far an outcome has got, as far as the queue is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Nothing to place: a mission not started, or a plain job that
    /// finished without anyone judging it.
    Idle,
    /// Waits for a slot.
    Queued,
    Running,
    /// The job finished; the gate's verdict is awaited.
    Verifying,
    /// The gate passed; the work has not landed.
    Verified,
    /// Delivered with proof.
    Landed,
    /// The mission was escalated: the user decides.
    Escalated,
}

/// One open ask of the attention inbox, as far as ranking cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub kind: AttentionKind,
    pub priority: Priority,
    /// When it resolves without the user (an approval times out).
    pub deadline: Option<Millis>,
    /// When it started needing the user.
    pub since: Millis,
}

impl Ask {
    pub fn of(item: &AttentionItem) -> Ask {
        Ask {
            kind: item.kind,
            priority: item.priority,
            deadline: item.deadline,
            since: item.since,
        }
    }

    /// Whether the ask puts its outcome among the decisions. A result
    /// to read, a quiet job and the follow-ups and knowledge the inbox
    /// also carries are information, not a decision: they never move an
    /// outcome out of the group its stage gives it.
    pub fn decides(&self) -> bool {
        !matches!(
            self.kind,
            AttentionKind::ResultReady
                | AttentionKind::Stalled
                | AttentionKind::FollowUp
                | AttentionKind::Knowledge
        )
    }
}

/// What ranking knows about one outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standing {
    /// Stable and unique; the final tie-break.
    pub id: String,
    pub stage: Stage,
    /// When the outcome entered its stage (queued, started, verified,
    /// delivered), the age the group orders by.
    pub since: Millis,
    /// The open asks about the outcome.
    pub asks: Vec<Ask>,
    /// When the mission is due, if its contract says so.
    pub due: Option<Millis>,
}

/// Where an outcome stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub group: Group,
    /// How urgent, for the decisions.
    pub priority: Option<Priority>,
}

/// The group of `s`, and its priority when it is a decision; `None` when
/// it is on no queue.
pub fn place(s: &Standing) -> Option<Placement> {
    let mut priority = s
        .asks
        .iter()
        .filter(|a| a.decides())
        .map(|a| a.priority)
        .min();
    if s.stage == Stage::Escalated {
        priority = Some(priority.map_or(ESCALATED, |p| p.min(ESCALATED)));
    }
    if priority.is_some() {
        return Some(Placement {
            group: Group::Decide,
            priority,
        });
    }
    let group = match s.stage {
        Stage::Queued | Stage::Running | Stage::Verifying => Group::Running,
        Stage::Verified => Group::ReadyToLand,
        Stage::Landed => Group::Landed,
        Stage::Idle | Stage::Escalated => return None,
    };
    Some(Placement {
        group,
        priority: None,
    })
}

/// An outcome placed: its index in the input, and where it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub placement: Placement,
}

/// What orders outcomes inside their group.
type Key = (Group, Option<Priority>, i64, i64, i64);

fn key(s: &Standing, p: &Placement) -> Key {
    match p.group {
        Group::Decide => {
            let decisive = || s.asks.iter().filter(|a| a.decides());
            let deadline = decisive()
                .filter_map(|a| a.deadline)
                .min()
                .unwrap_or(Millis::MAX);
            let age = decisive().map(|a| a.since).min().unwrap_or(s.since);
            let due = s.due.unwrap_or(Millis::MAX);
            (p.group, p.priority, due, deadline, age)
        }
        Group::ReadyToLand => (p.group, None, s.since, 0, 0),
        Group::Running => {
            let step = match s.stage {
                Stage::Verifying => 0,
                Stage::Running => 1,
                _ => 2,
            };
            (p.group, None, step, s.since, 0)
        }
        Group::Landed => (p.group, None, s.since.saturating_neg(), 0, 0),
    }
}

/// Places every outcome of `standings` that is on a queue and orders them:
/// by group, then by the group's rule. The order does not depend on the
/// order of the input.
pub fn rank(standings: &[Standing]) -> Vec<Ranked> {
    let mut out: Vec<Ranked> = standings
        .iter()
        .enumerate()
        .filter_map(|(index, s)| place(s).map(|placement| Ranked { index, placement }))
        .collect();
    out.sort_by(|a, b| {
        let (sa, sb) = (&standings[a.index], &standings[b.index]);
        key(sa, &a.placement)
            .cmp(&key(sb, &b.placement))
            .then_with(|| sa.id.cmp(&sb.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(
        kind: AttentionKind,
        priority: Priority,
        deadline: Option<Millis>,
        since: Millis,
    ) -> Ask {
        Ask {
            kind,
            priority,
            deadline,
            since,
        }
    }

    fn stand(id: &str, stage: Stage, since: Millis, asks: Vec<Ask>) -> Standing {
        Standing {
            id: id.into(),
            stage,
            since,
            asks,
            due: None,
        }
    }

    fn ids(standings: &[Standing]) -> Vec<(&str, Group)> {
        rank(standings)
            .into_iter()
            .map(|r| (standings[r.index].id.as_str(), r.placement.group))
            .collect()
    }

    #[test]
    fn a_decision_beats_the_stage_and_takes_the_highest_priority() {
        let s = stand(
            "a",
            Stage::Running,
            10,
            vec![
                ask(AttentionKind::Approval, Priority::P2, Some(500), 20),
                ask(AttentionKind::Approval, Priority::P1, Some(900), 30),
            ],
        );
        assert_eq!(
            place(&s),
            Some(Placement {
                group: Group::Decide,
                priority: Some(Priority::P1)
            })
        );
    }

    #[test]
    fn information_never_moves_an_outcome_out_of_its_group() {
        let info = [
            AttentionKind::ResultReady,
            AttentionKind::Stalled,
            AttentionKind::FollowUp,
            AttentionKind::Knowledge,
        ]
        .map(|k| ask(k, Priority::P3, None, 5));
        for stage in [Stage::Running, Stage::Verified, Stage::Landed] {
            let s = stand("a", stage, 1, info.to_vec());
            let group = place(&s).unwrap().group;
            assert_ne!(group, Group::Decide, "{stage:?}");
        }
        let delivered = stand("d", Stage::Landed, 1, info.to_vec());
        assert_eq!(place(&delivered).unwrap().group, Group::Landed);
    }

    #[test]
    fn an_outcome_with_nothing_to_say_is_on_no_queue() {
        let quiet = stand("q", Stage::Idle, 1, vec![]);
        assert_eq!(place(&quiet), None);
        let read = stand(
            "r",
            Stage::Idle,
            1,
            vec![ask(AttentionKind::ResultReady, Priority::P3, None, 1)],
        );
        assert_eq!(place(&read), None);
        // ... until something asks for a decision.
        let worried = stand(
            "w",
            Stage::Idle,
            1,
            vec![ask(AttentionKind::ReviewConcerns, Priority::P3, None, 1)],
        );
        assert_eq!(
            place(&worried),
            Some(Placement {
                group: Group::Decide,
                priority: Some(Priority::P3)
            })
        );
    }

    #[test]
    fn an_escalated_mission_is_a_decision_of_at_least_p2() {
        let bare = stand("e", Stage::Escalated, 1, vec![]);
        assert_eq!(place(&bare).unwrap().priority, Some(Priority::P2));
        let weak = stand(
            "e",
            Stage::Escalated,
            1,
            vec![ask(AttentionKind::ReviewConcerns, Priority::P3, None, 1)],
        );
        assert_eq!(place(&weak).unwrap().priority, Some(Priority::P2));
        let grave = stand(
            "e",
            Stage::Escalated,
            1,
            vec![ask(AttentionKind::GateEscalated, Priority::P1, None, 1)],
        );
        assert_eq!(place(&grave).unwrap().priority, Some(Priority::P1));
    }

    #[test]
    fn stages_give_groups() {
        let all = [
            stand("1", Stage::Queued, 1, vec![]),
            stand("2", Stage::Running, 1, vec![]),
            stand("3", Stage::Verifying, 1, vec![]),
            stand("4", Stage::Verified, 1, vec![]),
            stand("5", Stage::Landed, 1, vec![]),
        ];
        let groups: Vec<Group> = all.iter().map(|s| place(s).unwrap().group).collect();
        assert_eq!(
            groups,
            [
                Group::Running,
                Group::Running,
                Group::Running,
                Group::ReadyToLand,
                Group::Landed
            ]
        );
    }

    #[test]
    fn decisions_order_by_priority_then_deadline_then_age_then_id() {
        let at = |id: &str, p: Priority, deadline: Option<Millis>, since: Millis| {
            stand(
                id,
                Stage::Running,
                0,
                vec![ask(AttentionKind::Approval, p, deadline, since)],
            )
        };
        let standings = vec![
            at("e", Priority::P3, None, 1),
            at("d", Priority::P2, None, 5),
            at("c", Priority::P2, Some(900), 9),
            at("b", Priority::P2, Some(100), 9),
            at("a", Priority::P1, Some(5_000), 9),
            at("a2", Priority::P2, None, 2),
            at("a3", Priority::P2, None, 2),
        ];
        let order: Vec<&str> = ids(&standings).into_iter().map(|(id, _)| id).collect();
        // P1; then P2 by deadline (100, 900), then no deadline by age (2, 2, 5).
        assert_eq!(order, ["a", "b", "c", "a2", "a3", "d", "e"]);
    }

    #[test]
    fn a_due_time_only_breaks_ties_after_priority_and_none_goes_last() {
        let at = |id: &str, p: Priority, due: Option<Millis>, deadline: Option<Millis>| {
            let mut s = stand(
                id,
                Stage::Running,
                0,
                vec![ask(AttentionKind::Approval, p, deadline, 1)],
            );
            s.due = due;
            s
        };
        let standings = vec![
            at("p2-no-due", Priority::P2, None, Some(10)),
            at("p2-late", Priority::P2, Some(9_000), Some(500)),
            at("p2-soon", Priority::P2, Some(1_000), None),
            at("p1-no-due", Priority::P1, None, None),
            at("p3-soonest", Priority::P3, Some(1), None),
        ];
        let order: Vec<&str> = ids(&standings).into_iter().map(|(id, _)| id).collect();
        // Priority first, however soon something else is due; within a
        // priority the earlier due time first, an outcome with none after
        // every one that has one (the auto-deny deadline orders those).
        assert_eq!(
            order,
            ["p1-no-due", "p2-soon", "p2-late", "p2-no-due", "p3-soonest"]
        );
    }

    #[test]
    fn the_due_time_does_not_move_outcomes_that_are_not_decisions() {
        let mut soon = stand("a", Stage::Verified, 50, vec![]);
        soon.due = Some(1);
        let later = stand("b", Stage::Verified, 20, vec![]);
        let standings = vec![soon, later];
        let order: Vec<&str> = ids(&standings).into_iter().map(|(id, _)| id).collect();
        assert_eq!(
            order,
            ["b", "a"],
            "ready to land stays by how long it waited"
        );
    }

    #[test]
    fn the_queue_runs_decide_ready_running_landed() {
        let standings = vec![
            stand("landed-old", Stage::Landed, 100, vec![]),
            stand("landed-new", Stage::Landed, 900, vec![]),
            stand("queued", Stage::Queued, 50, vec![]),
            stand("running-new", Stage::Running, 80, vec![]),
            stand("running-old", Stage::Running, 10, vec![]),
            stand("verifying", Stage::Verifying, 99, vec![]),
            stand("ready-new", Stage::Verified, 70, vec![]),
            stand("ready-old", Stage::Verified, 20, vec![]),
            stand(
                "held",
                Stage::Running,
                1,
                vec![ask(AttentionKind::Approval, Priority::P1, Some(10), 1)],
            ),
            stand("idle", Stage::Idle, 1, vec![]),
        ];
        let order: Vec<&str> = ids(&standings).into_iter().map(|(id, _)| id).collect();
        assert_eq!(
            order,
            [
                "held",
                "ready-old",
                "ready-new",
                "verifying",
                "running-old",
                "running-new",
                "queued",
                "landed-new",
                "landed-old",
            ]
        );
    }

    #[test]
    fn the_order_does_not_depend_on_the_input_order() {
        let mut standings = vec![
            stand("a", Stage::Landed, 5, vec![]),
            stand("b", Stage::Landed, 5, vec![]),
            stand("c", Stage::Running, 5, vec![]),
            stand("d", Stage::Running, 5, vec![]),
            stand("e", Stage::Escalated, 5, vec![]),
            stand("f", Stage::Verified, 5, vec![]),
            stand("g", Stage::Verified, 5, vec![]),
        ];
        let first: Vec<String> = rank(&standings)
            .iter()
            .map(|r| standings[r.index].id.clone())
            .collect();
        standings.reverse();
        let second: Vec<String> = rank(&standings)
            .iter()
            .map(|r| standings[r.index].id.clone())
            .collect();
        assert_eq!(first, second);
        assert_eq!(first, ["e", "f", "g", "c", "d", "a", "b"]);
    }
}
