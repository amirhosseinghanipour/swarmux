//! Multi-line text editor (UTF-8 aware; columns count chars, not bytes).

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Editor {
    /// Never empty: an empty text is one empty line.
    lines: Vec<String>,
    row: usize,
    col: usize,
    /// First visible line. Rendering moves it so the cursor stays in view.
    top: Cell<usize>,
}

impl Default for Editor {
    fn default() -> Editor {
        Editor::new("")
    }
}

impl Editor {
    /// Starts with the cursor at the beginning of `text`.
    pub fn new(text: &str) -> Editor {
        Editor {
            lines: text
                .split('\n')
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect(),
            row: 0,
            col: 0,
            top: Cell::new(0),
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// (line, column in chars).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn handle(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_at(self.col);
                self.lines[self.row].insert(at, c);
                self.col += 1;
            }
            KeyCode::Enter => self.split_line(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Up if self.row > 0 => self.move_to_row(self.row - 1),
            KeyCode::Down if self.row + 1 < self.lines.len() => self.move_to_row(self.row + 1),
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.line_len(self.row),
            _ => {}
        }
    }

    fn split_line(&mut self) {
        let at = self.byte_at(self.col);
        let rest = self.lines[self.row].split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let at = self.byte_at(self.col);
            self.lines[self.row].remove(at);
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len(self.row);
            self.lines[self.row].push_str(&line);
        }
    }

    fn delete(&mut self) {
        if self.col < self.line_len(self.row) {
            let at = self.byte_at(self.col);
            self.lines[self.row].remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.line_len(self.row);
        }
    }

    fn right(&mut self) {
        if self.col < self.line_len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_to_row(&mut self, row: usize) {
        self.row = row;
        self.col = self.col.min(self.line_len(row));
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    fn byte_at(&self, col: usize) -> usize {
        let line = &self.lines[self.row];
        line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
    }

    /// First line to show in a view `height` lines tall, keeping the cursor visible.
    fn scroll_top(&self, height: usize) -> usize {
        let mut top = self.top.get();
        if self.row < top {
            top = self.row;
        } else if height > 0 && self.row >= top + height {
            top = self.row + 1 - height;
        }
        self.top.set(top);
        top
    }
}

/// Draws the editor inside `block`; the cursor is shown only when `focused`.
pub fn render(frame: &mut Frame, area: Rect, editor: &Editor, block: Block, focused: bool) {
    let inner = block.inner(area);
    let (height, width) = (inner.height as usize, inner.width as usize);
    let top = editor.scroll_top(height);
    let left = if focused {
        editor.col.saturating_sub(width.saturating_sub(1))
    } else {
        0
    };
    let lines: Vec<Line> = editor
        .lines
        .iter()
        .enumerate()
        .skip(top)
        .take(height)
        .map(|(row, text)| {
            let visible: String = text.chars().skip(left).take(width).collect();
            if focused && row == editor.row {
                cursor_line(&visible, editor.col - left)
            } else {
                Line::raw(visible)
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn cursor_line(text: &str, col: usize) -> Line<'static> {
    let before: String = text.chars().take(col).collect();
    let mut rest = text.chars().skip(col);
    let at = rest.next().map_or(" ".to_string(), String::from);
    Line::from(vec![
        Span::raw(before),
        Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
        Span::raw(rest.collect::<String>()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(e: &mut Editor, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                e.handle(key(KeyCode::Enter));
            } else {
                e.handle(key(KeyCode::Char(c)));
            }
        }
    }

    #[test]
    fn typing_lines_with_utf8() {
        let mut e = Editor::default();
        type_str(&mut e, "né🐝\nzwei");
        assert_eq!(e.text(), "né🐝\nzwei");
        assert_eq!(e.cursor(), (1, 4));
    }

    #[test]
    fn enter_splits_in_the_middle() {
        let mut e = Editor::new("héllo");
        e.handle(key(KeyCode::Right));
        e.handle(key(KeyCode::Right));
        e.handle(key(KeyCode::Enter));
        assert_eq!(e.text(), "hé\nllo");
        assert_eq!(e.cursor(), (1, 0));
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut e = Editor::new("ab\ncd");
        e.handle(key(KeyCode::Down));
        e.handle(key(KeyCode::Backspace));
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), (0, 2));
        e.handle(key(KeyCode::Enter));
        e.handle(key(KeyCode::Up));
        e.handle(key(KeyCode::End));
        e.handle(key(KeyCode::Delete));
        assert_eq!(e.text(), "abcd");
        e.handle(key(KeyCode::Backspace));
        assert_eq!(e.text(), "acd");
    }

    #[test]
    fn edges_are_safe() {
        let mut e = Editor::default();
        for code in [
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
        ] {
            e.handle(key(code));
        }
        assert_eq!((e.text(), e.cursor()), (String::new(), (0, 0)));
    }

    #[test]
    fn arrows_wrap_and_clamp() {
        let mut e = Editor::new("long line\nab");
        e.handle(key(KeyCode::End));
        e.handle(key(KeyCode::Down));
        assert_eq!(e.cursor(), (1, 2));
        e.handle(key(KeyCode::Home));
        e.handle(key(KeyCode::Left));
        assert_eq!(e.cursor(), (0, 9));
        e.handle(key(KeyCode::Right));
        assert_eq!(e.cursor(), (1, 0));
    }

    #[test]
    fn ctrl_chars_are_not_typed() {
        let mut e = Editor::default();
        e.handle(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "");
    }

    fn draw(e: &Editor, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(12, h)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), e, Block::bordered(), true))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| (0..12).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn view_scrolls_to_keep_cursor_visible() {
        let mut e = Editor::new("l0\nl1\nl2\nl3\nl4");
        for _ in 0..4 {
            e.handle(key(KeyCode::Down));
        }
        let out = draw(&e, 4);
        assert!(out.contains("l4") && out.contains("l3"), "{out}");
        assert!(!out.contains("l0"), "{out}");
        for _ in 0..4 {
            e.handle(key(KeyCode::Up));
        }
        let out = draw(&e, 4);
        assert!(out.contains("l0") && !out.contains("l4"), "{out}");
    }

    #[test]
    fn view_scrolls_sideways_on_long_lines() {
        let mut e = Editor::new("abcdefghijklmnopqrstuvwxyz");
        e.handle(key(KeyCode::End));
        let out = draw(&e, 3);
        assert!(out.contains("rstuvwxyz"), "{out}");
        assert!(!out.contains("abc"), "{out}");
    }
}
