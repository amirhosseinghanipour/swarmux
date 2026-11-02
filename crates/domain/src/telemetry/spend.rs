//! Spend and burn from Claude Code transcripts: totals per day, week, model
//! and project, billing blocks, the activity calendar and the session wall.

use std::collections::HashMap;

use super::blocks::{BillingBlock, BurnRate, UsagePoint, billing_blocks};
use super::context::{ContextGauge, context_window};
use super::flow::{FlowBucket, cache_hit_ratio, series_of};
use super::pricing::{ModelPrice, PriceTable, TokenUsage};
use super::time::{DAY, HOUR, MINUTE, day_date, local_day, week_start, weekday};
use crate::Millis;

/// Days in the activity calendar (12 weeks).
pub const CALENDAR_DAYS: usize = 84;
/// Weeks in the weekly totals.
pub const WEEKS: usize = 12;
/// A session with a message this recent is live.
pub const LIVE_WITHIN: Millis = 2 * MINUTE;
/// Sessions listed on the wall, most recent first.
pub const SESSION_LIMIT: usize = 200;
/// Billing blocks kept, by age.
const BLOCK_HISTORY: Millis = 7 * DAY;

/// Hands out small ids for repeated strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interner {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.index.get(s) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(s.to_string());
        self.index.insert(s.to_string(), id);
        id
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// One assistant message from a transcript (never its text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageEntry {
    pub ts: Millis,
    pub session: u32,
    /// The working folder.
    pub project: u32,
    pub model: u32,
    /// Written by a subagent (a side chain), not the session's own turn.
    pub sidechain: bool,
    pub usage: TokenUsage,
}

/// Every known message, with interned names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageBook {
    pub entries: Vec<UsageEntry>,
    pub sessions: Interner,
    pub projects: Interner,
    pub models: Interner,
}

/// Money, tokens and messages of some slice of usage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spend {
    /// USD; messages of unpriced models add nothing.
    pub cost: f64,
    pub tokens: TokenUsage,
    pub messages: u64,
}

impl Spend {
    fn add(&mut self, cost: f64, usage: &TokenUsage) {
        self.cost += cost;
        self.tokens.add(usage);
        self.messages += 1;
    }
}

/// One day of the calendar.
#[derive(Clone, Debug, PartialEq)]
pub struct DaySpend {
    /// Local day number.
    pub day: i64,
    pub date: String,
    /// 0 = Monday.
    pub weekday: u32,
    pub spend: Spend,
    /// Heat 0 (nothing) to 4 (busiest quarter of active days).
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeekSpend {
    /// Local day number of its Monday.
    pub start_day: i64,
    pub start_date: String,
    pub spend: Spend,
}

/// Spend of one model or project.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedSpend {
    pub name: String,
    pub spend: Spend,
    /// Share of the group's cost (of its tokens when nothing is priced).
    pub share: f64,
    pub last_at: Millis,
}

/// One Claude Code session on this machine.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionUsage {
    pub session_id: String,
    /// The folder it works in.
    pub project: String,
    /// Model of its latest message.
    pub model: String,
    pub first_at: Millis,
    pub last_at: Millis,
    pub spend: Spend,
    /// Context of its latest own turn.
    pub context: ContextGauge,
    /// A message in the last [`LIVE_WITHIN`].
    pub live: bool,
    /// Started by swarmux (set by the application layer).
    pub swarmux: bool,
}

/// Everything the spend stations show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReport {
    pub generated_at: Millis,
    /// Local time offset the days were cut with, in minutes.
    pub offset_min: i32,
    pub today: Spend,
    pub week: Spend,
    pub total: Spend,
    /// [`CALENDAR_DAYS`] days ending today, oldest first.
    pub days: Vec<DaySpend>,
    /// [`WEEKS`] weeks ending with this one, oldest first.
    pub weeks: Vec<WeekSpend>,
    /// Per model over the calendar, costliest first.
    pub models: Vec<NamedSpend>,
    /// Per model today, costliest first.
    pub models_today: Vec<NamedSpend>,
    /// Per project folder over the calendar, costliest first.
    pub projects: Vec<NamedSpend>,
    /// Billing blocks of the last 7 days, oldest first.
    pub blocks: Vec<BillingBlock>,
    pub active_block: Option<BillingBlock>,
    /// Burn rate of the active block.
    pub burn: Option<BurnRate>,
    /// Machine-wide tokens per minute, last hour.
    pub flow: Vec<FlowBucket>,
    /// Cache reads over prompt tokens in the last hour.
    pub cache_hit_ratio: Option<f64>,
    pub cache_hit_ratio_today: Option<f64>,
    /// Most recently active first, at most [`SESSION_LIMIT`].
    pub sessions: Vec<SessionUsage>,
    pub sessions_total: usize,
    pub live_sessions: usize,
    /// Models seen without a known price (their cost counts as 0).
    pub unpriced_models: Vec<String>,
    pub messages: usize,
}

