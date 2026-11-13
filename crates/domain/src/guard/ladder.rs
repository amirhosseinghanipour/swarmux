//! The recovery ladder: what the watchdog tries, cheapest first, when an
//! agent is stuck. Each rung is tried once per run of work, in order; after
//! a rung the agent gets time to settle before the next one, so it is never
//! nudged again and again.
//!
//! Pure: the application carries the rungs out.

use super::watchdog::AlarmKind;
use crate::Millis;

/// One step of the recovery ladder, cheapest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rung {
    /// A short message on the agent's next tool call.
    Nudge,
    /// Checkpoint, hand off, and carry on in a new session with a clean context.
    Fresh,
    /// Haiku reads the last steps and names what went wrong.
    Diagnose,
    /// Back to the last checkpoint before the trouble, in a new session.
    Fork,
    /// A new session on a stronger model.
    Escalate,
    /// A new session that plans smaller steps first.
    Replan,
    /// The user decides.
    Inbox,
}

impl Rung {
    pub const ALL: [Rung; 7] = [
        Rung::Nudge,
        Rung::Fresh,
        Rung::Diagnose,
        Rung::Fork,
        Rung::Escalate,
        Rung::Replan,
        Rung::Inbox,
    ];

    /// The stable name stored and written in `config.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Rung::Nudge => "nudge",
            Rung::Fresh => "fresh",
            Rung::Diagnose => "diagnose",
            Rung::Fork => "fork",
            Rung::Escalate => "escalate",
            Rung::Replan => "replan",
            Rung::Inbox => "inbox",
        }
    }

    pub fn parse(s: &str) -> Option<Rung> {
        Rung::ALL.into_iter().find(|r| r.as_str() == s.trim())
    }

    /// What the recovery trail calls it.
    pub fn label(self) -> &'static str {
        match self {
            Rung::Nudge => "nudge",
            Rung::Fresh => "fresh session",
            Rung::Diagnose => "diagnosis",
            Rung::Fork => "fork from checkpoint",
            Rung::Escalate => "stronger model",
            Rung::Replan => "replan",
            Rung::Inbox => "inbox",
        }
    }

    /// The rung stops the job and starts another.
    pub fn restarts(self) -> bool {
        matches!(
            self,
            Rung::Fresh | Rung::Fork | Rung::Escalate | Rung::Replan
        )
    }

    fn index(self) -> usize {
        Rung::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
}

/// Which rungs may be used and how long each waits before the next.
#[derive(Clone, Debug, PartialEq)]
pub struct LadderPolicy {
    /// The rungs that may be taken; the inbox is always the last resort.
    pub rungs: Vec<Rung>,
    /// Settling time after a nudge or a diagnosis.
    pub nudge_cooldown_ms: Millis,
    /// Settling time after a new session starts.
    pub restart_cooldown_ms: Millis,
    /// New sessions one run of work may get.
    pub max_restarts: u32,
}

impl Default for LadderPolicy {
    fn default() -> LadderPolicy {
        LadderPolicy {
            rungs: Rung::ALL.to_vec(),
            nudge_cooldown_ms: 90_000,
            restart_cooldown_ms: 180_000,
            max_restarts: 3,
        }
    }
}

impl LadderPolicy {
    /// How long the agent is left alone after `rung`.
    pub fn cooldown(&self, rung: Rung) -> Millis {
        match rung {
            Rung::Nudge | Rung::Diagnose => self.nudge_cooldown_ms,
            Rung::Fresh | Rung::Fork | Rung::Escalate | Rung::Replan => self.restart_cooldown_ms,
            Rung::Inbox => 0,
        }
    }

    fn allows(&self, rung: Rung) -> bool {
        rung == Rung::Inbox || self.rungs.contains(&rung)
    }
}

/// What can be done right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RungOptions {
    /// The job still runs.
    pub live: bool,
    /// A message on its next tool call would reach it.
    pub reachable: bool,
    /// A checkpoint from before the trouble exists.
    pub checkpoint: bool,
    /// A stronger model than the current one is configured.
    pub stronger_model: bool,
}

