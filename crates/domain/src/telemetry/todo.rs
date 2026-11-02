//! The todo list an agent keeps with the `TodoWrite` tool.

use crate::json::Json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn parse(s: &str) -> TodoStatus {
        match s {
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
    /// What the agent shows while doing it ("Running the tests").
    pub active_form: Option<String>,
}

/// Todos of a `TodoWrite` call, from its input as JSON. `None` when the input
/// has no `todos` list.
pub fn parse_todos(input_json: &str) -> Option<Vec<Todo>> {
    let input = Json::parse(input_json).ok()?;
    let items = input.get("todos")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|t| {
                let content = t.get("content").and_then(Json::as_str)?.trim();
                Some(Todo {
                    content: content.to_string(),
                    status: TodoStatus::parse(
                        t.get("status").and_then(Json::as_str).unwrap_or_default(),
                    ),
                    active_form: t
                        .get("activeForm")
                        .and_then(Json::as_str)
                        .map(str::to_string),
                })
            })
            .filter(|t| !t.content.is_empty())
            .collect(),
    )
}

/// (completed, in progress, pending).
pub fn todo_counts(todos: &[Todo]) -> (usize, usize, usize) {
    let n = |s: TodoStatus| todos.iter().filter(|t| t.status == s).count();
    (
        n(TodoStatus::Completed),
        n(TodoStatus::InProgress),
        n(TodoStatus::Pending),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_status_and_active_form() {
        let input = r#"{"todos":[
            {"content":"Write tests","status":"completed","activeForm":"Writing tests"},
            {"content":"Fix bug","status":"in_progress","activeForm":"Fixing bug"},
            {"content":"Ship","status":"pending"},
            {"content":"  ","status":"pending"},
            {"status":"pending"},
            {"content":"Odd","status":"later"}
        ]}"#;
        let todos = parse_todos(input).unwrap();
        assert_eq!(todos.len(), 4);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        assert_eq!(todos[1].active_form.as_deref(), Some("Fixing bug"));
        assert_eq!(todos[2].active_form, None);
        assert_eq!(todos[3].status, TodoStatus::Pending);
        assert_eq!(todo_counts(&todos), (1, 1, 2));
    }

    #[test]
    fn no_list_is_none() {
        assert_eq!(parse_todos(r#"{"x":1}"#), None);
        assert_eq!(parse_todos("not json"), None);
        assert_eq!(parse_todos(r#"{"todos":[]}"#), Some(vec![]));
        assert_eq!(TodoStatus::parse("completed").as_str(), "completed");
    }
}
