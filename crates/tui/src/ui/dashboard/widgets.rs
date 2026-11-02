//! Small pieces shared by the dashboard panels: spinners, bars, sparklines,
//! colours per model and text fitting.

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, BorderType},
};
use swarmux_domain::{Family, JobStatus};

use crate::ui::{BLUE, DIM, LABEL, MUTED, ORANGE, RED, short};

const SPINNER: [&str; 4] = ["[|]", "[/]", "[-]", "[\\]"];
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The spinner for frame `frame` (it turns every other frame).
pub fn spinner(frame: u64) -> &'static str {
    SPINNER[((frame / 2) % 4) as usize]
}

/// A spinner while `status` is running, otherwise how it ended.
pub fn status_mark(status: JobStatus, frame: u64) -> Span<'static> {
    match status {
        JobStatus::Running => Span::styled(spinner(frame), Style::new().fg(ORANGE)),
        JobStatus::Done => Span::styled("[✓]", Style::new().fg(crate::ui::GREEN)),
        JobStatus::Queued => Span::styled("[·]", Style::new().fg(MUTED)),
        _ => Span::styled("[x]", Style::new().fg(RED)),
    }
}

/// Colour of a model: orange for Opus and Sonnet, blue for Haiku, muted
/// when unknown.
pub fn model_color(model: Option<&str>) -> Color {
    match model.map(Family::of) {
        Some(Family::Haiku) => BLUE,
        Some(_) => ORANGE,
        None => MUTED,
    }
}

/// `fraction` of `width` cells filled: `██████░░░░`.
pub fn bar(fraction: f64, width: usize) -> String {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = ((fraction * width as f64).round() as usize).min(width);
    let mut s = String::with_capacity(width * 3);
    s.extend(std::iter::repeat_n('█', filled));
    s.extend(std::iter::repeat_n('░', width - filled));
    s
}

/// Activity over `[start, end]` as a sparkline `width` cells wide: each
/// cell is as tall as the number of marks that fell in it.
pub fn sparkline(marks: &[u32], start: u32, end: u32, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut buckets = vec![0u32; width];
    let span = end.saturating_sub(start).max(1) as u64;
    for &m in marks.iter().filter(|&&m| m >= start && m <= end) {
        let x = (u64::from(m - start) * (width as u64 - 1) / span) as usize;
        buckets[x.min(width - 1)] += 1;
    }
    let peak = buckets.iter().copied().max().unwrap_or(0);
    buckets
        .iter()
        .map(|&n| match n {
            0 => ' ',
            n => SPARK[((n as usize * SPARK.len()).div_ceil(peak as usize) - 1).min(7)],
        })
        .collect()
}

/// `s` cut to `max` characters, with `…` when cut.
pub fn fit(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    match s.char_indices().nth(max) {
        Some(_) => {
            let keep: String = s.chars().take(max - 1).collect();
            format!("{keep}…")
        }
        None => s.to_string(),
    }
}

/// Tokens as `15.1k`, or `—` when there are none to report.
pub fn tokens(n: u64) -> String {
    if n == 0 { "—".into() } else { short(n) }
}

/// A panel with a thin border in `color` and a coloured, bold title.
pub fn panel(title: String, color: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Span::styled(
            title,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ))
}

/// A panel with a dashed border, for the silent reviewer.
pub fn dashed(title: String, color: Color) -> Block<'static> {
    panel(title, color).border_type(BorderType::LightDoubleDashed)
}

pub fn label(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(LABEL))
}

pub fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(DIM))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_turns_every_other_frame() {
        let frames: Vec<&str> = (0..8).map(spinner).collect();
        assert_eq!(
            frames,
            ["[|]", "[|]", "[/]", "[/]", "[-]", "[-]", "[\\]", "[\\]"]
        );
    }

    #[test]
    fn bars_and_sparklines() {
        assert_eq!(bar(0.5, 4), "██░░");
        assert_eq!(bar(f64::NAN, 3), "░░░");
        assert_eq!(bar(2.0, 2), "██");
        assert_eq!(sparkline(&[0, 0, 0, 10], 0, 10, 5), "█   ▃");
        assert_eq!(sparkline(&[], 0, 0, 3), "   ");
        assert_eq!(sparkline(&[5], 0, 10, 0), "");
    }

    #[test]
    fn fitting_text() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 4), "abc");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(tokens(0), "—");
        assert_eq!(tokens(15_100), "15.1k");
    }

    #[test]
    fn model_colours() {
        assert_eq!(model_color(Some("claude-haiku-5-5")), BLUE);
        assert_eq!(model_color(Some("sonnet")), ORANGE);
        assert_eq!(model_color(None), MUTED);
    }
}
