use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};
use swarmux_application::{BUILTIN_VARIABLES, PromptService};
use swarmux_domain::{PromptTemplate, variables};

use super::input::Input;
use super::{MUTED, ORANGE, editor};
use crate::app::App;
use crate::prompts::{Field, PromptForm};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [list_area, right] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area);
    render_list(frame, list_area, app);
    match (&app.prompts.form, app.prompts.selected_prompt()) {
        (Some(form), _) => render_form(frame, right, form),
        (None, Some(prompt)) => render_preview(frame, right, app, prompt),
        (None, None) => {}
    }
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(MUTED))
        .title(format!(" {title} "))
}

fn render_list(frame: &mut Frame, area: Rect, app: &App) {
    if app.prompts.list.is_empty() {
        frame.render_widget(
            Paragraph::new("no prompts yet: press n to write one")
                .fg(MUTED)
                .wrap(Wrap { trim: true })
                .block(panel("prompts")),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .prompts
        .list
        .iter()
        .map(|p| {
            let mut spans = vec![Span::raw(p.name.clone())];
            if !p.tags.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", p.tags.join(", ")),
                    Style::new().fg(MUTED),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(panel("prompts"))
        .highlight_symbol("› ")
        .highlight_style(Style::new().bold());
    let mut state = ListState::default().with_selected(Some(app.prompts.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_preview(frame: &mut Frame, area: Rect, app: &App, prompt: &PromptTemplate) {
    let target = app
        .selected_session()
        .map_or("no session", |s| s.name.as_str());
    let block = panel(&format!("{} → {target}", prompt.name));
    let mut lines = vec![variables_line(&prompt.body), Line::raw("")];
    lines.extend(prompt.body.lines().map(Line::raw));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn variables_line(body: &str) -> Line<'static> {
    let user = PromptService::user_variables(body);
    let builtin: Vec<String> = variables(body)
        .into_iter()
        .filter(|v| BUILTIN_VARIABLES.contains(&v.as_str()))
        .collect();
    let mut parts = Vec::new();
    if !user.is_empty() {
        parts.push(format!("asks for: {}", user.join(", ")));
    }
    if !builtin.is_empty() {
        parts.push(format!("fills in: {}", builtin.join(", ")));
    }
    if parts.is_empty() {
        parts.push("no variables".into());
    }
    Line::styled(parts.join(" · "), Style::new().fg(MUTED))
}

fn render_form(frame: &mut Frame, area: Rect, form: &PromptForm) {
    let title = if form.id.is_some() {
        "edit prompt"
    } else {
        "new prompt"
    };
    let outer = panel(title);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let [name, tags, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(
        field_line("name", &form.name, form.focus == Field::Name),
        name,
    );
    frame.render_widget(
        field_line("tags", &form.tags, form.focus == Field::Tags),
        tags,
    );
    let focused = form.focus == Field::Body;
    let border = if focused { ORANGE } else { MUTED };
    let block = Block::bordered()
        .border_style(Style::new().fg(border))
        .title(" body · {{ variable }} ");
    editor::render(frame, body, &form.body, block, focused);
}

fn field_line(label: &str, input: &Input, focused: bool) -> Line<'static> {
    let label_style = if focused {
        Style::new().fg(ORANGE)
    } else {
        Style::new().fg(MUTED)
    };
    let label = Span::styled(format!("{label:<6}"), label_style);
    if !focused {
        return Line::from(vec![label, Span::raw(input.text.clone())]);
    }
    let (before, at, after) = input.split_at_cursor();
    Line::from(vec![
        label,
        Span::raw(before),
        Span::styled(at, Style::new().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ])
}

#[cfg(test)]
mod tests {
    use crate::app::Screen;
    use crate::app::tests_support::test_app;
    use crate::prompts::PromptForm;
    use crate::ui::render_to_string;

    #[test]
    fn empty_library_hint() {
        let mut t = test_app();
        t.app.screen = Screen::Prompts;
        let out = render_to_string(&t.app, 100, 12);
        assert!(out.contains("no prompts yet"), "{out}");
        assert!(out.contains("d dispatch"), "{out}");
    }

    fn app_with_prompt() -> crate::app::tests_support::TestApp {
        let mut t = test_app();
        t.app.screen = Screen::Prompts;
        t.add_prompt("review", "review {{file}}\nsince {{date}}", &["rust", "ai"]);
        t
    }

    #[test]
    fn list_and_preview() {
        let t = app_with_prompt();
        let out = render_to_string(&t.app, 100, 12);
        assert!(out.contains("review  rust, ai"), "{out}");
        assert!(out.contains("review → no session"), "{out}");
        assert!(out.contains("asks for: file · fills in: date"), "{out}");
        assert!(out.contains("since {{date}}"), "{out}");
    }

    #[test]
    fn edit_form_shows_fields() {
        let mut t = app_with_prompt();
        let p = t.app.prompts.selected_prompt().unwrap().clone();
        t.app.prompts.form = Some(PromptForm::edit(&p));
        let out = render_to_string(&t.app, 100, 12);
        assert!(out.contains("edit prompt"), "{out}");
        assert!(out.contains("name  review"), "{out}");
        assert!(out.contains("tags  rust, ai"), "{out}");
        assert!(out.contains("ctrl+s save"), "{out}");
    }

    #[test]
    fn delete_confirmation_hint() {
        let mut t = app_with_prompt();
        t.app.prompts.confirm_delete = true;
        let out = render_to_string(&t.app, 100, 12);
        assert!(out.contains("press x again to delete \"review\""), "{out}");
    }
}
