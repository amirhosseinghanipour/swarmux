//! A multi-series braille line chart with an auto-scaled y axis.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::Widget,
};

use super::{BrailleGrid, clip, put_char, put_str};

/// Narrower charts drop the y axis.
const AXIS_MIN_WIDTH: u16 = 12;

/// One line of a [`LineChart`]: values oldest first, newest last.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Series<'a> {
    pub name: &'a str,
    pub points: &'a [f64],
    pub color: Color,
}

/// Braille lines for one or more series sharing a y scale.
///
/// The y range is `[min(0, data min), data max]` rounded out to a "nice"
/// number with a little headroom, unless fixed with [`LineChart::bounds`].
/// Series are right-aligned (their newest points share the right edge) and
/// the longest one spans the full width. Non-finite points break the line.
/// Where series overlap, the later one colours the cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineChart<'a> {
    series: &'a [Series<'a>],
    y_axis: bool,
    unit: &'a str,
    min: Option<f64>,
    max: Option<f64>,
    label_color: Color,
    axis_color: Color,
    legend: bool,
}

impl<'a> LineChart<'a> {
    /// A chart of `series` with the y axis and legend on.
    pub fn new(series: &'a [Series<'a>]) -> LineChart<'a> {
        LineChart {
            series,
            y_axis: true,
            unit: "",
            min: None,
            max: None,
            label_color: Color::Reset,
            axis_color: Color::Reset,
            legend: true,
        }
    }

    /// Shows the y-axis labels (only when the chart is at least 12 wide).
    pub fn y_axis(mut self, on: bool) -> LineChart<'a> {
        self.y_axis = on;
        self
    }

    /// A suffix for the axis labels, e.g. `"%"` or `"/s"`.
    pub fn unit(mut self, unit: &'a str) -> LineChart<'a> {
        self.unit = unit;
        self
    }

    /// Fixes the y range; a non-finite bound stays automatic.
    pub fn bounds(mut self, min: f64, max: f64) -> LineChart<'a> {
        self.min = min.is_finite().then_some(min);
        self.max = max.is_finite().then_some(max);
        self
    }

    /// The colour of the axis labels and legend names.
    pub fn labels(mut self, color: Color) -> LineChart<'a> {
        self.label_color = color;
        self
    }

    /// The colour of the axis line.
    pub fn axis(mut self, color: Color) -> LineChart<'a> {
        self.axis_color = color;
        self
    }

    /// Shows the series names on the top row, right-aligned.
    pub fn legend(mut self, on: bool) -> LineChart<'a> {
        self.legend = on;
        self
    }

    /// Every finite point of every series.
    fn finite_points(&self) -> impl Iterator<Item = f64> + '_ {
        self.series
            .iter()
            .flat_map(|s| s.points.iter().copied())
            .filter(|v| v.is_finite())
    }

    /// The y range `(lo, hi)`, always with `hi > lo`.
    fn range(&self) -> (f64, f64) {
        let data_lo = self.finite_points().fold(0.0, f64::min);
        let data_hi = self.finite_points().fold(0.0, f64::max);
        let lo = self.min.unwrap_or(-nice_ceil(-data_lo));
        let hi = self.max.unwrap_or(nice_ceil(data_hi));
        if hi > lo { (lo, hi) } else { (lo, lo + 1.0) }
    }

    /// The labels and axis line in the left `gutter` columns.
    fn draw_axis(&self, area: Rect, buf: &mut Buffer, label_width: u16, (lo, hi): (f64, f64)) {
        let rows = area.height;
        let label_style = Style::default().fg(self.label_color);
        for r in 0..rows {
            let y = area.y + r;
            let value = if r == 0 {
                Some(hi)
            } else if r == rows - 1 {
                Some(lo)
            } else if rows >= 5 && r == rows / 2 {
                Some(hi - (hi - lo) * f64::from(r) / f64::from(rows - 1))
            } else {
                None
            };
            if let Some(v) = value {
                let text = format!("{}{}", compact(v), self.unit);
                let x = area.x + label_width.saturating_sub(width_of(&text));
                put_str(buf, area, x, y, &text, label_style);
            }
            let glyph = if value.is_some() { '┤' } else { '│' };
            put_char(buf, area, area.x + label_width, y, glyph, self.axis_color);
        }
    }

    /// The braille lines inside `plot`.
    fn draw_lines(&self, plot: Rect, buf: &mut Buffer, (lo, hi): (f64, f64)) {
        let longest = self
            .series
            .iter()
            .map(|s| s.points.len())
            .max()
            .unwrap_or(0);
        if longest == 0 {
            return;
        }
        let grids: Vec<(BrailleGrid, Color)> = self
            .series
            .iter()
            .map(|s| (trace(s.points, longest, plot, (lo, hi)), s.color))
            .collect();
        let mut all = BrailleGrid::new(plot.width, plot.height);
        for (grid, _) in &grids {
            all.overlay(grid);
        }
        all.render_with(plot, buf, |cx, cy| {
            grids
                .iter()
                .rev()
                .find(|(grid, _)| grid.bits(cx, cy) != 0)
                .map_or(Color::Reset, |(_, color)| *color)
        });
    }

    /// `● name  ● name` right-aligned on the top row of `plot`, if it fits.
    fn draw_legend(&self, plot: Rect, buf: &mut Buffer) {
        let entries: Vec<&Series> = self.series.iter().filter(|s| !s.name.is_empty()).collect();
        let total: u16 = entries
            .iter()
            .map(|s| width_of(s.name).saturating_add(2))
            .fold(0u16, u16::saturating_add)
            .saturating_add(2 * u16::try_from(entries.len().saturating_sub(1)).unwrap_or(u16::MAX));
        if entries.is_empty() || total > plot.width {
            return;
        }
        let style = Style::default().fg(self.label_color);
        let mut x = plot.right() - total;
        for (i, s) in entries.iter().enumerate() {
            if i > 0 {
                x = put_str(buf, plot, x, plot.y, "  ", style);
            }
            put_char(buf, plot, x, plot.y, '●', s.color);
            x = put_str(
                buf,
                plot,
                x.saturating_add(1),
                plot.y,
                &format!(" {}", s.name),
                style,
            );
        }
    }
}

