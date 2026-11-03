//! Small pieces the Focus panels share: tones, meters, wrapping and the
//! markdown-ish styling of the answer.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::app::App;
use crate::focus::stream::DiffLine;
use crate::mission::Tone;
use crate::theme::{Theme, mix};
use crate::ui::look::{Icon, IconSet};

/// The glyph and colour of a chip's tone.
pub fn tone(t: &Theme, tone: Tone) -> (Icon, Color) {
    match tone {
        Tone::Good => (Icon::Result, t.ok),
        Tone::Warn => (Icon::Concerns, t.warn),
        Tone::Bad => (Icon::Failed, t.error),
        Tone::Info => (Icon::StateRun, t.muted),
    }
}

/// `▰▰▰▱▱`, or `###--` in plain ASCII.
pub fn meter(app: &App, filled: usize, total: usize) -> String {
    let (on, off) = if app.look.icons == IconSet::Ascii {
        ('#', '-')
    } else {
        ('▰', '▱')
    };
    (0..total)
        .map(|i| if i < filled { on } else { off })
        .collect()
}

/// The background a line of diff gets: a faint wash of its colour.
pub fn wash(t: &Theme, color: Color) -> Color {
    mix(t.panel_bg, color, 0.18)
}

/// A diff line, tinted: green and red washes, the hunk header in the label
/// colour. The `+`/`-` stays in the text, so colour is never the only cue.
pub fn diff_style(t: &Theme, kind: DiffLine) -> Style {
    match kind {
        DiffLine::Added => Style::new().fg(t.ok).bg(wash(t, t.ok)),
        DiffLine::Removed => Style::new().fg(t.error).bg(wash(t, t.error)),
        DiffLine::Hunk => Style::new().fg(t.label).add_modifier(Modifier::BOLD),
        DiffLine::Context => Style::new().fg(t.muted),
    }
}

fn cell_width(c: char) -> usize {
    if c.is_ascii() {
        1
    } else {
        Span::raw(c.to_string()).width().max(1)
    }
}

/// Wraps `(text, style)` runs to `width` cells, breaking at spaces where
/// it can. Never returns an empty list.
pub fn wrap(runs: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let cells: Vec<(char, Style)> = runs
        .iter()
        .flat_map(|(s, st)| s.chars().map(move |c| (c, *st)))
        .collect();
    let mut lines: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut used = 0;
    for (c, st) in cells {
        let w = cell_width(c);
        if used + w > width {
            let split = cur
                .iter()
                .rposition(|(c, _)| *c == ' ')
                .filter(|i| *i > 0 && c != ' ');
            match split {
                Some(i) => {
                    let rest = cur.split_off(i + 1);
                    lines.push(std::mem::take(&mut cur));
                    cur = rest;
                }
                None => lines.push(std::mem::take(&mut cur)),
            }
            used = cur.iter().map(|(c, _)| cell_width(*c)).sum();
            if c == ' ' && cur.is_empty() {
                continue;
            }
        }
        used += w;
        cur.push((c, st));
    }
    lines.push(cur);
    lines
        .into_iter()
        .map(|cells| {
            let mut spans: Vec<Span> = Vec::new();
            let mut text = String::new();
            let mut style = None;
            for (c, st) in cells {
                if style.is_some_and(|s| s != st) {
                    spans.push(Span::styled(
                        std::mem::take(&mut text),
                        style.unwrap_or_default(),
                    ));
                }
                style = Some(st);
                text.push(c);
            }
            if let Some(s) = style {
                spans.push(Span::styled(text, s));
            }
            Line::from(spans)
        })
        .collect()
}

