//! Gradient bars and gauges with eighth-of-a-cell precision.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::Widget,
};

use super::{clip, put_char, put_str, unit};
use crate::theme::gradient;

/// Partial-cell glyphs, from one eighth to a full cell.
const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
/// An empty bar cell.
const TRACK: char = '─';
/// The caps around a gauge's bar.
const LEFT_CAP: char = '▕';
const RIGHT_CAP: char = '▏';

/// The glyph for a cell that is `n` eighths (1..=8) full.
fn eighth(n: usize) -> char {
    EIGHTHS.get(n.saturating_sub(1)).copied().unwrap_or('█')
}

/// Where cell `x` of `width` sits along the gradient: 0 at the first cell,
/// 1 at the last.
fn position(x: usize, width: usize) -> f64 {
    if width <= 1 {
        0.0
    } else {
        x as f64 / (width - 1) as f64
    }
}

/// One `(glyph, colour)` per cell of a `width`-cell bar filled to `ratio`.
fn bar_cells(
    ratio: f64,
    width: usize,
    stops: &[Color],
    track: Color,
) -> impl Iterator<Item = (char, Color)> + '_ {
    let filled = (unit(ratio) * width as f64 * 8.0).round() as usize;
    (0..width).map(move |x| match filled.saturating_sub(x * 8).min(8) {
        0 => (TRACK, track),
        n => (eighth(n), gradient(stops, position(x, width))),
    })
}

/// A one-line bar as spans, one per cell: filled cells take the gradient
/// colour at their position, the partial cell an eighth glyph, empty cells
/// `─` in `track`. `ratio` is clamped to 0..=1 (NaN → 0).
pub fn bar_spans(ratio: f64, width: usize, stops: &[Color], track: Color) -> Vec<Span<'static>> {
    bar_cells(ratio, width, stops, track)
        .map(|(ch, color)| Span::styled(ch.to_string(), Style::default().fg(color)))
        .collect()
}

/// Draws a bar of `width` cells at `(x, y)`, clipped to `clip`.
fn draw_bar(buf: &mut Buffer, clip: Rect, (x, y): (u16, u16), width: u16, bar: &GradientBar) {
    for (i, (ch, color)) in
        bar_cells(bar.ratio, usize::from(width), bar.stops, bar.track).enumerate()
    {
        let cx = x.saturating_add(u16::try_from(i).unwrap_or(u16::MAX));
        put_char(buf, clip, cx, y, ch, color);
    }
}

/// Draws `▕bar▏` when there is room for the caps, else the bare bar.
fn draw_capped_bar(
    buf: &mut Buffer,
    clip: Rect,
    (x, y): (u16, u16),
    width: u16,
    bar: &GradientBar,
) {
    if width >= 3 {
        put_char(buf, clip, x, y, LEFT_CAP, bar.track);
        draw_bar(buf, clip, (x.saturating_add(1), y), width - 2, bar);
        put_char(
            buf,
            clip,
            x.saturating_add(width - 1),
            y,
            RIGHT_CAP,
            bar.track,
        );
    } else {
        draw_bar(buf, clip, (x, y), width, bar);
    }
}

/// The display width of `text` in cells.
fn text_width(text: &str) -> u16 {
    u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
}

/// A one-row gradient bar filling the first row of its area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientBar<'a> {
    ratio: f64,
    stops: &'a [Color],
    track: Color,
}

impl<'a> GradientBar<'a> {
    /// A bar filled to `ratio` (0..=1) along the gradient `stops`.
    pub fn new(ratio: f64, stops: &'a [Color]) -> GradientBar<'a> {
        GradientBar {
            ratio,
            stops,
            track: Color::DarkGray,
        }
    }

    /// The colour of the empty part.
    pub fn track(mut self, track: Color) -> GradientBar<'a> {
        self.track = track;
        self
    }
}

impl Widget for GradientBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let clip = clip(area, buf);
        if clip.is_empty() {
            return;
        }
        draw_bar(buf, clip, (clip.x, clip.y), clip.width, &self);
    }
}

