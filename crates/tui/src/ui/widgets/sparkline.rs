//! btop-style sparklines: block columns or a filled braille area.

use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

use super::{BrailleGrid, clip, put_char};
use crate::theme::gradient;

/// Block glyphs from empty to full, in eighths.
const BARS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// How a [`Sparkline`] draws its values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SparkStyle {
    /// One value per column, ` ▁▂▃▄▅▆▇█`, stacked over several rows.
    #[default]
    Block,
    /// Two values per column, a filled braille area (four dot rows per line).
    Braille,
}

/// A value usable for drawing: negative and non-finite values become 0.
fn clean(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// The last `n` values of `data`.
fn tail(data: &[f64], n: usize) -> &[f64] {
    data.get(data.len().saturating_sub(n)..).unwrap_or(&[])
}

/// The top of the scale: `max` when usable, else the largest value, else 1.
fn scale_max(values: &[f64], max: Option<f64>) -> f64 {
    let top = max
        .map(clean)
        .filter(|m| *m > 0.0)
        .unwrap_or_else(|| values.iter().copied().map(clean).fold(0.0, f64::max));
    if top > 0.0 { top } else { 1.0 }
}

/// `v` as a fraction of `max`, in 0..=1.
fn fraction(v: f64, max: f64) -> f64 {
    (clean(v) / max).clamp(0.0, 1.0)
}

/// How many of `steps` units `v` fills; any positive value gets at least one.
fn steps_for(v: f64, max: f64, steps: usize) -> usize {
    let frac = fraction(v, max);
    let n = (frac * steps as f64).round() as usize;
    if frac > 0.0 { n.max(1) } else { 0 }
}

/// A one-line block sparkline of the last `width` values, newest on the
/// right; shorter data is padded with spaces on the left.
pub fn spark_string(data: &[f64], width: usize) -> String {
    let values = tail(data, width);
    let max = scale_max(values, None);
    let pad = width.saturating_sub(values.len());
    let bars = values
        .iter()
        .map(|v| BARS.get(steps_for(*v, max, 8)).copied().unwrap_or(' '));
    std::iter::repeat_n(' ', pad).chain(bars).collect()
}

/// A sparkline with the newest value at the right edge. Each column is
/// coloured `gradient(stops, height fraction)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sparkline<'a> {
    data: &'a [f64],
    style: SparkStyle,
    max: Option<f64>,
    stops: &'a [Color],
    baseline: bool,
}

impl<'a> Sparkline<'a> {
    /// A block sparkline of `data` (oldest first) coloured along `stops`.
    pub fn new(data: &'a [f64], stops: &'a [Color]) -> Sparkline<'a> {
        Sparkline {
            data,
            style: SparkStyle::Block,
            max: None,
            stops,
            baseline: false,
        }
    }

    /// Block or braille drawing.
    pub fn style(mut self, style: SparkStyle) -> Sparkline<'a> {
        self.style = style;
        self
    }

    /// A fixed top of the scale (default: the largest visible value).
    pub fn max(mut self, max: f64) -> Sparkline<'a> {
        self.max = Some(max);
        self
    }

    /// Draws a floor (`▁` or the bottom dot) under zero values.
    pub fn baseline(mut self, baseline: bool) -> Sparkline<'a> {
        self.baseline = baseline;
        self
    }

    fn render_blocks(&self, area: Rect, buf: &mut Buffer) {
        let values = tail(self.data, usize::from(area.width));
        let max = scale_max(values, self.max);
        let offset = usize::from(area.width) - values.len();
        let rows = usize::from(area.height);
        for (i, v) in values.iter().enumerate() {
            let x = area.x + u16::try_from(offset + i).unwrap_or(u16::MAX);
            let total = steps_for(*v, max, rows * 8);
            let color = gradient(self.stops, fraction(*v, max));
            for r in 0..rows {
                let y = area.bottom() - 1 - u16::try_from(r).unwrap_or(0);
                match total.saturating_sub(r * 8).min(8) {
                    0 if r == 0 && self.baseline => {
                        put_char(buf, area, x, y, BARS[1], gradient(self.stops, 0.0));
                    }
                    0 => {}
                    n => put_char(buf, area, x, y, BARS.get(n).copied().unwrap_or('█'), color),
                }
            }
        }
    }

    fn render_braille(&self, area: Rect, buf: &mut Buffer) {
        let mut grid = BrailleGrid::new(area.width, area.height);
        let values = tail(self.data, usize::from(area.width) * 2);
        let max = scale_max(values, self.max);
        let offset = grid.dot_width() - i32::try_from(values.len()).unwrap_or(0);
        let dot_height = grid.dot_height();
        let mut column_frac = vec![0.0_f64; usize::from(area.width)];
        for (i, v) in values.iter().enumerate() {
            let x = offset + i32::try_from(i).unwrap_or(0);
            let mut n =
                i32::try_from(steps_for(*v, max, 4 * usize::from(area.height))).unwrap_or(0);
            if n == 0 && self.baseline {
                n = 1;
            }
            for y in (dot_height - n)..dot_height {
                grid.set(x, y);
            }
            if let Some(f) = usize::try_from(x / 2)
                .ok()
                .and_then(|cx| column_frac.get_mut(cx))
            {
                *f = f.max(fraction(*v, max));
            }
        }
        grid.render_with(area, buf, |cx, _| {
            gradient(
                self.stops,
                column_frac.get(usize::from(cx)).copied().unwrap_or(0.0),
            )
        });
    }
}