/// Runs of `text` split at backticks (code) and double asterisks (bold).
fn inline(t: &Theme, text: &str, base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut code) = (false, false);
    let style = |bold: bool, code: bool| {
        let mut s = base;
        if bold {
            s = s.add_modifier(Modifier::BOLD).fg(t.title);
        }
        if code {
            s = s.fg(t.label);
        }
        s
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let is_code = c == '`';
        let is_bold = c == '*' && !code && chars.peek() == Some(&'*');
        if !is_code && !is_bold {
            cur.push(c);
            continue;
        }
        if is_bold {
            chars.next();
        }
        if !cur.is_empty() {
            out.push((std::mem::take(&mut cur), style(bold, code)));
        }
        if is_code {
            code = !code;
        } else {
            bold = !bold;
        }
    }
    if !cur.is_empty() {
        out.push((cur, style(bold, code)));
    }
    out
}

/// The answer as styled, wrapped lines: headings, bullets, quotes, inline
/// code and bold, and fenced code with diffs tinted.
pub fn markdown(t: &Theme, text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_code = false;
    let plain = Style::new().fg(t.text);
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            let lang = trimmed.trim_start_matches('`').trim();
            if in_code {
                let tag = if lang.is_empty() { "code" } else { lang };
                out.push(Line::styled(format!("╌╌ {tag} "), Style::new().fg(t.dim)));
            }
            continue;
        }
        if in_code {
            let style = match DiffLine::of(raw) {
                Some(d) => diff_style(t, d),
                None => Style::new().fg(t.text).bg(mix(t.panel_bg, t.text, 0.06)),
            };
            out.extend(wrap(&[(raw.to_string(), style)], width));
            continue;
        }
        let (text, base) = if let Some(h) = trimmed.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim_start();
            (
                h.to_string(),
                Style::new().fg(t.title).add_modifier(Modifier::BOLD),
            )
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let indent = " ".repeat(raw.len() - trimmed.len());
            (format!("{indent}• {rest}"), plain)
        } else if let Some(q) = trimmed.strip_prefix('>') {
            (
                format!("│ {}", q.trim_start()),
                Style::new().fg(t.muted).add_modifier(Modifier::ITALIC),
            )
        } else {
            (raw.to_string(), plain)
        };
        out.extend(wrap(&inline(t, &text, base), width));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_hard_breaks_long_words() {
        let run = [("alpha beta gamma".to_string(), Style::new())];
        assert_eq!(text(&wrap(&run, 10)), ["alpha beta", "gamma"]);
        let long = [("abcdefghij".to_string(), Style::new())];
        assert_eq!(text(&wrap(&long, 4)), ["abcd", "efgh", "ij"]);
        assert_eq!(text(&wrap(&[], 4)), [""]);
    }

    #[test]
    fn wrapping_keeps_styles_apart() {
        let a = Style::new().fg(Color::Red);
        let runs = [("ab".to_string(), a), ("cd".to_string(), Style::new())];
        let lines = wrap(&runs, 10);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[0].style, a);
    }

    #[test]
    fn markdown_styles_headings_bullets_and_fences() {
        let t = Theme::default();
        let md = "# Done\n- one **bold** and `code`\n```diff\n+added\n-gone\n```\nplain";
        let lines = markdown(&t, md, 40);
        let s = text(&lines);
        assert_eq!(s[0], "Done");
        assert_eq!(s[1], "• one bold and code");
        assert_eq!(s[2], "╌╌ diff ");
        assert_eq!(s[3], "+added");
        assert_eq!(lines[3].spans[0].style.fg, Some(t.ok));
        assert_eq!(lines[4].spans[0].style.fg, Some(t.error));
        assert_eq!(s[5], "plain");
    }

    #[test]
    fn inline_markers_do_not_leak_into_text() {
        let t = Theme::default();
        let runs = inline(&t, "a **b** `c` d", Style::new());
        let joined: String = runs.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "a b c d");
        assert_eq!(inline(&t, "**open", Style::new()).len(), 1);
    }

    #[test]
    fn meters_have_an_ascii_form() {
        let mut app = crate::app::tests_support::test_app().app;
        app.look.icons = IconSet::Unicode;
        assert_eq!(meter(&app, 2, 5), "▰▰▱▱▱");
        app.look.icons = IconSet::Ascii;
        assert_eq!(meter(&app, 2, 5), "##---");
    }
}