struct SessionAcc {
    first: Millis,
    last: Millis,
    spend: Spend,
    model: u32,
    project: u32,
    /// (time, context, model) of the latest own turn.
    context: Option<(Millis, u64, u32)>,
}

impl UsageBook {
    /// Adds a message and returns its index.
    pub fn push(
        &mut self,
        ts: Millis,
        session: &str,
        project: &str,
        model: &str,
        sidechain: bool,
        usage: TokenUsage,
    ) -> usize {
        let entry = UsageEntry {
            ts,
            session: self.sessions.intern(session),
            project: self.projects.intern(project),
            model: self.models.intern(model),
            sidechain,
            usage,
        };
        self.entries.push(entry);
        self.entries.len() - 1
    }

    /// Aggregates every entry as of `now`, cutting days at `offset_min`.
    pub fn report(&self, prices: &PriceTable, now: Millis, offset_min: i32) -> UsageReport {
        let model_prices: Vec<Option<ModelPrice>> = (0..self.models.len() as u32)
            .map(|m| prices.price_for(self.models.name(m)).copied())
            .collect();
        let today = local_day(now, offset_min);
        let first_day = today - CALENDAR_DAYS as i64 + 1;
        let this_week = week_start(today);
        let first_week = this_week - 7 * (WEEKS as i64 - 1);

        let mut r = UsageReport {
            generated_at: now,
            offset_min,
            ..UsageReport::default()
        };
        let mut days = vec![Spend::default(); CALENDAR_DAYS];
        let mut weeks = vec![Spend::default(); WEEKS];
        let mut models: HashMap<u32, (Spend, Millis)> = HashMap::new();
        let mut models_today: HashMap<u32, (Spend, Millis)> = HashMap::new();
        let mut projects: HashMap<u32, (Spend, Millis)> = HashMap::new();
        let mut sessions: HashMap<u32, SessionAcc> = HashMap::new();
        let mut recent: Vec<UsagePoint> = Vec::new();
        let mut flow: Vec<FlowBucket> = Vec::new();
        let mut unpriced: Vec<u32> = Vec::new();

        for e in &self.entries {
            let price = model_prices.get(e.model as usize).copied().flatten();
            if price.is_none() && e.usage.total() > 0 && !unpriced.contains(&e.model) {
                unpriced.push(e.model);
            }
            let cost = price.map_or(0.0, |p| p.cost(&e.usage));
            r.total.add(cost, &e.usage);
            let day = local_day(e.ts, offset_min);
            if (first_day..=today).contains(&day) {
                days[(day - first_day) as usize].add(cost, &e.usage);
                bump(&mut models, e.model, cost, e);
                bump(&mut projects, e.project, cost, e);
            }
            let week = week_start(day);
            if (first_week..=this_week).contains(&week) {
                weeks[((week - first_week) / 7) as usize].add(cost, &e.usage);
            }
            if day == today {
                r.today.add(cost, &e.usage);
                bump(&mut models_today, e.model, cost, e);
            }
            if week == this_week {
                r.week.add(cost, &e.usage);
            }
            if e.ts >= now - BLOCK_HISTORY && e.ts <= now {
                recent.push(UsagePoint {
                    ts: e.ts,
                    cost,
                    usage: e.usage,
                });
            }
            if e.ts > now - HOUR && e.ts <= now {
                flow.push(FlowBucket {
                    minute: e.ts.div_euclid(MINUTE),
                    input: e.usage.input,
                    output: e.usage.output,
                    cache_read: e.usage.cache_read,
                    cache_write: e.usage.cache_write(),
                });
            }
            observe_session(&mut sessions, e, cost);
        }

        // Heat by cost; a day with only unpriced tokens still shows as active.
        let costs: Vec<f64> = days
            .iter()
            .map(|d| d.cost.max(d.tokens.total() as f64 * 1e-12))
            .collect();
        let levels = heat_levels(&costs);
        r.days = days
            .into_iter()
            .enumerate()
            .map(|(i, spend)| {
                let day = first_day + i as i64;
                DaySpend {
                    day,
                    date: day_date(day),
                    weekday: weekday(day),
                    spend,
                    level: levels[i],
                }
            })
            .collect();
        r.weeks = weeks
            .into_iter()
            .enumerate()
            .map(|(i, spend)| {
                let start_day = first_week + 7 * i as i64;
                WeekSpend {
                    start_day,
                    start_date: day_date(start_day),
                    spend,
                }
            })
            .collect();
        r.models = named(models, |id| self.models.name(id));
        r.models_today = named(models_today, |id| self.models.name(id));
        r.projects = named(projects, |id| self.projects.name(id));

        recent.sort_by_key(|p| p.ts);
        r.blocks = billing_blocks(&recent, now);
        r.active_block = r.blocks.iter().rev().find(|b| b.active).cloned();
        r.burn = r.active_block.as_ref().and_then(|b| b.burn);

        r.flow = series_of(flow.into_iter(), now);
        r.cache_hit_ratio = cache_hit_ratio(&r.flow);
        r.cache_hit_ratio_today = r.today.tokens.cache_hit_ratio();

        r.sessions_total = sessions.len();
        let mut wall: Vec<SessionUsage> = sessions
            .into_iter()
            .map(|(id, s)| self.session_usage(id, s, now))
            .collect();
        wall.sort_by(|a, b| {
            b.last_at
                .cmp(&a.last_at)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        r.live_sessions = wall.iter().filter(|s| s.live).count();
        wall.truncate(SESSION_LIMIT);
        r.sessions = wall;
        r.unpriced_models = unpriced
            .into_iter()
            .map(|m| self.models.name(m).to_string())
            .filter(|m| m != "<synthetic>")
            .collect();
        r.messages = self.entries.len();
        r
    }

    fn session_usage(&self, id: u32, s: SessionAcc, now: Millis) -> SessionUsage {
        let context = s
            .context
            .map_or_else(ContextGauge::default, |(_, used, model)| {
                ContextGauge::new(used, context_window(self.models.name(model), used))
            });
        SessionUsage {
            session_id: self.sessions.name(id).to_string(),
            project: self.projects.name(s.project).to_string(),
            model: self.models.name(s.model).to_string(),
            first_at: s.first,
            last_at: s.last,
            spend: s.spend,
            context,
            live: now - s.last < LIVE_WITHIN,
            swarmux: false,
        }
    }
}

fn bump(map: &mut HashMap<u32, (Spend, Millis)>, key: u32, cost: f64, e: &UsageEntry) {
    let slot = map.entry(key).or_default();
    slot.0.add(cost, &e.usage);
    slot.1 = slot.1.max(e.ts);
}

fn observe_session(sessions: &mut HashMap<u32, SessionAcc>, e: &UsageEntry, cost: f64) {
    let s = sessions.entry(e.session).or_insert(SessionAcc {
        first: e.ts,
        last: e.ts,
        spend: Spend::default(),
        model: e.model,
        project: e.project,
        context: None,
    });
    s.spend.add(cost, &e.usage);
    s.first = s.first.min(e.ts);
    if e.ts >= s.last {
        s.last = e.ts;
        s.model = e.model;
        s.project = e.project;
    }
    if !e.sidechain && s.context.is_none_or(|(at, _, _)| e.ts >= at) {
        s.context = Some((e.ts, e.usage.context(), e.model));
    }
}

fn named<'a>(map: HashMap<u32, (Spend, Millis)>, name: impl Fn(u32) -> &'a str) -> Vec<NamedSpend> {
    let cost: f64 = map.values().map(|(s, _)| s.cost).sum();
    let tokens: u64 = map.values().map(|(s, _)| s.tokens.total()).sum();
    let mut out: Vec<NamedSpend> = map
        .into_iter()
        .map(|(id, (spend, last_at))| NamedSpend {
            name: name(id).to_string(),
            share: if cost > 0.0 {
                spend.cost / cost
            } else if tokens > 0 {
                spend.tokens.total() as f64 / tokens as f64
            } else {
                0.0
            },
            spend,
            last_at,
        })
        .collect();
    out.sort_by(|a, b| {
        b.spend
            .cost
            .total_cmp(&a.spend.cost)
            .then(b.spend.tokens.total().cmp(&a.spend.tokens.total()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Heat levels 0–4: 0 for nothing, else the quarter the value falls in among
/// the non-zero values.
pub fn heat_levels(values: &[f64]) -> Vec<u8> {
    let mut active: Vec<f64> = values.iter().copied().filter(|v| *v > 0.0).collect();
    active.sort_by(f64::total_cmp);
    values
        .iter()
        .map(|&v| {
            if v <= 0.0 {
                return 0;
            }
            let below = active.partition_point(|&a| a < v);
            (1 + below * 4 / active.len()).min(4) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Millis = 1_793_613_600_000; // 2026-11-02 10:00 UTC, a Monday

    fn out(n: u64) -> TokenUsage {
        TokenUsage {
            output: n,
            ..TokenUsage::default()
        }
    }

    fn book() -> UsageBook {
        let mut b = UsageBook::default();
        // Sonnet 4.6: 1M output = $15.
        b.push(
            NOW - 30 * MINUTE,
            "s1",
            "/w/a",
            "claude-sonnet-4-6",
            false,
            out(1_000_000),
        );
        b.push(
            NOW - MINUTE,
            "s1",
            "/w/a",
            "claude-sonnet-4-6",
            false,
            out(1_000_000),
        );
        // Yesterday (Sunday, last week).
        b.push(
            NOW - DAY,
            "s2",
            "/w/b",
            "claude-opus-4-1",
            false,
            out(100_000),
        );
        // A subagent turn of s1 doesn't set its context.
        let mut big = out(0);
        big.cache_read = 900_000;
        b.push(
            NOW - 10 * MINUTE,
            "s1",
            "/w/a",
            "claude-haiku-4-5",
            true,
            big,
        );
        // An unpriced model and a message outside the calendar.
        b.push(NOW - 2 * MINUTE, "s3", "/w/c", "mystery-1", false, out(10));
        b.push(
            NOW - 200 * DAY,
            "s4",
            "/w/d",
            "claude-sonnet-4-6",
            false,
            out(1_000_000),
        );
        b
    }

    #[test]
    fn totals_per_day_week_and_all_time() {
        let r = book().report(&PriceTable::builtin(), NOW, 0);
        assert!((r.today.cost - 30.09).abs() < 1e-9);
        assert_eq!(r.today.messages, 4);
        assert_eq!(r.week.messages, 4);
        assert_eq!(r.total.messages, 6);
        assert_eq!(r.days.len(), CALENDAR_DAYS);
        assert_eq!(r.days.last().map(|d| d.date.as_str()), Some("2026-11-02"));
        assert_eq!(r.days[CALENDAR_DAYS - 2].spend.cost, 7.5);
        assert_eq!(r.weeks.len(), WEEKS);
        assert_eq!(r.weeks[WEEKS - 2].spend.cost, 7.5);
        assert_eq!(r.unpriced_models, ["mystery-1"]);
    }

    #[test]
    fn local_offset_moves_the_day_boundary() {
        // At -11:00 it is still Sunday 23:00: today holds everything since 13:00 UTC Sunday.
        let r = book().report(&PriceTable::builtin(), NOW, -11 * 60);
        assert_eq!(r.days.last().map(|d| d.date.as_str()), Some("2026-11-01"));
        assert_eq!(r.today.messages, 4);
    }

    #[test]
    fn models_and_projects_rank_by_cost() {
        let r = book().report(&PriceTable::builtin(), NOW, 0);
        assert_eq!(r.models[0].name, "claude-sonnet-4-6");
        assert!((r.models[0].share - 30.0 / (30.0 + 7.5 + 0.09)).abs() < 1e-9);
        assert_eq!(r.projects[0].name, "/w/a");
        assert_eq!(r.models_today.len(), 3);
    }

    #[test]
    fn session_wall_marks_live_and_uses_own_context() {
        let r = book().report(&PriceTable::builtin(), NOW, 0);
        assert_eq!(r.sessions_total, 4);
        assert_eq!(r.sessions[0].session_id, "s1");
        assert!(r.sessions[0].live);
        assert_eq!(r.sessions[0].context.used, 0);
        assert_eq!(r.sessions[0].spend.messages, 3);
        assert!(!r.sessions[1].live); // s3, two minutes ago
        assert_eq!(r.live_sessions, 1);
    }

    #[test]
    fn active_block_burn_and_flow() {
        let r = book().report(&PriceTable::builtin(), NOW, 0);
        let b = r.active_block.as_ref().unwrap();
        assert_eq!(b.start, NOW - HOUR);
        assert_eq!(b.messages, 4);
        assert!(r.burn.is_some());
        assert_eq!(r.flow.len(), 60);
        assert_eq!(r.flow.iter().map(|f| f.output).sum::<u64>(), 2_000_010);
        assert_eq!(r.cache_hit_ratio, Some(1.0));
    }

    #[test]
    fn heat_levels_by_quarter() {
        assert_eq!(heat_levels(&[0.0, 1.0, 2.0, 3.0, 4.0]), [0, 1, 2, 3, 4]);
        assert_eq!(heat_levels(&[5.0, 5.0, 0.0]), [1, 1, 0]);
        assert_eq!(heat_levels(&[]), Vec::<u8>::new());
    }

    #[test]
    fn interner_reuses_ids() {
        let mut i = Interner::default();
        assert_eq!((i.intern("a"), i.intern("b"), i.intern("a")), (0, 1, 0));
        assert_eq!((i.name(1), i.name(9), i.len()), ("b", "", 2));
    }
}