/// One rung taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken {
    pub rung: Rung,
    pub kind: AlarmKind,
    pub at: Millis,
}

/// What to do about an alarm now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextRung {
    Take(Rung),
    /// Let the agent settle after the last rung.
    Wait {
        until: Millis,
    },
    /// The inbox has it; nothing more to try.
    Done,
}

/// Where one run of work stands on the ladder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ladder {
    taken: Vec<Taken>,
    /// The lowest rung not tried yet.
    next: usize,
    restarts: u32,
}

impl Ladder {
    /// The next step for an alarm of `kind`. A job that has ended needs no
    /// settling time.
    pub fn plan(
        &self,
        kind: AlarmKind,
        now: Millis,
        policy: &LadderPolicy,
        o: &RungOptions,
    ) -> NextRung {
        if self.escalated() {
            return NextRung::Done;
        }
        if o.live
            && let Some(until) = self.taken.last().map(|t| t.at + policy.cooldown(t.rung))
            && now < until
        {
            return NextRung::Wait { until };
        }
        let can_restart = self.restarts < policy.max_restarts;
        Rung::ALL[self.next..]
            .iter()
            .copied()
            .find(|&r| policy.allows(r) && applies(r, kind, o, can_restart))
            .map_or(NextRung::Take(Rung::Inbox), NextRung::Take)
    }

    /// Notes that `rung` was taken for an alarm of `kind`.
    pub fn record(&mut self, rung: Rung, kind: AlarmKind, now: Millis) {
        self.taken.push(Taken {
            rung,
            kind,
            at: now,
        });
        self.next = self.next.max(rung.index() + 1);
        if rung.restarts() {
            self.restarts += 1;
        }
    }

    pub fn taken(&self) -> &[Taken] {
        &self.taken
    }

    /// The highest rung taken so far.
    pub fn highest(&self) -> Option<Rung> {
        self.taken.iter().map(|t| t.rung).max()
    }

    /// The inbox rung was reached.
    pub fn escalated(&self) -> bool {
        self.taken.iter().any(|t| t.rung == Rung::Inbox)
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }
}

