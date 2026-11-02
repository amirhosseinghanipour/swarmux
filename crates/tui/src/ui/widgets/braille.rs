//! A braille dot canvas: each terminal cell holds a 2 × 4 grid of dots.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// The first braille code point (the empty pattern).
const BRAILLE_BASE: u32 = 0x2800;

/// The bit for the dot at `(dx, dy)` inside a cell (`dx` 0..2, `dy` 0..4).
fn dot_bit(dx: i32, dy: i32) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

/// A `w` × `h` cell canvas addressed in dots (`2w` × `4h`, origin top-left).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrailleGrid {
    w: u16,
    h: u16,
    dots: Vec<u8>,
}

impl BrailleGrid {
    /// An empty canvas of `w` × `h` cells.
    pub fn new(w: u16, h: u16) -> BrailleGrid {
        BrailleGrid {
            w,
            h,
            dots: vec![0; usize::from(w) * usize::from(h)],
        }
    }

    /// Width in cells.
    pub fn width(&self) -> u16 {
        self.w
    }

    /// Height in cells.
    pub fn height(&self) -> u16 {
        self.h
    }

    /// Width in dots (`2 × width`).
    pub fn dot_width(&self) -> i32 {
        i32::from(self.w) * 2
    }

    /// Height in dots (`4 × height`).
    pub fn dot_height(&self) -> i32 {
        i32::from(self.h) * 4
    }

    /// Turns on the dot at `(x, y)`; dots outside the canvas are ignored.
    pub fn set(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 || x >= self.dot_width() || y >= self.dot_height() {
            return;
        }
        let index = (y / 4) as usize * usize::from(self.w) + (x / 2) as usize;
        if let Some(cell) = self.dots.get_mut(index) {
            *cell |= dot_bit(x % 2, y % 4);
        }
    }