/// `label ▕████████▌───────▏ 73%` on one row; with two or more rows the
/// label and value sit on the first row and the bar on the second.
///
/// The value text is coloured with the gradient at `ratio`. When the row is
/// too narrow the label is dropped first, then the value.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientGauge<'a> {
    ratio: f64,
    label: Option<&'a str>,
    value_text: Option<String>,
    stops: &'a [Color],
    track: Color,
    label_color: Color,
}

impl<'a> GradientGauge<'a> {
    /// A gauge filled to `ratio` (0..=1, NaN → 0) along `stops`.
    pub fn new(ratio: f64, stops: &'a [Color]) -> GradientGauge<'a> {
        GradientGauge {
            ratio: unit(ratio),
            label: None,
            value_text: None,
            stops,
            track: Color::DarkGray,
            label_color: Color::Reset,
        }
    }

    /// Text on the left.
    pub fn label(mut self, label: &'a str) -> GradientGauge<'a> {
        self.label = Some(label);
        self
    }

    /// Text on the right, e.g. `"73%"`.
    pub fn value(mut self, value: impl Into<String>) -> GradientGauge<'a> {
        self.value_text = Some(value.into());
        self
    }

    /// The colour of the empty part and the caps.
    pub fn track(mut self, track: Color) -> GradientGauge<'a> {
        self.track = track;
        self
    }

    /// The colour of the label.
    pub fn label_color(mut self, color: Color) -> GradientGauge<'a> {
        self.label_color = color;
        self
    }

    fn bar(&self) -> GradientBar<'a> {
        GradientBar::new(self.ratio, self.stops).track(self.track)
    }

    fn value_style(&self) -> Style {
        Style::default().fg(gradient(self.stops, self.ratio))
    }

    /// Label and value on the first row, the bar on the second.
    fn render_stacked(&self, clip: Rect, buf: &mut Buffer) {
        let label_style = Style::default().fg(self.label_color);
        let after_label = match self.label {
            Some(label) => put_str(buf, clip, clip.x, clip.y, label, label_style),
            None => clip.x,
        };
        if let Some(value) = self.value_text.as_deref() {
            let x = clip.right().saturating_sub(text_width(value));
            let gap = u16::from(after_label > clip.x);
            if x >= after_label.saturating_add(gap) {
                put_str(buf, clip, x, clip.y, value, self.value_style());
            }
        }
        draw_capped_bar(buf, clip, (clip.x, clip.y + 1), clip.width, &self.bar());
    }

    /// Everything on one row.
    fn render_inline(&self, clip: Rect, buf: &mut Buffer) {
        const MIN_BAR: u16 = 3;
        let need = |text: Option<&str>| text.map_or(0, |t| text_width(t).saturating_add(1));
        let value_need = need(self.value_text.as_deref());
        let show_value = value_need > 0 && clip.width >= MIN_BAR.saturating_add(value_need);
        let value_need = if show_value { value_need } else { 0 };
        let label_need = need(self.label);
        let show_label = label_need > 0
            && clip.width
                >= MIN_BAR
                    .saturating_add(value_need)
                    .saturating_add(label_need);
        let mut x = clip.x;
        if let (true, Some(label)) = (show_label, self.label) {
            put_str(
                buf,
                clip,
                x,
                clip.y,
                label,
                Style::default().fg(self.label_color),
            );
            x = x.saturating_add(label_need);
        }
        let bar_width = clip.right().saturating_sub(x).saturating_sub(value_need);
        draw_capped_bar(buf, clip, (x, clip.y), bar_width, &self.bar());
        if let (true, Some(value)) = (show_value, self.value_text.as_deref()) {
            let vx = x.saturating_add(bar_width).saturating_add(1);
            put_str(buf, clip, vx, clip.y, value, self.value_style());
        }
    }
}

