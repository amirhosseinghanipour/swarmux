//! Telling the user something needs them while swarmux is out of sight:
//! the [`Notifier`] port, the rule for when to use it, and the service that
//! applies the rule to the attention inbox and sends each item once.

use std::collections::HashSet;

use swarmux_domain::{AttentionId, AttentionItem, AttentionKind, Millis, Priority};

use crate::{AppError, AppResult};

/// One notification: short on purpose, since it may leave the machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub id: AttentionId,
    pub priority: Priority,
    pub kind: AttentionKind,
    pub title: String,
    pub body: String,
}

/// Delivers a notification: a desktop toast, a terminal sequence, a push.
pub trait Notifier {
    fn notify(&self, notification: &Notification) -> AppResult<()>;
}

/// A notifier that does nothing, for when notifications are off.
pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn notify(&self, _: &Notification) -> AppResult<()> {
        Ok(())
    }
}

/// The `[notify]` section.
#[derive(Clone, Debug, PartialEq)]
pub struct NotifySettings {
    /// The kill switch: off means nothing is ever sent, P1 included.
    pub enabled: bool,
    /// A desktop toast (Windows).
    pub toast: bool,
    /// What to write to the terminal as a fallback.
    pub terminal: TerminalMode,
    /// A push to this ntfy topic URL, when the user configured one.
    pub ntfy: Option<String>,
    /// Hours in which P2 stays silent. P1 ignores them.
    pub quiet: Option<QuietHours>,
}

impl Default for NotifySettings {
    fn default() -> NotifySettings {
        NotifySettings {
            enabled: true,
            toast: cfg!(windows),
            terminal: if cfg!(windows) {
                TerminalMode::Off
            } else {
                TerminalMode::Osc777
            },
            ntfy: None,
            quiet: None,
        }
    }
}

/// How to reach the user through the terminal itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMode {
    Off,
    /// `OSC 777;notify` (VTE, foot, WezTerm, Ghostty).
    Osc777,
    /// `OSC 9` (iTerm2, ConEmu, Kitty).
    Osc9,
    /// Just the bell character.
    Bell,
}

impl TerminalMode {
    pub fn parse(s: &str) -> Option<TerminalMode> {
        match s {
            "off" => Some(TerminalMode::Off),
            "osc777" => Some(TerminalMode::Osc777),
            "osc9" => Some(TerminalMode::Osc9),
            "bell" => Some(TerminalMode::Bell),
            _ => None,
        }
    }
}

/// A daily window in local minutes since midnight; may wrap midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietHours {
    from: u32,
    to: u32,
}

impl QuietHours {
    /// `"22:00-08:00"`.
    pub fn parse(s: &str) -> Option<QuietHours> {
        let (a, b) = s.split_once('-')?;
        let (from, to) = (clock_minutes(a)?, clock_minutes(b)?);
        (from != to).then_some(QuietHours { from, to })
    }

    /// Whether `minute` (local, since midnight) falls inside the window.
    pub fn contains(self, minute: u32) -> bool {
        if self.from < self.to {
            (self.from..self.to).contains(&minute)
        } else {
            minute >= self.from || minute < self.to
        }
    }
}

