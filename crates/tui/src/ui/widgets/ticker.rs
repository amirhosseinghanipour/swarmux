//! A news-ticker line that scrolls left as the tick advances.

use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

use super::{clip, put_char};

/// Placed between items and between the end of the text and its restart.
pub const SEPARATOR: &str = "  ·  ";

/// `width` items of the endless loop `seq, sep, seq, sep, …` starting
/// `offset` items in; `seq` itself when it already fits.
fn window<T: Copy>(seq: &[T], sep: &[T], width: usize, offset: u64) -> Vec<T> {
    if seq.len() <= width {
        return seq.to_vec();
    }
    let cycle: Vec<T> = seq.iter().chain(sep).copied().collect();
    let len = u64::try_from(cycle.len()).unwrap_or(u64::MAX).max(1);
    let start = usize::try_from(offset % len).unwrap_or(0);
    cycle
        .iter()
        .cycle()
        .skip(start)
        .take(width)
        .copied()
        .collect()
}

/// The `width` characters of `text` visible after scrolling `offset`
/// characters, wrapping around with `  ·  `. Text that fits is returned
/// unchanged (not padded).
pub fn ticker_window(text: &str, width: usize, offset: u64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let sep: Vec<char> = SEPARATOR.chars().collect();
    window(&chars, &sep, width, offset).into_iter().collect()
}

/// Items joined by `  ·  `, scrolling one character every `speed` ticks
/// when they do not fit (left-aligned and still when they do). Scrolling is
/// per character, so wide characters may shift the line by a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticker<'a> {
    items: &'a [String],
    tick: u64,
    speed: u64,
    color: Color,
    sep_color: Color,
}

impl<'a> Ticker<'a> {
    /// A ticker over `items`.
    pub fn new(items: &'a [String]) -> Ticker<'a> {
        Ticker {
            items,
            tick: 0,
            speed: 1,
            color: Color::Reset,
            sep_color: Color::DarkGray,
        }
    }

    /// The current animation tick.
    pub fn tick(mut self, tick: u64) -> Ticker<'a> {
        self.tick = tick;
        self
    }

    /// Ticks per character scrolled (0 is treated as 1).
    pub fn speed(mut self, speed: u64) -> Ticker<'a> {
        self.speed = speed.max(1);
        self
    }

    /// The colour of the items.
    pub fn color(mut self, color: Color) -> Ticker<'a> {
        self.color = color;
        self
    }

    /// The colour of the separators.
    pub fn sep_color(mut self, color: Color) -> Ticker<'a> {
        self.sep_color = color;
        self
    }

    /// Every character of the joined items, flagged `true` for separators.
    fn chars(&self) -> Vec<(char, bool)> {
        let mut out = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                out.extend(SEPARATOR.chars().map(|c| (c, true)));
            }
            out.extend(item.chars().map(|c| (c, false)));
        }
        out
    }
}

impl Widget for Ticker<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = clip(area, buf);
        if area.is_empty() {
            return;
        }
        let sep: Vec<(char, bool)> = SEPARATOR.chars().map(|c| (c, true)).collect();
        let offset = self.tick / self.speed.max(1);
        let visible = window(&self.chars(), &sep, usize::from(area.width), offset);
        for (x, (ch, is_sep)) in (area.left()..area.right()).zip(visible) {
            let color = if is_sep { self.sep_color } else { self.color };
            put_char(buf, area, x, area.y, ch, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::row;

    #[test]
    fn windows_scroll_and_wrap() {
        assert_eq!(ticker_window("abc", 5, 7), "abc");
        assert_eq!(ticker_window("abcdef", 4, 0), "abcd");
        assert_eq!(ticker_window("abcdef", 4, 3), "def ");
        assert_eq!(ticker_window("abcdef", 4, 5), "f  ·");
        assert_eq!(ticker_window("abcdef", 4, 11), "abcd");
        assert_eq!(ticker_window("abcdef", 0, 3), "");
        assert_eq!(ticker_window("", 0, 3), "");
        assert_eq!(ticker_window("åäö✓xyz", 3, 2), "ö✓x");
        assert_eq!(
            ticker_window("abcdef", 4, u64::MAX),
            ticker_window("abcdef", 4, u64::MAX % 11)
        );
    }

    #[test]
    fn renders_items_and_coloured_separators() {
        let items = vec!["cpu 12%".to_string(), "mem 3G".to_string()];
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Ticker::new(&items)
            .tick(6)
            .speed(2)
            .color(Color::White)
            .sep_color(Color::Gray)
            .render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), " 12%  ·  m");
        assert_eq!(buf[(1, 0)].fg, Color::White);
        assert_eq!(buf[(6, 0)].fg, Color::Gray);
        let mut wide = Buffer::empty(Rect::new(0, 0, 20, 1));
        Ticker::new(&items).tick(99).render(wide.area, &mut wide);
        assert_eq!(row(&wide, 0), "cpu 12%  ·  mem 3G  ");
    }

    #[test]
    fn size_fuzz_never_panics() {
        let items = vec!["漢字 wide".to_string(), String::new(), "x".repeat(50)];
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                for tick in [0, 1, 17, u64::MAX] {
                    let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
                    Ticker::new(&items)
                        .tick(tick)
                        .speed(0)
                        .render(Rect::new(w % 7, h % 3, w, h), &mut buf);
                    Ticker::new(&[]).render(Rect::new(0, 0, w, h), &mut buf);
                    let _ = ticker_window("odd ✓", usize::from(w), tick);
                }
            }
        }
    }
}
