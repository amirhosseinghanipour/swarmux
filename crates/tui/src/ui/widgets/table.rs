//! A compact table: fixed or filling columns, `…` clipping, zebra rows.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Widget,
};

use super::{clip, put_str};

/// A dense table of one span per cell with a 1-column gap between columns.
///
/// A width of 0 makes a column share the space left by the fixed ones.
/// Text that does not fit ends in `…`. Rows beyond the height are dropped
/// and the last visible row then reads `+N more`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiniTable<'a> {
    header: Option<Vec<Span<'a>>>,
    rows: Vec<Vec<Span<'a>>>,
    widths: Vec<u16>,
    header_color: Color,
    zebra: Option<Color>,
    right_align: Vec<bool>,
}

impl<'a> MiniTable<'a> {
    /// A table of `rows` with column `widths` (0 = fill).
    pub fn new(rows: Vec<Vec<Span<'a>>>, widths: Vec<u16>) -> MiniTable<'a> {
        MiniTable {
            rows,
            widths,
            header_color: Color::Reset,
            ..MiniTable::default()
        }
    }

    /// A bold header row.
    pub fn header(mut self, header: Vec<Span<'a>>) -> MiniTable<'a> {
        self.header = Some(header);
        self
    }

    /// The colour of the header and the `+N more` row.
    pub fn header_color(mut self, color: Color) -> MiniTable<'a> {
        self.header_color = color;
        self
    }

    /// A background for every other row (the 2nd, 4th, …).
    pub fn zebra(mut self, bg: Color) -> MiniTable<'a> {
        self.zebra = Some(bg);
        self
    }

    /// Which columns are right-aligned (missing entries are left-aligned).
    pub fn right_align(mut self, right: Vec<bool>) -> MiniTable<'a> {
        self.right_align = right;
        self
    }

    /// `(x offset, width)` of each column within `width` cells.
    fn columns(&self, width: u16) -> Vec<(u16, u16)> {
        let count = u16::try_from(self.widths.len()).unwrap_or(u16::MAX);
        let fixed = self
            .widths
            .iter()
            .fold(0u16, |sum, w| sum.saturating_add(*w));
        let fills = self.widths.iter().filter(|w| **w == 0).count();
        let gaps = count.saturating_sub(1);
        let rest = width.saturating_sub(fixed.saturating_add(gaps));
        let share = u16::try_from(fills)
            .ok()
            .filter(|n| *n > 0)
            .map_or(0, |n| rest / n);
        let mut x = 0u16;
        self.widths
            .iter()
            .map(|w| {
                let want = if *w == 0 { share } else { *w };
                let col = (x, want.min(width.saturating_sub(x)));
                x = x.saturating_add(want).saturating_add(1);
                col
            })
            .collect()
    }

    /// Draws one row of cells at `y`.
    fn draw_row(&self, buf: &mut Buffer, area: Rect, y: u16, cells: &[Span], base: Style) {
        for (i, ((x, width), span)) in self.columns(area.width).into_iter().zip(cells).enumerate() {
            let column = Rect::new(area.x + x, y, width, 1).intersection(area);
            let right = self.right_align.get(i).copied().unwrap_or(false);
            draw_cell(buf, column, span, right, base);
        }
    }
}

/// Writes `span` into the one-row `column`, aligned, ending in `…` when cut.
fn draw_cell(buf: &mut Buffer, column: Rect, span: &Span, right: bool, base: Style) {
    if column.is_empty() {
        return;
    }
    let style = base.patch(span.style);
    let text_width = span.width();
    let room = usize::from(column.width);
    if text_width <= room {
        let pad = u16::try_from(room - text_width).unwrap_or(0);
        let x = if right { column.x + pad } else { column.x };
        put_str(buf, column, x, column.y, &span.content, style);
    } else {
        let cut = Rect {
            width: column.width - 1,
            ..column
        };
        let end = put_str(buf, cut, column.x, column.y, &span.content, style);
        put_str(buf, column, end, column.y, "…", style);
    }
}

