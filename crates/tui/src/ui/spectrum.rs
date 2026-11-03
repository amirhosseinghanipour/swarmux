//! The Spectrum: one cluster of bars per agent, cava style. Prompt tokens
//! per minute rise above the centre line, output tokens fall below it, and
//! a cap holds each bar's recent peak. Colours follow the theme gradient.

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

use super::showcase_chrome::{self, Chrome};
use crate::app::App;
use crate::showcase::{Agent, BUCKETS, Phase, Showcase};
use crate::theme::{Theme, mix};

/// Lower blocks, one eighth to seven eighths.
const LOWER: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];
/// Columns a bar takes, and the gap after it.
const BAR_W: u16 = 2;
const BAR_GAP: u16 = 1;
/// Columns between two agents' clusters.
const CLUSTER_GAP: u16 = 3;
/// Narrowest cluster worth drawing.
const MIN_CLUSTER: u16 = 6;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let chrome = Chrome::new(app, "spectrum");
    let (head, body, foot) = showcase_chrome::split(area);
    showcase_chrome::header(frame, head, app, &chrome);
    bars(frame.buffer_mut(), body, t, &app.showcase);
    showcase_chrome::dim_when_frozen(frame.buffer_mut(), body, t, &app.showcase);
    showcase_chrome::footer(frame, foot, app, &chrome);
}

/// How many agents fit side by side in `width` columns, and how wide each
/// cluster is.
pub fn clusters(width: u16, agents: usize) -> (usize, u16) {
    let usable = width.saturating_sub(4);
    let mut n = agents;
    while n > 0 {
        let gaps = CLUSTER_GAP * (n as u16 - 1);
        let each = usable.saturating_sub(gaps) / n as u16;
        if each >= MIN_CLUSTER {
            return (n, each);
        }
        n -= 1;
    }
    (0, 0)
}

fn bars(buf: &mut Buffer, area: Rect, t: &Theme, show: &Showcase) {
    let area = area.intersection(buf.area);
    if area.width < 12 || area.height < 7 {
        return;
    }
    let rows = (area.height - 3) / 2;
    let mid = area.y + 1 + rows;
    let rule = Style::new().fg(mix(t.bg, t.dim, 0.8));
    for x in area.left()..area.right() {
        buf.set_string(x, mid, "┄", rule);
    }
    buf.set_string(area.x, area.y, "▲ prompt", Style::new().fg(t.dim));
    buf.set_string(
        area.x,
        area.bottom() - 2,
        "▼ output",
        Style::new().fg(t.dim),
    );
    let (fit, each) = clusters(area.width, show.agents.len());
    if fit == 0 {
        buf.set_string(area.x + 2, mid, "no agents yet", Style::new().fg(t.muted));
        return;
    }
    let used = each * fit as u16 + CLUSTER_GAP * (fit as u16 - 1);
    let left = area.x + area.width.saturating_sub(used) / 2;
    for (i, agent) in show.agents.iter().take(fit).enumerate() {
        let x0 = left + i as u16 * (each + CLUSTER_GAP);
        let room = Rect::new(x0, area.y + 1, each, area.height - 1);
        cluster(buf, t, agent, room, rows, mid);
    }
}

fn cluster(buf: &mut Buffer, t: &Theme, agent: &Agent, area: Rect, rows: u16, mid: u16) {
    let per = BAR_W + BAR_GAP;
    let slots = usize::from((area.width + BAR_GAP) / per).min(BUCKETS);
    let take = |series: &[f64]| -> Vec<f64> {
        let from = series.len().saturating_sub(slots);
        let mut v = series.get(from..).unwrap_or(&[]).to_vec();
        // Missing history sits at the left as silence.
        while v.len() < slots {
            v.insert(0, 0.0);
        }
        v
    };
    let (up, down) = (take(&agent.up), take(&agent.down));
    let (peak_up, peak_down) = (take(&agent.peak_up), take(&agent.peak_down));
    let used = slots as u16 * per - BAR_GAP;
    let x0 = area.x + area.width.saturating_sub(used) / 2;
    let eighths = |v: f64| (v.clamp(0.0, 1.0) * f64::from(rows) * 8.0).round() as i32;
    for j in 0..slots {
        let x = x0 + j as u16 * per;
        let (hu, hd) = (eighths(up[j]), eighths(down[j]));
        for r in 0..rows {
            let frac = (f64::from(r) + 0.5) / f64::from(rows);
            let color = t.chart_at(frac);
            let below = mix(t.bg, color, 0.7);
            let fill_up = (hu - i32::from(r) * 8).clamp(0, 8);
            let fill_down = (hd - i32::from(r) * 8).clamp(0, 8);
            for dx in 0..BAR_W {
                put_up(buf, x + dx, mid - 1 - r, fill_up, color);
                put_down(buf, x + dx, mid + 1 + r, fill_down, below, t.bg);
            }
        }
        let caps = [
            (eighths(peak_up[j]), hu, true),
            (eighths(peak_down[j]), hd, false),
        ];
        for (peak, bar, upward) in caps {
            cap(
                buf,
                Cap {
                    x,
                    mid,
                    rows,
                    peak,
                    bar,
                    upward,
                },
                t,
            );
        }
    }
    // The agent's name under its cluster, in its hue.
    let name = super::dashboard::fit(&agent.name, usize::from(area.width));
    let tone = match agent.phase {
        Phase::Working => t.chart_at(agent.hue),
        Phase::Waiting => t.warn,
        _ => t.muted,
    };
    let at = area.x + area.width.saturating_sub(name.chars().count() as u16) / 2;
    buf.set_string(at, area.bottom() - 1, name, Style::new().fg(tone));
}

