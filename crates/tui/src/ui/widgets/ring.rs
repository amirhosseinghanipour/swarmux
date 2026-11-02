//! A braille donut showing shares of a whole (e.g. the model mix).

use std::f64::consts::TAU;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::Widget,
};

use super::{BrailleGrid, clip, put_char, put_str};

/// Columns a legend to the right needs.
const LEGEND_MIN_WIDTH: u16 = 12;
/// The hole's radius as a fraction of the ring's.
const HOLE: f64 = 0.55;

/// A donut of `(name, value, colour)` shares, starting at 12 o'clock and
/// running clockwise, with optional center text and a `● name 42%` legend.
///
/// The ring is `2h` cells wide for `h` rows, which is round in braille dot
/// space. Shares that are negative or not finite count as zero; when nothing
/// is left only the track ring is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring<'a> {
    shares: &'a [(&'a str, f64, Color)],
    center: Option<&'a str>,
    legend: bool,
    track: Color,
    text: Color,
}

/// Where the legend goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Legend {
    Right,
    Below,
    Hidden,
}

/// A share's usable value.
fn weight(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

impl<'a> Ring<'a> {
    /// A ring of `shares` with the legend on.
    pub fn new(shares: &'a [(&'a str, f64, Color)]) -> Ring<'a> {
        Ring {
            shares,
            center: None,
            legend: true,
            track: Color::DarkGray,
            text: Color::Reset,
        }
    }

    /// Text in the hole, e.g. a total.
    pub fn center(mut self, text: &'a str) -> Ring<'a> {
        self.center = Some(text);
        self
    }

    /// Shows the legend when there is room.
    pub fn legend(mut self, on: bool) -> Ring<'a> {
        self.legend = on;
        self
    }

    /// The ring's colour when there is nothing to share.
    pub fn track(mut self, track: Color) -> Ring<'a> {
        self.track = track;
        self
    }

    /// The colour of the center text and legend names.
    pub fn text(mut self, color: Color) -> Ring<'a> {
        self.text = color;
        self
    }

    fn total(&self) -> f64 {
        self.shares.iter().map(|s| weight(s.1)).sum()
    }

    /// The colour at `turn` (0..1 clockwise from 12 o'clock).
    fn color_at(&self, turn: f64, total: f64) -> Color {
        if total <= 0.0 || !total.is_finite() {
            return self.track;
        }
        let mut end = 0.0;
        let mut last = self.track;
        for (_, value, color) in self.shares {
            if weight(*value) <= 0.0 {
                continue;
            }
            end += weight(*value) / total;
            last = *color;
            if turn < end {
                return *color;
            }
        }
        last
    }

    /// The ring's rectangle and where the legend goes.
    fn layout(&self, area: Rect) -> (Rect, Legend) {
        let natural = area.height.min(area.width / 2);
        let rows = u16::try_from(self.shares.len()).unwrap_or(u16::MAX);
        let shrunk = area
            .height
            .min(area.width.saturating_sub(LEGEND_MIN_WIDTH) / 2);
        let below = area.height.saturating_sub(rows).min(area.width / 2);
        let (size, place) = if !self.legend || self.shares.is_empty() {
            (natural, Legend::Hidden)
        } else if natural > 0 && area.width >= natural * 2 + LEGEND_MIN_WIDTH {
            (natural, Legend::Right)
        } else if below >= 2 {
            (below, Legend::Below)
        } else if shrunk >= 2 {
            (shrunk, Legend::Right)
        } else {
            (natural, Legend::Hidden)
        };
        let width = size * 2;
        let x = if place == Legend::Right {
            area.x
        } else {
            area.x + (area.width - width) / 2
        };
        (Rect::new(x, area.y, width, size), place)
    }

    fn draw_ring(&self, ring: Rect, buf: &mut Buffer) {
        let mut grid = BrailleGrid::new(ring.width, ring.height);
        let radius = f64::from(grid.dot_height()) / 2.0;
        let (cx, cy) = (f64::from(grid.dot_width()) / 2.0, radius);
        for y in 0..grid.dot_height() {
            for x in 0..grid.dot_width() {
                let d = (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy);
                if d <= radius && d >= radius * HOLE {
                    grid.set(x, y);
                }
            }
        }
        let total = self.total();
        grid.render_with(ring, buf, |col, row| {
            let dx = f64::from(col) * 2.0 + 1.0 - cx;
            let dy = f64::from(row) * 4.0 + 2.0 - cy;
            let turn = dx.atan2(-dy).rem_euclid(TAU) / TAU;
            self.color_at(turn, total)
        });
    }

    fn draw_center(&self, ring: Rect, buf: &mut Buffer, clip: Rect) {
        let Some(text) = self.center else { return };
        let width = u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX);
        let hole = (f64::from(ring.height) * 2.0 * HOLE).floor() as u16;
        if ring.height < 3 || width > hole.saturating_sub(1) {
            return;
        }
        let x = ring.x + (ring.width - width) / 2;
        let y = ring.y + ring.height / 2;
        put_str(buf, clip, x, y, text, Style::default().fg(self.text));
    }

    fn draw_legend(&self, area: Rect, buf: &mut Buffer) {
        let total = self.total();
        let style = Style::default().fg(self.text);
        for (i, (name, value, color)) in self.shares.iter().enumerate() {
            let Ok(offset) = u16::try_from(i) else { break };
            if offset >= area.height {
                break;
            }
            let y = area.y + offset;
            let percent = if total > 0.0 {
                weight(*value) / total * 100.0
            } else {
                0.0
            };
            put_char(buf, area, area.x, y, '●', *color);
            put_str(
                buf,
                area,
                area.x + 1,
                y,
                &format!(" {name} {percent:.0}%"),
                style,
            );
        }
    }
}

