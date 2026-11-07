//! The owner brief: when it is due, and what goes in it.
//!
//! A brief is a short list of decision cards, urgent ones first. Each card
//! says what is proposed, why it matters, where it came from and a reason
//! the owner might say no, so that approving is a decision and not a
//! reflex.

use super::{FollowKind, FollowUp, Stamped};
use crate::board::clean;
use crate::Millis;

const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * 60 * MINUTE;

/// Local times of day the brief is delivered at, in minutes since midnight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefSlots(Vec<u32>);

impl Default for BriefSlots {
    /// 09:00, 13:00 and 18:00: three batches a day.
    fn default() -> BriefSlots {
        BriefSlots(vec![9 * 60, 13 * 60, 18 * 60])
    }
}

impl BriefSlots {
    /// `["09:00", "13:00"]`; `None` when any entry is not `HH:MM`. An empty
    /// list means no scheduled brief.
    pub fn parse<S: AsRef<str>>(times: &[S]) -> Option<BriefSlots> {
        let mut v = Vec::new();
        for t in times {
            let (h, m) = t.as_ref().trim().split_once(':')?;
            if h.len() != 2 || m.len() != 2 {
                return None;
            }
            let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
            if h >= 24 || m >= 60 {
                return None;
            }
            v.push(h * 60 + m);
        }
        v.sort_unstable();
        v.dedup();
        Some(BriefSlots(v))
    }

    pub fn none() -> BriefSlots {
        BriefSlots(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn labels(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|m| format!("{:02}:{:02}", m / 60, m % 60))
            .collect()
    }
}

/// The latest slot that has come and has not been served: the instant it
/// fell on, when it is after `last`. Several missed slots count as one, so
/// a laptop that slept all day does not get three briefs at once.
/// `utc_offset` is minutes east of UTC.
pub fn due_slot(slots: &BriefSlots, now: Millis, utc_offset: i32, last: Millis) -> Option<Millis> {
    let offset = i64::from(utc_offset) * MINUTE;
    let today = (now + offset).div_euclid(DAY);
    (today - 1..=today)
        .flat_map(|day| {
            slots
                .0
                .iter()
                .map(move |m| day * DAY + i64::from(*m) * MINUTE - offset)
        })
        .filter(|at| *at <= now && *at > last)
        .max()
}

/// One decision card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: i64,
    pub kind: FollowKind,
    /// What is proposed or asked.
    pub what: String,
    /// Why it matters.
    pub why: String,
    /// Where it came from.
    pub evidence: String,
    /// A reason to say no.
    pub against: String,
    pub urgent: bool,
    pub repo: String,
}

impl Card {
    pub fn of(f: &FollowUp, urgent: bool) -> Card {
        Card {
            id: f.id,
            kind: f.kind,
            what: f.text.clone(),
            why: if f.rationale.is_empty() {
                "—".into()
            } else {
                f.rationale.clone()
            },
            evidence: evidence_line(f),
            against: reason_to_decline(f),
            urgent,
            repo: f.repo.clone(),
        }
    }
}

fn evidence_line(f: &FollowUp) -> String {
    let mut lines: Vec<String> = f.sources.iter().take(2).map(|s| s.line()).collect();
    if f.sources.len() > 2 {
        lines.push(format!("+{} more", f.sources.len() - 2));
    }
    if lines.is_empty() {
        "—".into()
    } else {
        lines.join("; ")
    }
}

/// A reason the owner might say no: the finder's own, or a fixed one for the
/// kind and size of it.
pub fn reason_to_decline(f: &FollowUp) -> String {
    if !f.against.is_empty() {
        return clean(&f.against, super::NOTE_MAX);
    }
    match f.kind {
        FollowKind::SuggestedTask => {
            if f.paths.len() >= 4 {
                format!(
                    "It touches {} files; is it worth the review time?",
                    f.paths.len()
                )
            } else if let Some(c) = f.est_cost_usd {
                format!("It costs about ${c:.2} that you did not plan to spend.")
            } else {
                "It is work you did not ask for; agents tend to suggest more than is needed.".into()
            }
        }
        FollowKind::Decision => {
            "You may want the other option; the agent chose with less context than you have.".into()
        }
        FollowKind::Risk => "It may be a false alarm; open the evidence before acting.".into(),
        FollowKind::Blocker => {
            "The work may be able to go around it; check it is really in the way.".into()
        }
        FollowKind::Question => "You may prefer to change the task than to answer this.".into(),
    }
}

