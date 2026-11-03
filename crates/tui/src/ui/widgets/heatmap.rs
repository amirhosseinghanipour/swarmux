//! A GitHub-style activity calendar: weeks as columns, weekdays as rows.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

use super::{clip, put_str};

/// Weekday labels by row (Monday first).
const WEEKDAY_LABELS: [&str; 7] = ["M", "", "W", "", "F", "", ""];

/// The heat level (0..=4) of `count` when the busiest day has `max`:
/// 0 for no activity, then the quartile of `max` the count falls in.
pub fn level(count: u32, max: u32) -> usize {
    if count == 0 || max == 0 {
        return 0;
    }
    let quartile = (u64::from(count) * 4).div_ceil(u64::from(max));
    usize::try_from(quartile.clamp(1, 4)).unwrap_or(4)
}

/// One glyph per day, Monday on the top row, the newest week on the right.
///
/// Shows as many recent weeks as fit: two columns per week (glyph and a
/// space) when every week fits that way, else one. Areas shorter than seven
/// rows show the top (earliest weekday) rows only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heatmap<'a> {
    counts: &'a [u32],
    levels: [Color; 5],
    glyph: &'a str,
    weekday_labels: bool,
    today_weekday: u8,
    label_color: Color,
}

impl<'a> Heatmap<'a> {
    /// A calendar of daily `counts` (oldest first, the last is today),
    /// coloured by the five `levels` (empty → busiest).
    pub fn new(counts: &'a [u32], levels: [Color; 5]) -> Heatmap<'a> {
        Heatmap {
            counts,
            levels,
            glyph: "■",
            weekday_labels: false,
            today_weekday: 0,
            label_color: Color::Reset,
        }
    }

    /// The glyph for a day (default `■`).
    pub fn glyph(mut self, glyph: &'a str) -> Heatmap<'a> {
        self.glyph = glyph;
        self
    }

    /// Shows `M W F` on the left when there is room.
    pub fn weekday_labels(mut self, on: bool) -> Heatmap<'a> {
        self.weekday_labels = on;
        self
    }

    /// Today's weekday, 0 = Monday … 6 = Sunday (taken modulo 7).
    pub fn today_weekday(mut self, weekday: u8) -> Heatmap<'a> {
        self.today_weekday = weekday % 7;
        self
    }

    /// The colour of the weekday labels.
    pub fn label_color(mut self, color: Color) -> Heatmap<'a> {
        self.label_color = color;
        self
    }

    /// `(weeks back, weekday row)` of the day `back` days before today.
    fn place(&self, back: usize) -> (usize, usize) {
        let today = usize::from(self.today_weekday % 7);
        let week = (back + 6 - today) / 7;
        let row = (today + 7 - back % 7) % 7;
        (week, row)
    }
}

impl Widget for Heatmap<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() || self.counts.is_empty() {
            return;
        }
        let gutter = if self.weekday_labels && area.width >= 6 {
            2
        } else {
            0
        };
        if gutter > 0 {
            let style = Style::default().fg(self.label_color);
            for (y, label) in (area.top()..area.bottom()).zip(WEEKDAY_LABELS) {
                put_str(buf, area, area.x, y, label, style);
            }
        }
        let room = usize::from(area.width - gutter);
        let weeks = self.place(self.counts.len() - 1).0 + 1;
        let cell = if room >= weeks * 2 { 2 } else { 1 };
        let visible = room / cell;
        let max = self.counts.iter().copied().max().unwrap_or(0);
        for (back, count) in self.counts.iter().rev().enumerate() {
            let (week, weekday) = self.place(back);
            if week >= visible {
                break;
            }
            let Ok(row) = u16::try_from(weekday) else {
                continue;
            };
            let Ok(offset) = u16::try_from((week + 1) * cell) else {
                continue;
            };
            let color = self
                .levels
                .get(level(*count, max))
                .copied()
                .unwrap_or(Color::Reset);
            let column = Rect {
                x: area.right() - offset,
                width: 1,
                ..area
            };
            put_str(
                buf,
                column,
                column.x,
                area.y + row,
                self.glyph,
                Style::default().fg(color),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    const LEVELS: [Color; 5] = [
        Color::Rgb(0, 0, 0),
        Color::Rgb(1, 1, 1),
        Color::Rgb(2, 2, 2),
        Color::Rgb(3, 3, 3),
        Color::Rgb(4, 4, 4),
    ];

    #[test]
    fn levels_are_quartiles_of_the_max() {
        assert_eq!(level(0, 10), 0);
        assert_eq!(level(5, 0), 0);
        assert_eq!(level(1, 100), 1);
        assert_eq!(level(25, 100), 1);
        assert_eq!(level(26, 100), 2);
        assert_eq!(level(75, 100), 3);
        assert_eq!(level(100, 100), 4);
        assert_eq!(level(u32::MAX, 1), 4);
    }

    #[test]
    fn today_lands_on_its_weekday_in_the_last_column() {
        // Ten days ending on a Wednesday (row 2).
        let counts = [1, 0, 0, 0, 0, 0, 0, 0, 0, 8];
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 7));
        Heatmap::new(&counts, LEVELS)
            .today_weekday(2)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "  ■ ■ ");
        assert_eq!(row(&buf, 2), "  ■ ■ ");
        assert_eq!(row(&buf, 3), "  ■   ");
        assert_eq!(row(&buf, 6), "  ■   ");
        assert_eq!(buf[(4, 2)].fg, LEVELS[4]);
        assert_eq!(buf[(4, 0)].fg, LEVELS[0]);
        // The oldest day is last week's Monday.
        assert_eq!(buf[(2, 0)].fg, LEVELS[1]);
    }

    #[test]
    fn narrow_areas_pack_weeks_and_keep_the_newest() {
        let counts = [3u32; 21];
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 7));
        Heatmap::new(&counts, LEVELS)
            .today_weekday(6)
            .glyph("●")
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "●●");
        assert_eq!(buf[(1, 0)].fg, LEVELS[4]);
    }

    #[test]
    fn weekday_labels_on_the_left() {
        let counts = [1u32; 7];
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 5));
        Heatmap::new(&counts, LEVELS)
            .today_weekday(6)
            .weekday_labels(true)
            .label_color(Color::White)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), "M     ■ ");
        assert_eq!(row(&buf, 1), "      ■ ");
        assert_eq!(row(&buf, 4), "F     ■ ");
        assert_eq!(buf[(0, 0)].fg, Color::White);
    }

    #[test]
    fn size_fuzz_never_panics() {
        let counts: Vec<u32> = (0..400)
            .map(|i| (i * 7919) % 13)
            .chain([u32::MAX, 0])
            .collect();
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for data in [&counts[..], &[], &[0], &[5]] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
                    let map = Heatmap::new(data, LEVELS)
                        .weekday_labels(true)
                        .today_weekday(200);
                    map.render(Rect::new(w % 7, h % 3, w, h), &mut buf);
                    map.glyph("").render(Rect::new(0, 0, w, h), &mut buf);
                }
            }
        }
    }
}