fn put_up(buf: &mut Buffer, x: u16, y: u16, eighths: i32, color: Color) {
    let glyph = match eighths {
        i if i <= 0 => return,
        8.. => '█',
        i => LOWER[(i - 1) as usize],
    };
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(glyph).set_fg(color);
    }
}

/// A bar growing downward: a partial cell is a lower block with the colours
/// swapped, so the fill hangs from the top of the cell.
fn put_down(buf: &mut Buffer, x: u16, y: u16, eighths: i32, color: Color, bg: Color) {
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    match eighths {
        i if i <= 0 => {}
        8.. => {
            cell.set_char('█').set_fg(color);
        }
        i => {
            cell.set_char(LOWER[(7 - i) as usize])
                .set_fg(bg)
                .set_bg(color);
        }
    }
}

/// Where a peak cap goes.
struct Cap {
    x: u16,
    mid: u16,
    rows: u16,
    /// Peak and bar heights, in eighths of a cell.
    peak: i32,
    bar: i32,
    upward: bool,
}

/// The peak cap, drawn only where it clears the bar.
fn cap(buf: &mut Buffer, c: Cap, t: &Theme) {
    if c.peak <= 0 || c.peak < c.bar + 4 {
        return;
    }
    let row = ((c.peak - 1) / 8).min(i32::from(c.rows) - 1) as u16;
    let (y, glyph) = if c.upward {
        (c.mid - 1 - row, '▔')
    } else {
        (c.mid + 1 + row, '▁')
    };
    for dx in 0..BAR_W {
        if let Some(cell) = buf.cell_mut((c.x + dx, y)) {
            cell.set_char(glyph).set_fg(t.title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::TileState;
    use crate::showcase::fixtures::{input, refresh, tile};

    #[test]
    fn clusters_shrink_in_count_before_they_get_too_narrow() {
        assert_eq!(clusters(160, 2).0, 2);
        let (n, each) = clusters(160, 8);
        assert_eq!(n, 8);
        assert!(each >= MIN_CLUSTER);
        let (few, _) = clusters(40, 12);
        assert!(few < 12 && few > 0);
        assert_eq!(clusters(5, 3), (0, 0));
        assert_eq!(clusters(160, 0), (0, 0));
    }

    #[test]
    fn input_rises_and_output_hangs_under_the_line() {
        let mut show = Showcase::default();
        show.refresh(refresh(
            vec![input(tile(0, "api", TileState::Running, 0.5, 6_000.0), &[])],
            100,
        ));
        let t = crate::theme::REFERENCE;
        let area = Rect::new(0, 0, 60, 21);
        let mut buf = Buffer::empty(area);
        bars(&mut buf, area, &t, &show);
        let mid = 1 + (21 - 3) / 2;
        let line = |buf: &Buffer, y: u16| -> String {
            (0..60).map(|x| buf[(x, y)].symbol().to_string()).collect()
        };
        assert!(line(&buf, mid).contains('┄'));
        assert!(
            (1..mid).any(|y| line(&buf, y).contains('█')),
            "prompt tokens above the line"
        );
        assert!(
            (mid + 1..19).any(|y| line(&buf, y).chars().any(|c| "█▁▂▃▄▅▆▇".contains(c))),
            "output below the line"
        );
        assert!(line(&buf, 20).contains("api"), "named under its cluster");
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let show = Showcase::default();
        let t = crate::theme::REFERENCE;
        for w in 0..30u16 {
            for h in 0..12u16 {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                bars(&mut buf, area, &t, &show);
            }
        }
    }
}
