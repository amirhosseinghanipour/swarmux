use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::DefaultTerminal;

/// The five top-level screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Dashboard,
    Prompts,
    Queue,
    History,
    Sessions,
}

impl Screen {
    pub const ALL: [Screen; 5] = [
        Screen::Dashboard,
        Screen::Prompts,
        Screen::Queue,
        Screen::History,
        Screen::Sessions,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Dashboard => "dashboard",
            Screen::Prompts => "prompts",
            Screen::Queue => "queue",
            Screen::History => "history",
            Screen::Sessions => "sessions",
        }
    }
}

#[derive(Default)]
pub struct App {
    pub screen: Screen,
    pub quit: bool,
}

impl App {
    pub fn run(mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            terminal.draw(|frame| crate::ui::draw(frame, &self))?;
            // ~10 fps redraw budget; keys are handled as soon as they arrive.
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key.code);
            }
        }
        Ok(())
    }

    pub fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char(c @ '1'..='5') => {
                self.screen = Screen::ALL[(c as u8 - b'1') as usize];
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_keys_switch_screens() {
        let mut app = App::default();
        app.on_key(KeyCode::Char('3'));
        assert_eq!(app.screen, Screen::Queue);
        app.on_key(KeyCode::Char('1'));
        assert_eq!(app.screen, Screen::Dashboard);
    }

    #[test]
    fn q_quits() {
        let mut app = App::default();
        app.on_key(KeyCode::Char('q'));
        assert!(app.quit);
    }
}
