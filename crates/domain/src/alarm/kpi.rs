//! Alarm key figures and flood detection: how many items arrive, how they
//! split over the priorities, how long they wait to be noticed and which
//! ones have been standing for too long.

use std::collections::{BTreeMap, HashSet};

use crate::{AttentionId, AttentionItem, Millis, Priority};

/// The span a flood is counted over.
pub const FLOOD_WINDOW_MS: Millis = 10 * 60_000;
/// This many items inside the window is a flood.
pub const FLOOD_AT: usize = 10;
/// An item nobody has looked at for this long is stale.
pub const STALE_AFTER_MS: Millis = 30 * 60_000;
/// An item open this long is standing.
pub const STANDING_AFTER_MS: Millis = 4 * 60 * 60_000;

/// An item that entered the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raised {
    pub at: Millis,
    pub priority: Priority,
}

/// The figures the alarm KPIs report.
#[derive(Clone, Debug, PartialEq)]
pub struct Kpis {
    /// Items raised in the last ten minutes, any priority.
    pub last_10min: usize,
    /// P1 and P2 items raised in the last ten minutes; the target is one.
    pub urgent_last_10min: usize,
    /// P1 and P2 items per ten minutes over the last hour (or as long as
    /// there has been to watch).
    pub urgent_avg_10min: f64,
    /// The most items inside any ten minutes.
    pub peak_10min: usize,
    /// Items raised so far at P1, P2, P3.
    pub distribution: [usize; 3],
    /// The last ten minutes were a flood.
    pub flood: bool,
    /// Open items nobody has looked at for [`STALE_AFTER_MS`].
    pub stale: usize,
    /// Open items older than [`STANDING_AFTER_MS`].
    pub standing: usize,
    /// How many items were looked at, and the median time that took.
    pub attended: usize,
    pub ttd_median: Option<Millis>,
}

impl Kpis {
    /// P1, P2, P3 as shares of what was raised, in percent.
    pub fn shares(&self) -> [f64; 3] {
        let total: usize = self.distribution.iter().sum();
        if total == 0 {
            return [0.0; 3];
        }
        self.distribution.map(|n| n as f64 * 100.0 / total as f64)
    }
}

fn slot(p: Priority) -> usize {
    match p {
        Priority::P1 => 0,
        Priority::P2 => 1,
        Priority::P3 => 2,
    }
}

/// The key figures at `now`, from what was raised, the detection times
/// (milliseconds from raise to first look) and what is open.
pub fn compute_kpis(
    raised: &[Raised],
    ttds: &[Millis],
    open: &[AttentionItem],
    now: Millis,
) -> Kpis {
    let mut sorted = raised.to_vec();
    sorted.sort_by_key(|r| r.at);
    let recent = |r: &&Raised| now - r.at < FLOOD_WINDOW_MS && r.at <= now;
    let last_10min = sorted.iter().filter(recent).count();
    let urgent_last_10min = sorted
        .iter()
        .filter(recent)
        .filter(|r| r.priority != Priority::P3)
        .count();

    let hour = 6 * FLOOD_WINDOW_MS;
    let first = sorted.first().map_or(now, |r| r.at);
    let watched = (now - first).clamp(FLOOD_WINDOW_MS, hour);
    let urgent_hour = sorted
        .iter()
        .filter(|r| now - r.at < hour && r.at <= now && r.priority != Priority::P3)
        .count();
    let urgent_avg_10min = urgent_hour as f64 * FLOOD_WINDOW_MS as f64 / watched as f64;

    let mut peak = 0;
    let mut from = 0;
    for (to, r) in sorted.iter().enumerate() {
        while r.at - sorted[from].at >= FLOOD_WINDOW_MS {
            from += 1;
        }
        peak = peak.max(to + 1 - from);
    }

    let mut distribution = [0; 3];
    for r in &sorted {
        distribution[slot(r.priority)] += 1;
    }

    let mut ttds = ttds.to_vec();
    ttds.sort_unstable();
    let ttd_median = (!ttds.is_empty()).then(|| ttds[ttds.len() / 2]);

    Kpis {
        last_10min,
        urgent_last_10min,
        urgent_avg_10min,
        peak_10min: peak,
        distribution,
        flood: last_10min >= FLOOD_AT,
        stale: open
            .iter()
            .filter(|i| now - i.since >= STALE_AFTER_MS)
            .count(),
        standing: open
            .iter()
            .filter(|i| now - i.since >= STANDING_AFTER_MS)
            .count(),
        attended: ttds.len(),
        ttd_median,
    }
}

/// Items that share a root cause, with the first one in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloodGroup {
    pub cause: String,
    pub first_out: AttentionId,
    pub count: usize,
}

/// A flood: many items at once, folded by cause.
#[derive(Clone, Debug, PartialEq)]
pub struct Flood {
    /// Items inside the window.
    pub total: usize,
    /// Distinct causes among them.
    pub causes: usize,
    /// Causes with more than one item, biggest first.
    pub groups: Vec<FloodGroup>,
    suppressed: HashSet<AttentionId>,
}

impl Flood {
    /// Items hidden because an earlier item of the same cause is shown.
    pub fn suppressed(&self) -> &HashSet<AttentionId> {
        &self.suppressed
    }

    pub fn is_suppressed(&self, id: &AttentionId) -> bool {
        self.suppressed.contains(id)
    }

    /// `flood: 14 items from 1 cause`.
    pub fn banner(&self) -> String {
        format!(
            "flood: {} items from {} cause{}",
            self.total,
            self.causes,
            if self.causes == 1 { "" } else { "s" }
        )
    }
}