impl Widget for LineChart<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() {
            return;
        }
        let range = self.range();
        let label_width = [range.0, range.1]
            .iter()
            .map(|v| width_of(&compact(*v)).saturating_add(width_of(self.unit)))
            .max()
            .unwrap_or(0);
        let gutter = label_width.saturating_add(1);
        let axis = self.y_axis && area.width >= AXIS_MIN_WIDTH && gutter < area.width / 2;
        let plot = if axis {
            self.draw_axis(area, buf, label_width, range);
            Rect {
                x: area.x + gutter,
                width: area.width - gutter,
                ..area
            }
        } else {
            area
        };
        self.draw_lines(plot, buf, range);
        if self.legend {
            self.draw_legend(plot, buf);
        }
    }
}

/// One series traced onto a canvas the size of `plot`.
fn trace(points: &[f64], longest: usize, plot: Rect, (lo, hi): (f64, f64)) -> BrailleGrid {
    let mut grid = BrailleGrid::new(plot.width, plot.height);
    let (dot_w, dot_h) = (grid.dot_width(), grid.dot_height());
    let offset = longest.saturating_sub(points.len());
    let mut prev: Option<(i32, i32)> = None;
    for (i, v) in points.iter().enumerate() {
        if !v.is_finite() {
            prev = None;
            continue;
        }
        let x = dot_x(offset + i, longest, dot_w);
        let frac = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        let y = ((1.0 - frac) * f64::from(dot_h - 1)).round() as i32;
        match prev {
            Some((px, py)) => grid.line(px, py, x, y),
            None => grid.set(x, y),
        }
        prev = Some((x, y));
    }
    grid
}

/// The dot column of point `i` of `n` spread across `dot_w` columns; a lone
/// point sits at the right edge.
fn dot_x(i: usize, n: usize, dot_w: i32) -> i32 {
    let last = f64::from((dot_w - 1).max(0));
    if n <= 1 {
        return last as i32;
    }
    (i as f64 * last / (n - 1) as f64).round() as i32
}

/// The display width of `text` in cells.
fn width_of(text: &str) -> u16 {
    u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
}

/// The smallest "nice" number (1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8 × 10ⁿ) at
/// or above `x` plus 5 % headroom; 0 for zero, negative or non-finite `x`.
fn nice_ceil(x: f64) -> f64 {
    const STEPS: [f64; 11] = [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0];
    if !x.is_finite() || x <= 0.0 {
        return 0.0;
    }
    let target = x * 1.05;
    if !target.is_finite() {
        return x;
    }
    let magnitude = 10f64.powf(target.log10().floor());
    STEPS
        .iter()
        .map(|s| s * magnitude)
        .find(|v| *v >= target * (1.0 - 1e-9))
        .unwrap_or(target)
}