    /// Draws a straight line of dots from `(x0, y0)` to `(x1, y1)`
    /// (Bresenham), clipped to the canvas.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let Some((x0, y0, x1, y1)) = self.clip_segment(x0, y0, x1, y1) else {
            return;
        };
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = ((x1 - x0).signum(), (y1 - y0).signum());
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.set(x, y);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// The segment cut to the canvas (Liang–Barsky), or `None` when it
    /// misses it. Segments already inside are returned unchanged.
    fn clip_segment(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<(i32, i32, i32, i32)> {
        let (max_x, max_y) = (self.dot_width() - 1, self.dot_height() - 1);
        if max_x < 0 || max_y < 0 {
            return None;
        }
        let within = |x: i32, y: i32| (0..=max_x).contains(&x) && (0..=max_y).contains(&y);
        if within(x0, y0) && within(x1, y1) {
            return Some((x0, y0, x1, y1));
        }
        let (fx0, fy0) = (f64::from(x0), f64::from(y0));
        let (dx, dy) = (f64::from(x1) - fx0, f64::from(y1) - fy0);
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let edges = [
            (-dx, fx0),
            (dx, f64::from(max_x) - fx0),
            (-dy, fy0),
            (dy, f64::from(max_y) - fy0),
        ];
        for (p, q) in edges {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else if p < 0.0 {
                t0 = t0.max(q / p);
            } else {
                t1 = t1.min(q / p);
            }
        }
        if t0 > t1 {
            return None;
        }
        let at = |t: f64, start: f64, delta: f64, max: i32| {
            ((start + t * delta).round() as i32).clamp(0, max)
        };
        Some((
            at(t0, fx0, dx, max_x),
            at(t0, fy0, dy, max_y),
            at(t1, fx0, dx, max_x),
            at(t1, fy0, dy, max_y),
        ))
    }

    /// The raw dot bits of cell `(cx, cy)` (0 when empty or out of range).
    pub fn bits(&self, cx: u16, cy: u16) -> u8 {
        if cx >= self.w || cy >= self.h {
            return 0;
        }
        let index = usize::from(cy) * usize::from(self.w) + usize::from(cx);
        self.dots.get(index).copied().unwrap_or(0)
    }

    /// The braille glyph for cell `(cx, cy)`, or `None` when it has no dots.
    pub fn cell(&self, cx: u16, cy: u16) -> Option<char> {
        match self.bits(cx, cy) {
            0 => None,
            bits => char::from_u32(BRAILLE_BASE + u32::from(bits)),
        }
    }

    /// Adds every dot of `other` to this canvas (cells beyond either size
    /// are ignored).
    pub fn overlay(&mut self, other: &BrailleGrid) {
        for cy in 0..self.h.min(other.h) {
            for cx in 0..self.w.min(other.w) {
                let index = usize::from(cy) * usize::from(self.w) + usize::from(cx);
                if let Some(cell) = self.dots.get_mut(index) {
                    *cell |= other.bits(cx, cy);
                }
            }
        }
    }

    /// Draws the non-empty cells into `area` (top-left aligned, clipped to
    /// `area` and `buf`), colouring cell `(cx, cy)` with `color(cx, cy)`.
    pub fn render_with(&self, area: Rect, buf: &mut Buffer, color: impl Fn(u16, u16) -> Color) {
        let clip = super::clip(area, buf);
        for cy in 0..self.h.min(area.height) {
            for cx in 0..self.w.min(area.width) {
                if let Some(ch) = self.cell(cx, cy) {
                    let (x, y) = (area.x.saturating_add(cx), area.y.saturating_add(cy));
                    super::put_char(buf, clip, x, y, ch, color(cx, cy));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_bits_follow_the_braille_standard() {
        let mut g = BrailleGrid::new(1, 1);
        assert_eq!(g.cell(0, 0), None);
        g.set(0, 0);
        assert_eq!(g.cell(0, 0), Some('⠁'));
        g.set(1, 3);
        assert_eq!(g.cell(0, 0), Some('\u{2881}'));
        let mut full = BrailleGrid::new(1, 1);
        for y in 0..4 {
            for x in 0..2 {
                full.set(x, y);
            }
        }
        assert_eq!(full.cell(0, 0), Some('⣿'));
    }

    #[test]
    fn sizes_and_out_of_range_dots() {
        let mut g = BrailleGrid::new(3, 2);
        assert_eq!((g.dot_width(), g.dot_height()), (6, 8));
        for (x, y) in [(-1, 0), (0, -1), (6, 0), (0, 8), (i32::MAX, i32::MIN)] {
            g.set(x, y);
        }
        assert!((0..2).all(|cy| (0..3).all(|cx| g.cell(cx, cy).is_none())));
        assert_eq!(g.cell(3, 0), None);
        assert_eq!(g.cell(0, 2), None);
    }

    #[test]
    fn lines_connect_and_clip() {
        let mut g = BrailleGrid::new(2, 1);
        g.line(0, 3, 3, 3);
        assert_eq!(g.cell(0, 0), Some('⣀'));
        assert_eq!(g.cell(1, 0), Some('⣀'));
        let mut d = BrailleGrid::new(1, 1);
        d.line(0, 0, 1, 3);
        assert_ne!(d.cell(0, 0), None);
        let mut far = BrailleGrid::new(4, 2);
        far.line(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        far.line(-50, 2, 50, 2);
        assert!(far.cell(0, 0).is_some() && far.cell(3, 0).is_some());
        far.line(100, 100, 200, 200);
    }

    #[test]
    fn renders_only_dotted_cells_with_their_colour() {
        let mut g = BrailleGrid::new(3, 1);
        g.set(0, 0);
        g.set(5, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        let untouched = buf[(1, 0)].clone();
        g.render_with(buf.area, &mut buf, |cx, _| {
            if cx == 0 { Color::Red } else { Color::Blue }
        });
        assert_eq!(buf[(0, 0)].symbol(), "⠁");
        assert_eq!(buf[(0, 0)].fg, Color::Red);
        assert_eq!(buf[(1, 0)], untouched);
        assert_eq!(buf[(2, 0)].symbol(), "⢀");
        assert_eq!(buf[(2, 0)].fg, Color::Blue);
    }

    #[test]
    fn overlay_adds_dots() {
        let mut a = BrailleGrid::new(1, 1);
        let mut b = BrailleGrid::new(2, 2);
        a.set(0, 0);
        b.set(1, 0);
        a.overlay(&b);
        assert_eq!(a.cell(0, 0), Some('⠉'));
    }

    #[test]
    fn size_fuzz_never_panics() {
        for w in 0..=40u16 {
            for h in 0..=12u16 {
                let mut g = BrailleGrid::new(w, h);
                g.line(-3, -7, i32::from(w) * 3, i32::from(h) * 5);
                g.set(1, 1);
                let mut buf = Buffer::empty(Rect::new(0, 0, 20, 6));
                g.render_with(Rect::new(15, 3, w, h), &mut buf, |_, _| Color::Green);
                g.render_with(Rect::new(0, 0, w, h), &mut buf, |_, _| Color::Green);
            }
        }
    }
}