/// Finds a flood among the open items: [`FLOOD_AT`] or more that began in
/// the last ten minutes. The first item of every cause stays; the
/// consequentials of it are suppressed. P1 is never suppressed.
pub fn detect_flood(items: &[AttentionItem], now: Millis) -> Option<Flood> {
    let recent: Vec<&AttentionItem> = items
        .iter()
        .filter(|i| now - i.since < FLOOD_WINDOW_MS)
        .collect();
    if recent.len() < FLOOD_AT {
        return None;
    }
    let mut by_cause: BTreeMap<String, Vec<&AttentionItem>> = BTreeMap::new();
    for i in &recent {
        by_cause.entry(i.cause()).or_default().push(i);
    }
    let mut groups = Vec::new();
    let mut suppressed = HashSet::new();
    for (cause, mut members) in by_cause {
        members.sort_by(|a, b| {
            (a.since, a.request.unwrap_or(u64::MAX), &a.id).cmp(&(
                b.since,
                b.request.unwrap_or(u64::MAX),
                &b.id,
            ))
        });
        if members.len() < 2 {
            continue;
        }
        let first = members[0];
        for other in &members[1..] {
            if other.priority != Priority::P1 {
                suppressed.insert(other.id.clone());
            }
        }
        groups.push(FloodGroup {
            cause,
            first_out: first.id.clone(),
            count: members.len(),
        });
    }
    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.cause.cmp(&b.cause)));
    let causes = recent
        .iter()
        .map(|i| i.cause())
        .collect::<HashSet<_>>()
        .len();
    Some(Flood {
        total: recent.len(),
        causes,
        groups,
        suppressed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttentionKind, Likelihood};

    const MIN: Millis = 60_000;

    fn raised(at_min: &[(i64, Priority)]) -> Vec<Raised> {
        at_min
            .iter()
            .map(|(m, p)| Raised {
                at: m * MIN,
                priority: *p,
            })
            .collect()
    }

    fn open(key: &str, kind: AttentionKind, priority: Priority, since: Millis) -> AttentionItem {
        AttentionItem {
            id: AttentionId::new(key),
            kind,
            priority,
            title: key.into(),
            detail: String::new(),
            session: None,
            job: None,
            request: None,
            since,
            deadline: None,
            likelihood: Likelihood::Certain,
            risk: None,
            signature: None,
            batch: 1,
            suggest: None,
        }
    }

    #[test]
    fn rates_peaks_and_the_distribution() {
        let r = raised(&[
            (0, Priority::P3),
            (1, Priority::P2),
            (2, Priority::P3),
            (3, Priority::P3),
            (50, Priority::P1),
            (55, Priority::P2),
        ]);
        let k = compute_kpis(&r, &[1_000, 3_000, 9_000], &[], 56 * MIN);
        assert_eq!(k.last_10min, 2);
        assert_eq!(k.urgent_last_10min, 2);
        assert_eq!(k.peak_10min, 4);
        assert_eq!(k.distribution, [1, 2, 3]);
        assert!(!k.flood);
        assert_eq!(k.ttd_median, Some(3_000));
        assert_eq!(k.attended, 3);
        let shares = k.shares();
        assert!((shares[2] - 50.0).abs() < 1e-9);
        assert!(k.urgent_avg_10min > 0.0);
    }

    #[test]
    fn ten_in_ten_minutes_is_a_flood() {
        let nine = raised(&[(0, Priority::P3); 9]);
        assert!(!compute_kpis(&nine, &[], &[], 5 * MIN).flood);
        let ten = raised(&[(0, Priority::P3); 10]);
        assert!(compute_kpis(&ten, &[], &[], 5 * MIN).flood);
        assert!(!compute_kpis(&ten, &[], &[], 11 * MIN).flood);
    }

    #[test]
    fn stale_and_standing_count_the_open_items() {
        let items = [
            open("a", AttentionKind::JobFailed, Priority::P2, 0),
            open("b", AttentionKind::JobFailed, Priority::P2, 100 * MIN),
            open("c", AttentionKind::JobFailed, Priority::P2, 5 * 60 * MIN),
        ];
        let k = compute_kpis(&[], &[], &items, 5 * 60 * MIN + 10 * MIN);
        assert_eq!((k.stale, k.standing), (2, 1));
        assert_eq!(k.ttd_median, None);
    }

    #[test]
    fn a_flood_keeps_the_first_of_each_cause_and_never_hides_p1() {
        let mut items: Vec<_> = (0..12)
            .map(|n| {
                open(
                    &format!("job:{n}:failed"),
                    AttentionKind::JobFailed,
                    Priority::P2,
                    1_000 + n,
                )
            })
            .collect();
        items.push(open(
            "job:50:result",
            AttentionKind::ResultReady,
            Priority::P3,
            1_500,
        ));
        let flood = detect_flood(&items, 5_000).expect("13 items in 10 minutes");
        assert_eq!((flood.total, flood.causes), (13, 2));
        assert_eq!(flood.banner(), "flood: 13 items from 2 causes");
        assert_eq!(flood.groups.len(), 1);
        assert_eq!(flood.groups[0].count, 12);
        assert_eq!(flood.groups[0].first_out.as_str(), "job:0:failed");
        assert_eq!(flood.suppressed().len(), 11);
        assert!(!flood.is_suppressed(&AttentionId::new("job:0:failed")));
        assert!(!flood.is_suppressed(&AttentionId::new("job:50:result")));

        items[5].priority = Priority::P1;
        let flood = detect_flood(&items, 5_000).expect("still a flood");
        assert!(!flood.is_suppressed(&items[5].id), "P1 stays");
        assert!(detect_flood(&items[..9], 5_000).is_none());
        assert!(detect_flood(&items, 5_000 + 11 * MIN).is_none());
    }
}