impl Widget for Ring<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() {
            return;
        }
        let (ring, place) = self.layout(area);
        self.draw_ring(ring, buf);
        self.draw_center(ring, buf, area);
        let legend = match place {
            Legend::Right => {
                let x = ring.right() + 1;
                Rect::new(x, area.y, area.right().saturating_sub(x), area.height)
            }
            Legend::Below => {
                let y = ring.bottom();
                Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y))
            }
            Legend::Hidden => return,
        };
        self.draw_legend(legend.intersection(area), buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const RED: Color = Color::Rgb(230, 90, 90);
    const BLUE: Color = Color::Rgb(110, 160, 235);
    const GREY: Color = Color::Rgb(75, 72, 90);

    fn braille_cells(buf: &Buffer) -> Vec<(u16, u16)> {
        let area = buf.area;
        (area.top()..area.bottom())
            .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
            .filter(|p| {
                buf[*p]
                    .symbol()
                    .chars()
                    .next()
                    .is_some_and(|c| ('⠁'..='⣿').contains(&c))
            })
            .collect()
    }

    #[test]
    fn halves_split_clockwise_from_noon() {
        let shares = [("opus", 1.0, RED), ("haiku", 1.0, BLUE)];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        Ring::new(&shares).legend(false).render(buf.area, &mut buf);
        let cells = braille_cells(&buf);
        assert!(cells.len() >= 12, "{cells:?}");
        for (x, y) in cells {
            let want = if x >= 4 { RED } else { BLUE };
            assert_eq!(buf[(x, y)].fg, want, "cell ({x}, {y})");
        }
        // The hole is empty.
        assert_eq!(buf[(3, 1)].symbol(), " ");
        assert_eq!(buf[(4, 2)].symbol(), " ");
    }

    #[test]
    fn legend_goes_right_with_percentages() {
        let shares = [("opus", 3.0, RED), ("haiku", 1.0, BLUE)];
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 3));
        Ring::new(&shares)
            .text(Color::White)
            .render(buf.area, &mut buf);
        assert!(row(&buf, 0).contains("● opus 75%"), "{:?}", row(&buf, 0));
        assert!(row(&buf, 1).contains("● haiku 25%"));
        let dot = (0..24)
            .find(|x| buf[(*x, 0)].symbol() == "●")
            .expect("legend dot");
        assert_eq!(buf[(dot, 0)].fg, RED);
        assert_eq!(buf[(dot + 2, 0)].fg, Color::White);
        assert!(dot >= 7, "legend sits right of the 6-wide ring");
    }

    #[test]
    fn legend_goes_below_when_narrow() {
        let shares = [("a", 1.0, RED)];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        Ring::new(&shares).render(buf.area, &mut buf);
        assert!(row(&buf, 3).starts_with("● a 100%"), "{:?}", row(&buf, 3));
    }

    #[test]
    fn center_text_and_track_only() {
        let shares = [("x", f64::NAN, RED), ("y", -2.0, BLUE)];
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 6));
        Ring::new(&shares)
            .legend(false)
            .center("42")
            .track(GREY)
            .text(Color::White)
            .render(buf.area, &mut buf);
        assert_eq!(buf[(5, 3)].symbol(), "4");
        assert_eq!(buf[(6, 3)].symbol(), "2");
        assert_eq!(buf[(5, 3)].fg, Color::White);
        let cells = braille_cells(&buf);
        assert!(!cells.is_empty());
        assert!(cells.iter().all(|p| buf[*p].fg == GREY));
    }

    #[test]
    fn size_fuzz_never_panics() {
        let shares = [
            ("nan", f64::NAN, RED),
            ("inf", f64::INFINITY, BLUE),
            ("neg", -1.0, GREY),
            ("ok", 2.0, RED),
            ("big", 1e308, BLUE),
        ];
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for list in [&shares[..], &shares[3..], &[]] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
                    let ring = Ring::new(list).center("12.3k tokens");
                    ring.render(Rect::new(w % 7, h % 3, w, h), &mut buf);
                    ring.legend(false).render(Rect::new(0, 0, w, h), &mut buf);
                }
            }
        }
    }
}
