//! Small pieces shared by the dashboard panels: status marks, text bars and
//! sparklines, themed labels and text fitting.

use ratatui::{style::Style, text::Span};
use swarmux_domain::JobStatus;

use crate::theme::Theme;
use crate::ui::short;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// `[spinner]` while running, otherwise how it ended.
pub fn status_mark(t: &Theme, status: JobStatus, spin: &str) -> Span<'static> {
    match status {
        JobStatus::Running => Span::styled(format!("[{spin}]"), Style::new().fg(t.lead)),
        JobStatus::Done => Span::styled("[✓]", Style::new().fg(t.ok)),
        JobStatus::Queued => Span::styled("[·]", Style::new().fg(t.muted)),
        _ => Span::styled("[x]", Style::new().fg(t.error)),
    }
}

/// Activity over `[start, end]` as `width` counts: how many marks fell in
/// each cell.
pub fn buckets(marks: &[u32], start: u32, end: u32, width: usize) -> Vec<f64> {
    if width == 0 {
        return Vec::new();
    }
    let mut buckets = vec![0.0; width];
    let span = end.saturating_sub(start).max(1) as u64;
    for &m in marks.iter().filter(|&&m| m >= start && m <= end) {
        let x = (u64::from(m - start) * (width as u64 - 1) / span) as usize;
        if let Some(b) = buckets.get_mut(x.min(width - 1)) {
            *b += 1.0;
        }
    }
    buckets
}

/// Activity over `[start, end]` as a sparkline `width` cells wide: each
/// cell is as tall as the number of marks that fell in it.
pub fn sparkline(marks: &[u32], start: u32, end: u32, width: usize) -> String {
    let buckets = buckets(marks, start, end, width);
    let peak = buckets.iter().copied().fold(0.0, f64::max);
    buckets
        .iter()
        .map(|&n| {
            if n <= 0.0 {
                ' '
            } else {
                let i = ((n * SPARK.len() as f64) / peak).ceil() as usize;
                SPARK[i.clamp(1, SPARK.len()) - 1]
            }
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

pub fn label(t: &Theme, s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(t.label))
}

pub fn dim(t: &Theme, s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(t.dim))
}

pub fn text(t: &Theme, s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(t.text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_and_sparklines() {
        assert_eq!(sparkline(&[0, 0, 0, 10], 0, 10, 5), "█   ▃");
        assert_eq!(sparkline(&[], 0, 0, 3), "   ");
        assert_eq!(sparkline(&[5], 0, 10, 0), "");
        assert_eq!(buckets(&[0, 0, 10], 0, 10, 2), [2.0, 1.0]);
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
    fn status_marks_use_the_theme() {
        let t = Theme::default();
        assert_eq!(status_mark(&t, JobStatus::Running, "/").content, "[/]");
        assert_eq!(
            status_mark(&t, JobStatus::Running, "/").style.fg,
            Some(t.lead)
        );
        assert_eq!(status_mark(&t, JobStatus::Done, "/").style.fg, Some(t.ok));
        assert_eq!(
            status_mark(&t, JobStatus::Failed, "/").style.fg,
            Some(t.error)
        );
    }
}