impl Widget for MiniTable<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() {
            return;
        }
        let mut y = area.y;
        if let Some(header) = &self.header {
            let style = Style::default()
                .fg(self.header_color)
                .add_modifier(Modifier::BOLD);
            let header: Vec<Span> = header
                .iter()
                .map(|s| Span::styled(s.content.clone(), s.style.fg(self.header_color)))
                .collect();
            self.draw_row(buf, area, y, &header, style);
            y += 1;
        }
        let room = usize::from(area.bottom().saturating_sub(y));
        let shown = if self.rows.len() > room {
            room.saturating_sub(1)
        } else {
            self.rows.len()
        };
        for (i, cells) in self.rows.iter().take(shown).enumerate() {
            let base = match self.zebra {
                Some(bg) if i % 2 == 1 => {
                    buf.set_style(
                        Rect {
                            y,
                            height: 1,
                            ..area
                        },
                        Style::default().bg(bg),
                    );
                    Style::default().bg(bg)
                }
                _ => Style::default(),
            };
            self.draw_row(buf, area, y, cells, base);
            y += 1;
        }
        let hidden = self.rows.len() - shown;
        if hidden > 0 && y < area.bottom() {
            let text = format!("+{hidden} more");
            put_str(
                buf,
                area,
                area.x,
                y,
                &text,
                Style::default().fg(self.header_color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const HEAD: Color = Color::Rgb(150, 140, 180);
    const ZEBRA: Color = Color::Rgb(30, 30, 40);

    fn cells(texts: &[&'static str]) -> Vec<Span<'static>> {
        texts.iter().map(|t| Span::raw(*t)).collect()
    }

    #[test]
    fn columns_fill_clip_and_align() {
        let rows = vec![
            cells(&["alpha", "12", "a very long note"]),
            vec![
                Span::raw("b"),
                Span::styled("3", Style::default().fg(Color::Red)),
                Span::raw("ok"),
            ],
        ];
        let mut buf = Buffer::empty(Rect::new(0, 0, 16, 3));
        MiniTable::new(rows, vec![4, 3, 0])
            .header(cells(&["name", "n", "note"]))
            .header_color(HEAD)
            .right_align(vec![false, true])
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "name   n note   ");
        assert_eq!(row(&buf, 1), "alp…  12 a very…");
        assert_eq!(row(&buf, 2), "b      3 ok     ");
        assert_eq!(buf[(0, 0)].fg, HEAD);
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(7, 2)].fg, Color::Red);
    }

    #[test]
    fn zebra_rows_and_overflow_marker() {
        let rows: Vec<Vec<Span>> = (0..5).map(|i| vec![Span::raw(format!("r{i}"))]).collect();
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        MiniTable::new(rows, vec![0])
            .zebra(ZEBRA)
            .header_color(HEAD)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "r0      ");
        assert_eq!(row(&buf, 1), "r1      ");
        assert_eq!(row(&buf, 2), "+3 more ");
        assert_eq!(buf[(7, 1)].bg, ZEBRA);
        assert_eq!(buf[(0, 1)].bg, ZEBRA);
        assert_ne!(buf[(0, 0)].bg, ZEBRA);
        assert_eq!(buf[(0, 2)].fg, HEAD);
    }

    #[test]
    fn size_fuzz_never_panics() {
        let rows: Vec<Vec<Span>> = (0..9)
            .map(|i| cells(&["x", "wide ✓ 漢字 text", "", "extra cell beyond widths"][..=(i % 4)]))
            .collect();
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for widths in [vec![], vec![0], vec![3, 0, 0], vec![200, 1, 0]] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
                    MiniTable::new(rows.clone(), widths.clone())
                        .header(cells(&["h1", "h2"]))
                        .zebra(ZEBRA)
                        .right_align(vec![true, true])
                        .render(Rect::new(w % 7, h % 3, w, h), &mut buf);
                    MiniTable::new(vec![], widths).render(Rect::new(0, 0, w, h), &mut buf);
                }
            }
        }
    }
}
