//! Panel chrome: rounded borders, a title chip with the panel's icon, the
//! focus highlight and the pulse when a panel's value changes.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType},
};

use super::{Panel, PanelId, ViewModel};
use crate::theme::Theme;
use crate::ui::widgets::pulse_color;

/// How a panel's frame looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    Rounded,
    /// For the silent reviewer.
    Dashed,
}

/// A block for `panel` in `accent`, titled with its chip followed by
/// `title` spans. `fingerprint` changes when the value shown changes, which
/// makes the border pulse briefly.
pub fn block(
    panel: &dyn Panel,
    vm: &ViewModel,
    theme: &Theme,
    accent: Color,
    title: Vec<Span<'static>>,
    fingerprint: Option<u64>,
) -> Block<'static> {
    framed(
        panel.id(),
        vm.icon(panel.icon()),
        vm,
        theme,
        accent,
        title,
        fingerprint,
        Border::Rounded,
    )
}

/// Like [`block`] with a choice of border.
#[allow(clippy::too_many_arguments)]
pub fn framed(
    id: PanelId,
    icon: &'static str,
    vm: &ViewModel,
    theme: &Theme,
    accent: Color,
    title: Vec<Span<'static>>,
    fingerprint: Option<u64>,
    border: Border,
) -> Block<'static> {
    let focused = vm.focus == Some(id);
    let pulse = fingerprint.map_or(0.0, |f| vm.pulse(id, f));
    let edge = if focused {
        theme.border_focus
    } else {
        pulse_color(accent, theme.title, pulse * 0.7)
    };
    let mut spans = vec![chip(icon, accent, theme, focused)];
    spans.extend(title);
    Block::bordered()
        .border_type(match border {
            Border::Rounded => BorderType::Rounded,
            Border::Dashed => BorderType::LightDoubleDashed,
        })
        .border_style(Style::new().fg(edge))
        .style(Style::new().bg(theme.panel_bg))
        .title(Line::from(spans))
}

/// ` icon ` before the title; inverted while the panel has focus.
pub fn chip(icon: &'static str, accent: Color, theme: &Theme, focused: bool) -> Span<'static> {
    let style = if focused {
        Style::new()
            .fg(theme.on_accent)
            .bg(theme.border_focus)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(accent).add_modifier(Modifier::BOLD)
    };
    Span::styled(format!(" {icon}"), style)
}

/// A bold title in `color`.
pub fn title(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// A cheap fingerprint of anything hashable, for pulses.
pub fn fingerprint(value: impl std::hash::Hash) -> u64 {
    use std::hash::{DefaultHasher, Hasher};
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