/// What goes into one brief.
pub struct BriefInput<'a> {
    pub now: Millis,
    /// Jobs finished since the last brief, and how many met their checks.
    pub finished: usize,
    pub met_checks: usize,
    /// Everything waiting for the owner, with whether each is urgent.
    pub waiting: &'a [(FollowUp, bool)],
    /// Cards shown; the rest are counted.
    pub max_cards: usize,
    /// Kinds the owner approves by reflex.
    pub stamped: &'a [Stamped],
    /// An approved follow-up to open the evidence of: `(id, text)`.
    pub spot_check: Option<(i64, String)>,
}

/// The brief.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerBrief {
    pub at: Millis,
    pub finished: usize,
    pub met_checks: usize,
    pub cards: Vec<Card>,
    /// Waiting follow-ups beyond the cards shown.
    pub more: usize,
    pub stamped: Vec<Stamped>,
    pub spot_check: Option<(i64, String)>,
}

impl OwnerBrief {
    /// `3 tasks finished, 2 met their checks; 2 decisions need you`.
    pub fn headline(&self) -> String {
        let n = self.cards.len() + self.more;
        let finished = format!(
            "{} {} finished, {} met their checks",
            self.finished,
            if self.finished == 1 { "task" } else { "tasks" },
            self.met_checks
        );
        if n == 0 {
            format!("{finished}; nothing needs you")
        } else {
            format!(
                "{finished}; {n} {} you",
                if n == 1 { "decision needs" } else { "decisions need" }
            )
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty() && self.more == 0
    }

    /// Plain text, for the terminal and the CLI.
    pub fn text(&self) -> String {
        let mut out = self.headline();
        for (i, c) in self.cards.iter().enumerate() {
            out.push_str(&format!(
                "\n{}) {}#{} [{}] {}\n   why: {}\n   from: {}\n   you might say no because: {}",
                i + 1,
                if c.urgent { "URGENT " } else { "" },
                c.id,
                c.kind.as_str(),
                c.what,
                c.why,
                c.evidence,
                c.against
            ));
        }
        if self.more > 0 {
            out.push_str(&format!("\n+{} more in the follow-ups view", self.more));
        }
        if let Some(note) = self.spot_note() {
            out.push_str(&format!("\n\n{note}"));
        }
        out
    }

    /// The gentle prompt for a spot check, when one is due.
    pub fn spot_note(&self) -> Option<String> {
        let (id, text) = self.spot_check.as_ref()?;
        let s = self.stamped.first()?;
        Some(format!(
            "You approved {} of your last {} {} follow-ups. Please open the evidence of \
             #{id} (\"{}\") and check it still holds.",
            s.approved,
            s.total,
            s.kind.as_str(),
            clean(text, 80)
        ))
    }
}

/// Builds the brief: urgent first, then risks, blockers, decisions,
/// questions and suggested tasks, the longest waiting first within each.
pub fn compose(input: &BriefInput<'_>) -> OwnerBrief {
    let mut waiting: Vec<&(FollowUp, bool)> = input.waiting.iter().collect();
    waiting.sort_by_key(|(f, urgent)| {
        (
            !urgent,
            f.kind.weight(),
            std::cmp::Reverse(f.urgency),
            f.created,
            f.id,
        )
    });
    let more = waiting.len().saturating_sub(input.max_cards);
    let cards = waiting
        .iter()
        .take(input.max_cards)
        .map(|(f, urgent)| Card::of(f, *urgent))
        .collect();
    OwnerBrief {
        at: input.now,
        finished: input.finished,
        met_checks: input.met_checks,
        cards,
        more,
        stamped: input.stamped.to_vec(),
        spot_check: input.spot_check.clone(),
    }
}

/// Picks one of `ids` without a random generator: the same seed picks the
/// same one, a different time another.
pub fn pick(ids: &[i64], seed: Millis) -> Option<i64> {
    if ids.is_empty() {
        return None;
    }
    let n = ids.len() as u64;
    ids.get((seed.unsigned_abs() / 1000 % n) as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobId;
    use crate::followup::{Source, Stamped};

    fn card(kind: FollowKind, text: &str, id: i64, created: Millis) -> FollowUp {
        let mut f = FollowUp::new(
            kind,
            text,
            Source {
                job: Some(JobId(id)),
                ..Source::default()
            },
            created,
        );
        f.id = id;
        f
    }

    const H: i64 = 3_600_000;

    #[test]
    fn slots_parse_and_reject_bad_times() {
        let s = BriefSlots::parse(&["13:00", "09:00", "09:00"]).unwrap();
        assert_eq!(s.labels(), ["09:00", "13:00"]);
        assert!(BriefSlots::parse(&["9:00"]).is_none());
        assert!(BriefSlots::parse(&["24:00"]).is_none());
        assert!(BriefSlots::parse::<&str>(&[]).unwrap().is_empty());
        assert_eq!(BriefSlots::default().labels(), ["09:00", "13:00", "18:00"]);
    }

    #[test]
    fn a_slot_is_due_once_it_has_come_and_not_before() {
        let s = BriefSlots::default();
        let day = 20_000 * DAY;
        // UTC+2: 09:00 local is 07:00 UTC.
        let off = 120;
        let nine = day + 7 * H;
        assert_eq!(due_slot(&s, nine - 1, off, nine - 5 * H), None);
        assert_eq!(due_slot(&s, nine, off, nine - 5 * H), Some(nine));
        assert_eq!(due_slot(&s, nine + H, off, nine), None, "served");
        // 13:00 local = 11:00 UTC.
        assert_eq!(due_slot(&s, day + 11 * H, off, nine), Some(day + 11 * H));
    }

    #[test]
    fn missed_slots_collapse_into_the_latest_one() {
        let s = BriefSlots::default();
        let day = 20_000 * DAY;
        let evening = day + 20 * H;
        // Slept since yesterday evening: one brief, for the 18:00 slot.
        assert_eq!(due_slot(&s, evening, 0, day - 10 * H), Some(day + 18 * H));
    }

    #[test]
    fn no_slots_no_brief() {
        assert_eq!(due_slot(&BriefSlots::none(), 100 * DAY, 0, 0), None);
    }

    #[test]
    fn cards_put_urgent_first_then_risk_over_tasks_then_oldest() {
        let waiting = vec![
            (card(FollowKind::SuggestedTask, "old task", 1, 10), false),
            (card(FollowKind::SuggestedTask, "new task", 2, 50), false),
            (card(FollowKind::Decision, "A or B", 3, 30), false),
            (card(FollowKind::Risk, "leak", 4, 40), false),
            (card(FollowKind::Blocker, "needs migration", 5, 60), true),
        ];
        let brief = compose(&BriefInput {
            now: 100,
            finished: 3,
            met_checks: 2,
            waiting: &waiting,
            max_cards: 4,
            stamped: &[],
            spot_check: None,
        });
        let ids: Vec<i64> = brief.cards.iter().map(|c| c.id).collect();
        assert_eq!(ids, [5, 4, 3, 1]);
        assert_eq!(brief.more, 1);
        assert!(brief.cards[0].urgent);
        assert_eq!(
            brief.headline(),
            "3 tasks finished, 2 met their checks; 5 decisions need you"
        );
    }

    #[test]
    fn every_card_carries_a_reason_to_say_no() {
        let mut f = card(FollowKind::SuggestedTask, "tidy", 1, 0);
        assert!(!Card::of(&f, false).against.is_empty());
        f.paths = (0..4).map(|i| format!("f{i}.rs")).collect();
        assert!(Card::of(&f, false).against.contains("4 files"));
        f.against = "the agent itself doubts it".into();
        assert_eq!(Card::of(&f, false).against, "the agent itself doubts it");
        assert_eq!(Card::of(&f, false).why, "—");
        for kind in FollowKind::ALL {
            assert!(!reason_to_decline(&card(kind, "x", 1, 0)).is_empty());
        }
    }

    #[test]
    fn an_empty_brief_says_nothing_needs_you() {
        let brief = compose(&BriefInput {
            now: 0,
            finished: 1,
            met_checks: 1,
            waiting: &[],
            max_cards: 5,
            stamped: &[],
            spot_check: None,
        });
        assert!(brief.is_empty());
        assert_eq!(
            brief.headline(),
            "1 task finished, 1 met their checks; nothing needs you"
        );
    }

    #[test]
    fn the_spot_check_note_names_the_item_and_the_rate() {
        let stamped = [Stamped {
            kind: FollowKind::SuggestedTask,
            approved: 24,
            total: 24,
        }];
        let brief = compose(&BriefInput {
            now: 0,
            finished: 0,
            met_checks: 0,
            waiting: &[],
            max_cards: 5,
            stamped: &stamped,
            spot_check: Some((12, "add retries".into())),
        });
        let note = brief.spot_note().unwrap();
        assert!(note.contains("24 of your last 24 task"));
        assert!(note.contains("#12"));
        assert!(brief.text().contains("#12"));
    }

    #[test]
    fn pick_is_stable_for_a_seed() {
        assert_eq!(pick(&[], 5), None);
        assert_eq!(pick(&[7, 8, 9], 1000), pick(&[7, 8, 9], 1000));
        assert_eq!(pick(&[7], 123_456), Some(7));
    }
}