impl Widget for Sparkline<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() {
            return;
        }
        match self.style {
            SparkStyle::Block => self.render_blocks(area, buf),
            SparkStyle::Braille => self.render_braille(area, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const LOW: Color = Color::Rgb(0, 0, 0);
    const HIGH: Color = Color::Rgb(200, 200, 200);
    const STOPS: [Color; 2] = [LOW, HIGH];

    #[test]
    fn spark_string_is_right_aligned_and_scaled() {
        assert_eq!(spark_string(&[0.0, 2.0, 4.0, 8.0], 4), " ▂▄█");
        assert_eq!(spark_string(&[1.0, 1.0], 4), "  ██");
        assert_eq!(spark_string(&[9.0, 0.0, 8.0], 2), " █");
        assert_eq!(spark_string(&[], 3), "   ");
        assert_eq!(spark_string(&[f64::NAN, -4.0, 0.01, 100.0], 4), "  ▁█");
        assert_eq!(spark_string(&[1.0], 0), "");
    }

    #[test]
    fn block_columns_and_colours() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        Sparkline::new(&[0.0, 2.0, 4.0, 8.0], &STOPS).render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), " ▂▄█");
        assert_eq!(buf[(3, 0)].fg, HIGH);
        assert_eq!(buf[(2, 0)].fg, gradient(&STOPS, 0.5));
    }

    #[test]
    fn multi_row_blocks_fill_from_the_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        Sparkline::new(&[8.0, 4.0, 0.0], &[Color::Green])
            .baseline(true)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "█  ");
        assert_eq!(row(&buf, 1), "██▁");
        assert_eq!(buf[(1, 1)].fg, Color::Green);
    }

    #[test]
    fn fixed_max_and_newest_on_the_right() {
        let data: Vec<f64> = (0..10).map(f64::from).collect();
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        Sparkline::new(&data, &STOPS)
            .max(18.0)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "▄▄");
    }

    #[test]
    fn braille_fills_an_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        Sparkline::new(&[4.0, 8.0], &STOPS)
            .style(SparkStyle::Braille)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), " ⣼");
        assert_eq!(buf[(1, 0)].fg, HIGH);
        let mut base = Buffer::empty(Rect::new(0, 0, 1, 1));
        Sparkline::new(&[0.0, 0.0], &STOPS)
            .style(SparkStyle::Braille)
            .baseline(true)
            .render(base.area, &mut base);
        assert_eq!(row(&base, 0), "⣀");
        assert_eq!(base[(0, 0)].fg, LOW);
    }

    #[test]
    fn size_fuzz_never_panics() {
        let odd = [
            f64::NAN,
            -3.0,
            0.0,
            f64::INFINITY,
            1e300,
            2.5,
            f64::NEG_INFINITY,
            7.0,
        ];
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for data in [&odd[..], &[], &[0.0]] {
                    for style in [SparkStyle::Block, SparkStyle::Braille] {
                        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
                        let spark = Sparkline::new(data, &STOPS).style(style).baseline(true);
                        spark.render(Rect::new(w % 5, h % 4, w, h), &mut buf);
                        spark.max(f64::NAN).render(Rect::new(0, 0, w, h), &mut buf);
                    }
                    let _ = spark_string(data, usize::from(w));
                }
            }
        }
    }
}