/// A short label for `v`: `1.2k`, `3.4M`, `5G`, `12`, `0.25`.
pub fn compact(v: f64) -> String {
    if !v.is_finite() {
        return "-".to_string();
    }
    let a = v.abs();
    let (scaled, suffix) = if a >= 1e9 {
        (v / 1e9, "G")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "k")
    } else {
        (v, "")
    };
    let digits = match scaled.abs() {
        s if s >= 10.0 => 0,
        _ if suffix.is_empty() && a < 1.0 => 2,
        _ => 1,
    };
    let mut text = format!("{scaled:.digits$}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" {
        text = "0".to_string();
    }
    text + suffix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const ORANGE: Color = Color::Rgb(232, 140, 70);
    const BLUE: Color = Color::Rgb(110, 160, 235);
    const LABEL: Color = Color::Rgb(150, 140, 180);
    const AXIS: Color = Color::Rgb(75, 72, 90);

    fn is_braille(symbol: &str) -> bool {
        symbol
            .chars()
            .next()
            .is_some_and(|c| ('⠁'..='⣿').contains(&c))
    }

    #[test]
    fn compact_labels() {
        assert_eq!(compact(0.0), "0");
        assert_eq!(compact(0.25), "0.25");
        assert_eq!(compact(3.0), "3");
        assert_eq!(compact(2.5), "2.5");
        assert_eq!(compact(120.0), "120");
        assert_eq!(compact(1234.0), "1.2k");
        assert_eq!(compact(12_345.0), "12k");
        assert_eq!(compact(3_400_000.0), "3.4M");
        assert_eq!(compact(5e9), "5G");
        assert_eq!(compact(-1500.0), "-1.5k");
        assert_eq!(compact(-0.001), "0");
        assert_eq!(compact(f64::NAN), "-");
    }

    #[test]
    fn nice_maxima_have_headroom() {
        assert_eq!(nice_ceil(10.0), 12.0);
        assert_eq!(nice_ceil(100.0), 120.0);
        assert_eq!(nice_ceil(0.9), 1.0);
        assert_eq!(nice_ceil(73.0), 80.0);
        assert_eq!(nice_ceil(0.0), 0.0);
        assert_eq!(nice_ceil(f64::MAX), f64::MAX);
    }

    #[test]
    fn axis_labels_and_line_colours() {
        let series = [Series {
            name: "",
            points: &[0.0, 10.0],
            color: ORANGE,
        }];
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        LineChart::new(&series)
            .labels(LABEL)
            .axis(AXIS)
            .render(buf.area, &mut buf);
        assert!(row(&buf, 0).starts_with("12┤"), "{:?}", row(&buf, 0));
        assert!(row(&buf, 1).starts_with("  │"));
        assert!(row(&buf, 2).starts_with(" 6┤"));
        assert!(row(&buf, 4).starts_with(" 0┤"));
        assert_eq!(buf[(1, 0)].fg, LABEL);
        assert_eq!(buf[(2, 0)].fg, AXIS);
        // The first point (0) sits in the bottom-left plot cell.
        assert!(is_braille(buf[(3, 4)].symbol()));
        assert_eq!(buf[(3, 4)].fg, ORANGE);
        // The last point (10 of 12) is near the top-right.
        assert!(is_braille(buf[(19, 0)].symbol()));
        assert_eq!(buf[(19, 0)].fg, ORANGE);
    }

    #[test]
    fn later_series_win_and_legend_is_right_aligned() {
        let flat = [2.0; 8];
        let series = [
            Series {
                name: "a",
                points: &flat,
                color: ORANGE,
            },
            Series {
                name: "b",
                points: &flat,
                color: BLUE,
            },
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        LineChart::new(&series)
            .labels(LABEL)
            .bounds(0.0, 10.0)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "  ● a  ● b");
        assert_eq!(buf[(2, 0)].fg, ORANGE);
        assert_eq!(buf[(7, 0)].fg, BLUE);
        assert_eq!(buf[(9, 0)].fg, LABEL);
        // Narrow: no axis; the flat line is drawn in the later colour.
        let y = (0..3)
            .find(|y| is_braille(buf[(0, *y)].symbol()))
            .expect("a line is drawn");
        assert_eq!(buf[(0, y)].fg, BLUE);
    }

    #[test]
    fn fixed_bounds_unit_and_empty_series() {
        let series = [
            Series {
                name: "none",
                points: &[],
                color: ORANGE,
            },
            Series {
                name: "gaps",
                points: &[f64::NAN, 50.0, f64::INFINITY],
                color: BLUE,
            },
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 16, 4));
        LineChart::new(&series)
            .bounds(0.0, 100.0)
            .unit("%")
            .legend(false)
            .render(buf.area, &mut buf);
        assert!(row(&buf, 0).starts_with("100%┤"));
        assert!(row(&buf, 3).starts_with("  0%┤"));
        let dots: Vec<(u16, u16)> = (0..16)
            .flat_map(|x| (0..4).map(move |y| (x, y)))
            .filter(|p| is_braille(buf[*p].symbol()))
            .collect();
        assert_eq!(dots.len(), 1, "a lone point: {dots:?}");
        assert_eq!(buf[dots[0]].fg, BLUE);
    }

    #[test]
    fn nothing_to_plot_draws_only_the_axis() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 2));
        LineChart::new(&[]).render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "1┤            ");
        assert_eq!(row(&buf, 1), "0┤            ");
    }

    #[test]
    fn size_fuzz_never_panics() {
        let odd = [f64::NAN, -3.0, 0.0, f64::INFINITY, 1e300, -1e300, 2.5, 7.0];
        let series = [
            Series {
                name: "odd",
                points: &odd,
                color: ORANGE,
            },
            Series {
                name: "one",
                points: &[4.0],
                color: BLUE,
            },
            Series {
                name: "",
                points: &[],
                color: BLUE,
            },
        ];
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
                LineChart::new(&series)
                    .unit("ms")
                    .render(Rect::new(w % 9, h % 5, w, h), &mut buf);
                LineChart::new(&series[1..])
                    .bounds(5.0, 5.0)
                    .render(Rect::new(0, 0, w, h), &mut buf);
                LineChart::new(&[]).render(Rect::new(0, 0, w, h), &mut buf);
            }
        }
    }
}
