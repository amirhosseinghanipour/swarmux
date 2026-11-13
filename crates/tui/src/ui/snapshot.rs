//! A screen drawn into a `TestBackend` buffer, with the questions a layout
//! test asks of it: what is on which row, where does a word start, what
//! does a cell look like. The helper the layout tests of new screens build
//! on; it draws through the same `draw` the terminal uses.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};

use super::render_buffer;
use crate::app::App;

/// Rows above the body: the header and the tab bar.
pub(crate) const TOP_ROWS: u16 = 2;
/// Rows below the body: the status bar.
pub(crate) const BOTTOM_ROWS: u16 = 1;

/// One frame of the app at one size.
pub(crate) struct Snapshot {
    buf: Buffer,
}

/// How one cell looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Look {
    pub fg: Color,
    pub bg: Color,
    pub modifier: Modifier,
}

impl Snapshot {
    /// The app as it draws at `width` × `height`.
    pub fn of(app: &App, width: u16, height: u16) -> Snapshot {
        Snapshot {
            buf: render_buffer(app, width, height),
        }
    }

    pub fn width(&self) -> u16 {
        self.buf.area.width
    }

    pub fn height(&self) -> u16 {
        self.buf.area.height
    }

    /// The body: what a screen draws into, between the tab bar and the
    /// status bar.
    pub fn body(&self) -> Rect {
        Rect::new(
            0,
            TOP_ROWS,
            self.width(),
            self.height().saturating_sub(TOP_ROWS + BOTTOM_ROWS),
        )
    }

    /// Row `y` as text, a cell to a character.
    pub fn row(&self, y: u16) -> String {
        self.region(Rect::new(0, y, self.width(), 1))
    }

    /// The header row.
    pub fn header(&self) -> String {
        self.row(0)
    }

    /// The tab bar.
    pub fn tabs(&self) -> String {
        self.row(1)
    }

    /// The status bar.
    pub fn status(&self) -> String {
        self.row(self.height() - 1)
    }

    /// The cells of `area` as text, one line per row, no trailing spaces.
    pub fn region(&self, area: Rect) -> String {
        let area = area.intersection(self.buf.area);
        (area.y..area.bottom())
            .map(|y| {
                let line: String = (area.x..area.right())
                    .map(|x| self.buf[(x, y)].symbol())
                    .collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The whole frame as text, one line per row.
    pub fn text(&self) -> String {
        self.region(self.buf.area)
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.find(needle).is_some()
    }

    /// The cell where `needle` starts, reading rows from the top. A word
    /// that wraps onto two rows is not found.
    pub fn find(&self, needle: &str) -> Option<(u16, u16)> {
        self.find_in(self.buf.area, needle)
    }

    /// Like [`Snapshot::find`], within `area`.
    pub fn find_in(&self, area: Rect, needle: &str) -> Option<(u16, u16)> {
        let area = area.intersection(self.buf.area);
        for y in area.y..area.bottom() {
            let cells: Vec<&str> = (area.x..area.right())
                .map(|x| self.buf[(x, y)].symbol())
                .collect();
            let line: String = cells.concat();
            if let Some(at) = line.find(needle) {
                // The byte offset of the match, as the cell it starts in.
                let mut bytes = 0;
                for (i, symbol) in cells.iter().enumerate() {
                    if bytes >= at {
                        return Some((area.x + i as u16, y));
                    }
                    bytes += symbol.len();
                }
            }
        }
        None
    }

    /// How the cell at `x`, `y` looks.
    pub fn look(&self, x: u16, y: u16) -> Look {
        let cell = &self.buf[(x, y)];
        Look {
            fg: cell.fg,
            bg: cell.bg,
            modifier: cell.modifier,
        }
    }

    /// Whether any cell of `area` shows something other than a space.
    pub fn has_content(&self, area: Rect) -> bool {
        let area = area.intersection(self.buf.area);
        (area.y..area.bottom())
            .any(|y| (area.x..area.right()).any(|x| self.buf[(x, y)].symbol().trim() != ""))
    }
}