fn applies(rung: Rung, kind: AlarmKind, o: &RungOptions, can_restart: bool) -> bool {
    match rung {
        // Telling an agent its context is full doesn't empty it.
        Rung::Nudge => o.live && o.reachable && kind != AlarmKind::Context,
        Rung::Fresh | Rung::Replan => can_restart,
        Rung::Diagnose => kind != AlarmKind::Context,
        Rung::Fork => can_restart && o.checkpoint,
        Rung::Escalate => can_restart && o.stronger_model,
        Rung::Inbox => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> RungOptions {
        RungOptions {
            live: true,
            reachable: true,
            ..RungOptions::default()
        }
    }

    #[test]
    fn rungs_go_cheapest_first_with_cooldowns() {
        let p = LadderPolicy::default();
        let mut l = Ladder::default();
        assert_eq!(
            l.plan(AlarmKind::Loop, 0, &p, &live()),
            NextRung::Take(Rung::Nudge)
        );
        l.record(Rung::Nudge, AlarmKind::Loop, 0);
        assert_eq!(
            l.plan(AlarmKind::Loop, 10_000, &p, &live()),
            NextRung::Wait { until: 90_000 }
        );
        assert_eq!(
            l.plan(AlarmKind::Loop, 90_000, &p, &live()),
            NextRung::Take(Rung::Fresh)
        );
    }

    #[test]
    fn an_ended_job_climbs_at_once() {
        let p = LadderPolicy::default();
        let mut l = Ladder::default();
        l.record(Rung::Nudge, AlarmKind::Loop, 0);
        assert_eq!(
            l.plan(AlarmKind::Loop, 1, &p, &RungOptions::default()),
            NextRung::Take(Rung::Fresh)
        );
    }

    #[test]
    fn silent_stalls_and_full_contexts_skip_the_nudge() {
        let p = LadderPolicy::default();
        let l = Ladder::default();
        let silent = RungOptions {
            reachable: false,
            ..live()
        };
        assert_eq!(
            l.plan(AlarmKind::Stall, 0, &p, &silent),
            NextRung::Take(Rung::Fresh)
        );
        assert_eq!(
            l.plan(AlarmKind::Context, 0, &p, &live()),
            NextRung::Take(Rung::Fresh)
        );
    }

    #[test]
    fn restarts_are_capped_then_the_inbox() {
        let p = LadderPolicy {
            max_restarts: 1,
            ..LadderPolicy::default()
        };
        let mut l = Ladder::default();
        l.record(Rung::Fresh, AlarmKind::Loop, 0);
        let o = RungOptions {
            checkpoint: true,
            stronger_model: true,
            ..RungOptions::default()
        };
        assert_eq!(
            l.plan(AlarmKind::Loop, 1, &p, &o),
            NextRung::Take(Rung::Diagnose)
        );
        l.record(Rung::Diagnose, AlarmKind::Loop, 1);
        assert_eq!(
            l.plan(AlarmKind::Loop, 2, &p, &o),
            NextRung::Take(Rung::Inbox)
        );
        l.record(Rung::Inbox, AlarmKind::Loop, 2);
        assert!(l.escalated());
        assert_eq!(l.plan(AlarmKind::Loop, 3, &p, &o), NextRung::Done);
    }

    #[test]
    fn fork_and_escalate_need_their_means() {
        let p = LadderPolicy::default();
        let mut l = Ladder::default();
        l.record(Rung::Diagnose, AlarmKind::Loop, 0);
        assert_eq!(
            l.plan(AlarmKind::Loop, 1, &p, &RungOptions::default()),
            NextRung::Take(Rung::Replan)
        );
        let o = RungOptions {
            stronger_model: true,
            ..RungOptions::default()
        };
        assert_eq!(
            l.plan(AlarmKind::Loop, 1, &p, &o),
            NextRung::Take(Rung::Escalate)
        );
    }

    #[test]
    fn disabled_rungs_are_skipped() {
        let p = LadderPolicy {
            rungs: vec![Rung::Nudge],
            ..LadderPolicy::default()
        };
        let mut l = Ladder::default();
        l.record(Rung::Nudge, AlarmKind::Loop, 0);
        assert_eq!(
            l.plan(AlarmKind::Loop, 1, &p, &RungOptions::default()),
            NextRung::Take(Rung::Inbox)
        );
    }

    #[test]
    fn the_ladder_has_no_model_rung_and_climbs_all_the_way_to_the_inbox() {
        let names: Vec<&str> = Rung::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names,
            [
                "nudge", "fresh", "diagnose", "fork", "escalate", "replan", "inbox"
            ]
        );
        // From a nudge onward every rung is taken in turn once the one
        // before it has settled, with nothing configured but the defaults.
        let p = LadderPolicy::default();
        let mut l = Ladder::default();
        let o = RungOptions {
            checkpoint: true,
            stronger_model: true,
            ..live()
        };
        let mut taken = Vec::new();
        let mut now = 0;
        while let NextRung::Take(r) = l.plan(AlarmKind::Loop, now, &p, &o) {
            taken.push(r);
            l.record(r, AlarmKind::Loop, now);
            if r == Rung::Inbox {
                break;
            }
            now += 1_000_000;
        }
        assert_eq!(taken.first(), Some(&Rung::Nudge));
        assert_eq!(taken.last(), Some(&Rung::Inbox));
        assert!(taken.contains(&Rung::Diagnose));
        assert!(l.escalated());
    }

    #[test]
    fn names_round_trip() {
        for r in Rung::ALL {
            assert_eq!(Rung::parse(r.as_str()), Some(r));
        }
        assert_eq!(Rung::parse("nope"), None);
        // The rung JEV had was removed with it; its name is no rung.
        assert_eq!(Rung::parse("judge"), None);
        assert!(Rung::Fresh.restarts() && !Rung::Diagnose.restarts());
    }
}
