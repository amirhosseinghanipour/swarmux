//! One-line text input (UTF-8 aware; cursor counts chars, not bytes).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputAction {
    None,
    Submit(String),
    Cancel,
}

impl Input {
    /// Starts with `text` and the cursor at its end.
    pub fn with_text(text: &str) -> Input {
        Input {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    pub fn handle(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Enter => return InputAction::Submit(self.text.clone()),
            KeyCode::Esc => return InputAction::Cancel,
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => {}
        }
        InputAction::None
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Text split around the cursor: (before, at-cursor char or " ", after).
    pub fn split_at_cursor(&self) -> (String, String, String) {
        let before: String = self.text.chars().take(self.cursor).collect();
        let mut rest = self.text.chars().skip(self.cursor);
        let at = rest.next().map_or(" ".to_string(), String::from);
        (before, at, rest.collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(input: &mut Input, s: &str) {
        for c in s.chars() {
            input.handle(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_utf8_and_backspace() {
        let mut i = Input::default();
        type_str(&mut i, "né🐝");
        i.handle(key(KeyCode::Backspace));
        assert_eq!(i.text, "né");
        assert_eq!(i.cursor, 2);
    }

    #[test]
    fn editing_in_the_middle() {
        let mut i = Input::default();
        type_str(&mut i, "ac");
        i.handle(key(KeyCode::Left));
        type_str(&mut i, "b");
        assert_eq!(i.text, "abc");
        i.handle(key(KeyCode::Home));
        i.handle(key(KeyCode::Delete));
        assert_eq!(i.text, "bc");
        assert_eq!(i.split_at_cursor(), ("".into(), "b".into(), "c".into()));
    }

    #[test]
    fn enter_submits_and_esc_cancels() {
        let mut i = Input::default();
        type_str(&mut i, "hi");
        assert_eq!(
            i.handle(key(KeyCode::Enter)),
            InputAction::Submit("hi".into())
        );
        assert_eq!(i.handle(key(KeyCode::Esc)), InputAction::Cancel);
    }
}