fn clock_minutes(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// What the rule needs to know about the moment.
#[derive(Clone, Copy, Debug)]
pub struct Moment {
    /// The terminal window is in the background.
    pub unfocused: bool,
    /// Local minutes since midnight.
    pub minute: u32,
}

/// Whether an item of `priority` may notify now. P3 never does. P1 always
/// does while notifications are on, focused or not, quiet hours or not. P2
/// waits for the terminal to be unfocused and for quiet hours to end.
pub fn may_notify(priority: Priority, settings: &NotifySettings, at: Moment) -> bool {
    if !settings.enabled {
        return false;
    }
    match priority {
        Priority::P1 => true,
        Priority::P2 => at.unfocused && !settings.quiet.is_some_and(|q| q.contains(at.minute)),
        Priority::P3 => false,
    }
}

/// Without focus events, the terminal counts as unfocused after this long
/// without a key.
pub const IDLE_UNFOCUSED_MS: Millis = 30_000;

/// Whether the terminal window has the user's attention. Uses the
/// terminal's focus reports; where it sends none, falls back to how long
/// ago the last key was.
#[derive(Clone, Copy, Debug, Default)]
pub struct FocusTracker {
    reported: Option<bool>,
    last_input: Millis,
}

impl FocusTracker {
    pub fn on_focus(&mut self, gained: bool) {
        self.reported = Some(gained);
    }

    pub fn on_input(&mut self, now: Millis) {
        self.last_input = now;
    }

    pub fn unfocused(&self, now: Millis) -> bool {
        match self.reported {
            Some(focused) => !focused,
            None => now - self.last_input >= IDLE_UNFOCUSED_MS,
        }
    }
}

/// More new items than this at once go out as one summary.
const BURST: usize = 3;
const BODY_MAX: usize = 120;

/// Sends each inbox item at most once, when the rule allows.
pub struct NotifyService {
    notifier: Box<dyn Notifier>,
    settings: NotifySettings,
    /// Minutes east of UTC now.
    utc_offset: fn() -> i32,
    sent: HashSet<AttentionId>,
}

impl NotifyService {
    pub fn new(
        notifier: Box<dyn Notifier>,
        settings: NotifySettings,
        utc_offset: fn() -> i32,
    ) -> NotifyService {
        NotifyService {
            notifier,
            settings,
            utc_offset,
            sent: HashSet::new(),
        }
    }

    /// Looks at the items that are due (the inbox after deferral) and sends
    /// what is new and allowed. Returns delivery errors for the caller to show.
    pub fn observe(
        &mut self,
        items: &[AttentionItem],
        unfocused: bool,
        now: Millis,
    ) -> Vec<AppError> {
        // A reused id notifies again once its item has gone.
        self.sent.retain(|id| items.iter().any(|i| &i.id == id));
        if !self.settings.enabled {
            return Vec::new();
        }
        let at = Moment {
            unfocused,
            minute: local_minute(now, (self.utc_offset)()),
        };
        let quiet = self.settings.quiet.is_some_and(|q| q.contains(at.minute));
        let mut fresh: Vec<&AttentionItem> = Vec::new();
        let mut lapsed: Vec<AttentionId> = Vec::new();
        for item in items.iter().filter(|i| !self.sent.contains(&i.id)) {
            if may_notify(item.priority, &self.settings, at) {
                fresh.push(item);
            } else if item.priority == Priority::P2 && quiet {
                // Quiet hours pass for good: no buzz at their end for old news.
                lapsed.push(item.id.clone());
            }
        }
        self.sent.extend(lapsed);
        fresh.sort_by_key(|i| i.priority);
        let notes = summarize(&fresh);
        for item in &fresh {
            self.sent.insert(item.id.clone());
        }
        notes
            .iter()
            .filter_map(|n| self.notifier.notify(n).err())
            .collect()
    }
}

fn summarize(fresh: &[&AttentionItem]) -> Vec<Notification> {
    let Some(first) = fresh.first() else {
        return Vec::new();
    };
    if fresh.len() <= BURST {
        return fresh.iter().map(|i| one(i)).collect();
    }
    vec![Notification {
        id: first.id.clone(),
        priority: first.priority,
        kind: first.kind,
        title: format!("swarmux: {} items need you", fresh.len()),
        body: truncate(&first.title),
    }]
}

fn one(item: &AttentionItem) -> Notification {
    Notification {
        id: item.id.clone(),
        priority: item.priority,
        kind: item.kind,
        title: format!("swarmux {}: {}", item.priority.as_str(), item.kind.as_str()),
        body: truncate(&item.title),
    }
}

fn truncate(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= BODY_MAX {
        return flat.to_string();
    }
    let cut: String = flat.chars().take(BODY_MAX - 1).collect();
    format!("{cut}…")
}

fn local_minute(now: Millis, offset_minutes: i32) -> u32 {
    let local = now.div_euclid(60_000) + i64::from(offset_minutes);
    u32::try_from(local.rem_euclid(24 * 60)).unwrap_or(0)
}

#[cfg(test)]
mod tests;
