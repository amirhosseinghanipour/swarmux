//! The change log: what happened, newest first. Informational only; nothing
//! here needs an answer.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use swarmux_application::{Change, ChangeKind};
use swarmux_domain::telemetry::local_clock;

use super::{glyph, panel, sigil};
use crate::app::App;
use crate::ui::dashboard::{dim, fit};
use crate::ui::look::Icon;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let meta = Line::from(dim(t, " FYI "));
    let block = panel(t, glyph(app, Icon::Changes), "changes", meta, t.border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 20 || inner.height == 0 {
        return;
    }
    let m = &app.mission;
    if m.changes.is_empty() {
        frame.render_widget(
            Line::from(dim(t, "nothing has changed yet")),
            Rect { height: 1, ..inner },
        );
        return;
    }
    let offset = m.snapshot.offset_min;
    let w = usize::from(inner.width);
    let lines: Vec<Line> = m
        .changes
        .iter()
        .take(usize::from(inner.height))
        .map(|c| row(app, c, offset, w))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn icon_of(kind: ChangeKind) -> Icon {
    match kind {
        ChangeKind::Started => Icon::StateRun,
        ChangeKind::Finished | ChangeKind::Reviewed => Icon::Result,
        ChangeKind::Failed | ChangeKind::Denied => Icon::Failed,
        ChangeKind::Asked => Icon::Question,
        ChangeKind::Stopped => Icon::Stalled,
        ChangeKind::Routed => Icon::Lead,
    }
}

/// `14:02 S ✓ demo  done $0.41`
fn row(app: &App, c: &Change, offset_min: i32, width: usize) -> Line<'static> {
    let t = &app.look.theme;
    let color = match c.kind {
        ChangeKind::Failed | ChangeKind::Denied | ChangeKind::Stopped => t.error,
        ChangeKind::Finished | ChangeKind::Reviewed => t.ok,
        _ => t.muted,
    };
    let head = format!("{} ", local_clock(c.at, offset_min));
    let name_w = 14;
    let used = head.chars().count() + 4 + name_w + 1;
    Line::from(vec![
        Span::styled(head, Style::new().fg(t.muted)),
        Span::styled(sigil(&c.session).to_string(), Style::new().fg(t.label)),
        Span::raw(" "),
        Span::styled(glyph(app, icon_of(c.kind)), Style::new().fg(color)),
        Span::styled(
            format!(" {:<name_w$} ", fit(&c.session, name_w)),
            Style::new().fg(t.text),
        ),
        Span::styled(
            fit(&c.text, width.saturating_sub(used)),
            Style::new().fg(t.muted),
        ),
    ])
}