impl Widget for GradientGauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let clip = clip(area, buf);
        if clip.is_empty() {
            return;
        }
        if clip.height >= 2 {
            self.render_stacked(clip, buf);
        } else {
            self.render_inline(clip, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const RED: Color = Color::Rgb(200, 0, 0);
    const BLUE: Color = Color::Rgb(0, 0, 200);
    const GREY: Color = Color::Rgb(50, 50, 50);
    const STOPS: [Color; 2] = [RED, BLUE];

    fn glyphs(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn spans_fill_with_gradient_and_eighths() {
        let spans = bar_spans(0.55, 4, &STOPS, GREY);
        assert_eq!(spans.len(), 4);
        assert_eq!(glyphs(&spans), "██▎─");
        assert_eq!(spans[0].style.fg, Some(RED));
        assert_eq!(spans[1].style.fg, Some(gradient(&STOPS, 1.0 / 3.0)));
        assert_eq!(spans[3].style.fg, Some(GREY));
        let full = bar_spans(1.0, 3, &STOPS, GREY);
        assert_eq!(glyphs(&full), "███");
        assert_eq!(full[2].style.fg, Some(BLUE));
    }

    #[test]
    fn ratios_are_clamped() {
        assert_eq!(glyphs(&bar_spans(f64::NAN, 3, &STOPS, GREY)), "───");
        assert_eq!(glyphs(&bar_spans(-2.0, 2, &STOPS, GREY)), "──");
        assert_eq!(glyphs(&bar_spans(9.0, 2, &STOPS, GREY)), "██");
        assert_eq!(glyphs(&bar_spans(f64::INFINITY, 2, &[], GREY)), "──");
        assert!(bar_spans(0.5, 0, &STOPS, GREY).is_empty());
    }

    #[test]
    fn bar_widget_fills_the_first_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        GradientBar::new(0.5, &STOPS)
            .track(GREY)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "██──");
        assert_eq!(row(&buf, 1), "    ");
        assert_eq!(buf[(0, 0)].fg, RED);
        assert_eq!(buf[(3, 0)].fg, GREY);
    }

    #[test]
    fn inline_gauge_lays_out_label_bar_value() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 16, 1));
        GradientGauge::new(0.5, &STOPS)
            .label("cpu")
            .value("50%")
            .track(GREY)
            .label_color(Color::White)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "cpu ▕███───▏ 50%");
        assert_eq!(buf[(0, 0)].fg, Color::White);
        assert_eq!(buf[(4, 0)].fg, GREY);
        assert_eq!(buf[(5, 0)].fg, RED);
        assert_eq!(buf[(13, 0)].fg, gradient(&STOPS, 0.5));
    }

    #[test]
    fn narrow_gauge_drops_label_then_value() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        GradientGauge::new(1.0, &STOPS)
            .label("memory")
            .value("100%")
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "▕█▏ 100%");
        let mut tiny = Buffer::empty(Rect::new(0, 0, 4, 1));
        GradientGauge::new(1.0, &STOPS)
            .label("memory")
            .value("100%")
            .render(tiny.area, &mut tiny);
        assert_eq!(row(&tiny, 0), "▕██▏");
    }

    #[test]
    fn stacked_gauge_uses_two_rows() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        GradientGauge::new(f64::NAN, &STOPS)
            .label("io")
            .value("0%")
            .track(GREY)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "io      0%");
        assert_eq!(row(&buf, 1), "▕────────▏");
        assert_eq!(buf[(8, 0)].fg, RED);
    }

    #[test]
    fn size_fuzz_never_panics() {
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for ratio in [f64::NAN, -1.0, 0.0, 0.37, 1.0, f64::INFINITY] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 24, 6));
                    let area = Rect::new(w % 7, h % 3, w, h);
                    GradientBar::new(ratio, &[]).render(area, &mut buf);
                    GradientGauge::new(ratio, &STOPS)
                        .label("a longer label ✓")
                        .value("100.0%")
                        .render(area, &mut buf);
                    let _ = bar_spans(ratio, usize::from(w), &STOPS, GREY);
                }
            }
        }
    }
}
