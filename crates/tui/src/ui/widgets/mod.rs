//! Small, self-contained widgets for the "riced terminal" look: braille
//! graphs, gradient gauges, rings, heatmaps and a few tick-driven helpers.
//!
//! Widgets take plain [`Color`]s (or gradient stops) rather than a theme,
//! never panic, and clip to the buffer they draw into.

// A library: a few builders and re-exports are not used by any panel yet.
#![allow(dead_code, unused_imports)]

pub mod braille;
pub mod gauge;
pub mod heatmap;
pub mod line_chart;
pub mod pulse;
pub mod ring;
pub mod sparkline;
pub mod spinner;
pub mod table;
pub mod ticker;
pub mod tween;

pub use braille::BrailleGrid;
pub use gauge::{GradientBar, GradientGauge, bar_spans};
pub use heatmap::Heatmap;
pub use line_chart::{LineChart, Series, compact};
pub use pulse::{Pulse, pulse_color};
pub use ring::Ring;
pub use sparkline::{SparkStyle, Sparkline, spark_string};
pub use spinner::SpinnerSet;
pub use table::MiniTable;
pub use ticker::{Ticker, ticker_window};
pub use tween::{Easing, Tween};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

/// The part of `area` that lies inside `buf`.
fn clip(area: Rect, buf: &Buffer) -> Rect {
    area.intersection(buf.area)
}

/// Whether the cell `(x, y)` lies inside `rect`.
fn inside(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.left() && x < rect.right() && y >= rect.top() && y < rect.bottom()
}

/// Writes `text` at `(x, y)` with `style`, cut at the right edge of `clip`.
/// Returns the column after the last cell written (`x` if nothing was).
fn put_str(buf: &mut Buffer, clip: Rect, x: u16, y: u16, text: &str, style: Style) -> u16 {
    let clip = clip.intersection(buf.area);
    if !inside(clip, x, y) {
        return x;
    }
    let room = usize::from(clip.right() - x);
    buf.set_stringn(x, y, text, room, style).0
}

/// Writes one glyph at `(x, y)` in `fg` when the cell lies inside `clip`.
fn put_char(buf: &mut Buffer, clip: Rect, x: u16, y: u16, ch: char, fg: Color) {
    if !inside(clip, x, y) {
        return;
    }
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch).set_fg(fg);
    }
}

/// `ratio` clamped to `0..=1`; NaN and other non-finite values become 0.
fn unit(ratio: f64) -> f64 {
    if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Every symbol in row `y` of `buf`, concatenated (test helper).
#[cfg(test)]
fn row(buf: &Buffer, y: u16) -> String {
    (buf.area.left()..buf.area.right())
        .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
        .collect()
}
