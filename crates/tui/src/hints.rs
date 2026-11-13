//! The footer: each screen lists its hints (see [`crate::screens::Hint`])
//! and this writes them from the live keymap, so a rebound key shows as
//! what it is now, and fits them to the room there is.

use ratatui::text::Line;
use swarmux_domain::keys::{Action, actions};

use crate::app::App;
use crate::keys::HELP_TOGGLE;
use crate::screens::{self, Hint};

/// Separates hints.
const SEP: &str = " · ";

/// Width of `text` in terminal cells.
fn cells(text: &str) -> usize {
    Line::from(text).width()
}

/// Arrows and Enter as symbols; everything else as it is written.
fn symbol(label: &str) -> &str {
    match label {
        "up" => "↑",
        "down" => "↓",
        "left" => "←",
        "right" => "→",
        "enter" => "⏎",
        other => other,
    }
}

/// The hints of the screen as they read now, in order. A hint whose keys
/// are all unbound is left out.
fn pieces(app: &App) -> Vec<String> {
    app.screen
        .def()
        .hints(app)
        .iter()
        .filter_map(|h| piece(app, h))
        .collect()
}

fn piece(app: &App, hint: &Hint) -> Option<String> {
    match hint {
        Hint::Fixed { keys, text } => Some(format!("{keys} {text}")),
        Hint::Keys {
            actions: acts,
            text,
            symbols,
        } => {
            let labels: Vec<String> = acts.iter().filter_map(|a| app.key_of(*a)).collect();
            if labels.is_empty() {
                return None;
            }
            let text = if app.in_service() && **acts == [actions::QUIT] {
                // `q` lets the window go; the background service keeps running.
                "detach"
            } else {
                *text
            };
            let keys = if *symbols {
                let shown: Vec<&str> = labels.iter().map(|l| symbol(l)).collect();
                // `↑↓`, but `k/j`.
                let arrows = shown.iter().all(|s| matches!(*s, "↑" | "↓" | "←" | "→"));
                shown.join(if arrows { "" } else { "/" })
            } else {
                labels.join("/")
            };
            Some(format!("{keys} {text}"))
        }
        Hint::Screens => screens_piece(app),
    }
}

/// `0-9 screens` when the screens are on digits, else nothing (`? help`
/// lists them).
fn screens_piece(app: &App) -> Option<String> {
    let digits: Option<Vec<char>> = screens::all()
        .map(|s| {
            let label = crate::keys::nav_label(app.keymap(), s);
            let mut chars = label.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_digit() => Some(c),
                _ => None,
            }
        })
        .collect();
    match digits {
        Some(d) if !d.is_empty() => {
            let (lo, hi) = (d.iter().min()?, d.iter().max()?);
            Some(format!("{lo}-{hi} screens"))
        }
        _ => None,
    }
}

/// Every hint, in one line, however wide.
pub(crate) fn full(app: &App) -> String {
    join(&pieces(app))
}

fn join(pieces: &[String]) -> String {
    if pieces.is_empty() {
        String::new()
    } else {
        format!(" {}", pieces.join(SEP))
    }
}

/// The hints of the screen with whether each is pinned: the way out, the
/// key list and the screen's last hint stay as long as anything does.
fn pinned_pieces(app: &App) -> Vec<(String, bool)> {
    let hints = app.screen.def().hints(app);
    let last = hints.len().saturating_sub(1);
    hints
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let pinned = i == last
                || matches!(h, Hint::Keys { actions: a, .. }
                    if **a == [actions::QUIT] || **a == [HELP_TOGGLE]);
            piece(app, h).map(|p| (p, pinned))
        })
        .collect()
}

fn joined(kept: &[(String, bool)]) -> String {
    join(&kept.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>())
}

/// The hints that fit in `width` cells, whole. The others go first, from
/// the end; the pinned ones (`? help`, `q quit`) are the last to go, the
/// help before the way out. A hint is never cut in half.
pub(crate) fn fit(app: &App, width: usize) -> String {
    let all = pinned_pieces(app);
    let mut kept = all.clone();
    loop {
        let line = joined(&kept);
        if cells(&line) <= width {
            return line;
        }
        match kept.iter().rposition(|(_, pinned)| !pinned) {
            Some(i) => {
                kept.remove(i);
            }
            None => break,
        }
    }
    // Only the pinned ones are left and they do not fit: the way out alone,
    // else nothing.
    let last = joined(&all[all.len().saturating_sub(1)..]);
    if cells(&last) <= width {
        last
    } else {
        String::new()
    }
}

/// The key of `action` for a message: `x`, or its name when it has none.
pub(crate) fn key_or_name(app: &App, action: Action) -> String {
    app.key_of(action)
        .unwrap_or_else(|| format!("`{}`", action.id()))
}

/// Like [`key_or_name`], with arrows and Enter as symbols.
pub(crate) fn key_symbol(app: &App, action: Action) -> String {
    app.key_of(action)
        .map(|k| symbol(&k).to_string())
        .unwrap_or_else(|| format!("`{}`", action.id()))
}

#[cfg(test)]
mod tests;
