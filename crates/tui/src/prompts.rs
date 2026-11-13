//! State of the prompts screen: the library list, the edit form and a
//! dispatch waiting for variable values.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::{PromptId, PromptTemplate, SessionId, parse_tags};

use crate::ui::editor::Editor;
use crate::ui::input::Input;

#[derive(Default)]
pub struct PromptsScreen {
    pub list: Vec<PromptTemplate>,
    pub selected: usize,
    pub form: Option<PromptForm>,
    /// `x` was pressed once; a second `x` deletes.
    pub confirm_delete: bool,
    /// The last key press found the delete armed (so it may delete).
    pub delete_armed: bool,
    pub pending: Option<PendingDispatch>,
}

impl PromptsScreen {
    pub fn selected_prompt(&self) -> Option<&PromptTemplate> {
        self.list.get(self.selected)
    }

    /// Replaces the list, selecting `select` if it is still there.
    pub fn set_list(&mut self, list: Vec<PromptTemplate>, select: Option<PromptId>) {
        self.list = list;
        if let Some(id) = select
            && let Some(i) = self.list.iter().position(|p| p.id == id)
        {
            self.selected = i;
        }
        self.selected = self.selected.min(self.list.len().saturating_sub(1));
    }

    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.selected + 1 < self.list.len() {
            self.selected += 1;
        }
    }
}

/// A template dispatch collecting user variables one at a time.
pub struct PendingDispatch {
    pub prompt_id: PromptId,
    pub session_id: SessionId,
    /// Still to ask, in order; the first one is being asked now.
    pub remaining: Vec<String>,
    pub values: HashMap<String, String>,
    /// Let swarmux pick the session instead of `session_id`.
    pub route: bool,
}

impl PendingDispatch {
    pub fn asking(&self) -> Option<&str> {
        self.remaining.first().map(String::as_str)
    }

    /// Stores the answer to the current question.
    pub fn answer(&mut self, value: String) {
        if !self.remaining.is_empty() {
            let name = self.remaining.remove(0);
            self.values.insert(name, value);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Tags,
    Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormAction {
    None,
    Save,
    Cancel,
}

/// Creating (`id` is `None`) or editing a prompt.
pub struct PromptForm {
    pub id: Option<PromptId>,
    pub name: Input,
    pub tags: Input,
    pub body: Editor,
    pub focus: Field,
}

impl PromptForm {
    pub fn blank() -> PromptForm {
        PromptForm {
            id: None,
            name: Input::default(),
            tags: Input::default(),
            body: Editor::default(),
            focus: Field::Name,
        }
    }

    pub fn edit(prompt: &PromptTemplate) -> PromptForm {
        PromptForm {
            id: Some(prompt.id),
            name: Input::with_text(&prompt.name),
            tags: Input::with_text(&prompt.tags.join(", ")),
            body: Editor::new(&prompt.body),
            focus: Field::Body,
        }
    }

    pub fn tag_list(&self) -> Vec<String> {
        parse_tags(&self.tags.text)
    }

    /// Every key goes to the form while it is open.
    pub fn handle(&mut self, key: KeyEvent) -> FormAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return FormAction::Cancel,
            KeyCode::Char('s') if ctrl => return FormAction::Save,
            KeyCode::Tab => self.focus = self.next_field(),
            KeyCode::BackTab => self.focus = self.prev_field(),
            KeyCode::Enter if self.focus != Field::Body => self.focus = self.next_field(),
            _ => match self.focus {
                // Enter and Esc are handled above, so the inputs never submit or cancel.
                Field::Name => {
                    self.name.handle(key);
                }
                Field::Tags => {
                    self.tags.handle(key);
                }
                Field::Body => self.body.handle(key),
            },
        }
        FormAction::None
    }

    fn next_field(&self) -> Field {
        match self.focus {
            Field::Name => Field::Tags,
            Field::Tags => Field::Body,
            Field::Body => Field::Name,
        }
    }

    fn prev_field(&self) -> Field {
        match self.focus {
            Field::Name => Field::Body,
            Field::Tags => Field::Name,
            Field::Body => Field::Tags,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_cycles_fields_and_enter_moves_on() {
        let mut f = PromptForm::blank();
        f.handle(key(KeyCode::Tab));
        assert_eq!(f.focus, Field::Tags);
        f.handle(key(KeyCode::Enter));
        assert_eq!(f.focus, Field::Body);
        f.handle(key(KeyCode::Enter));
        assert_eq!(f.focus, Field::Body);
        assert_eq!(f.body.text(), "\n");
        f.handle(key(KeyCode::Tab));
        assert_eq!(f.focus, Field::Name);
        f.handle(key(KeyCode::BackTab));
        assert_eq!(f.focus, Field::Body);
    }

    #[test]
    fn save_and_cancel_keys() {
        let mut f = PromptForm::blank();
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(f.handle(ctrl_s), FormAction::Save);
        assert_eq!(f.handle(key(KeyCode::Esc)), FormAction::Cancel);
        assert_eq!(f.handle(key(KeyCode::Char('s'))), FormAction::None);
        assert_eq!(f.name.text, "s");
    }

    #[test]
    fn pending_dispatch_answers_in_order() {
        let mut p = PendingDispatch {
            prompt_id: PromptId(1),
            session_id: SessionId::random(),
            remaining: vec!["a".into(), "b".into()],
            values: HashMap::new(),
            route: false,
        };
        assert_eq!(p.asking(), Some("a"));
        p.answer("1".into());
        assert_eq!(p.asking(), Some("b"));
        p.answer("2".into());
        assert_eq!(p.asking(), None);
        assert_eq!(p.values["a"], "1");
        assert_eq!(p.values["b"], "2");
    }
}
