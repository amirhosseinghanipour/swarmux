//! The action stream: a job's log lines as rows of a tool-call tree.

use std::collections::VecDeque;

/// What a diff line is, for tinting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Added,
    Removed,
    Hunk,
    Context,
}

impl DiffLine {
    /// Like [`DiffLine::of`], for prose that may use `- ` for bullets: a
    /// `+` or `-` must touch what follows it.
    pub fn of_text(line: &str) -> Option<DiffLine> {
        let mut chars = line.chars();
        match (chars.next()?, chars.next()) {
            ('+' | '-', Some(c)) if c.is_whitespace() => None,
            _ => DiffLine::of(line),
        }
    }

    /// Classifies a line of a unified diff. `None` for text that isn't one.
    pub fn of(line: &str) -> Option<DiffLine> {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
            return Some(DiffLine::Hunk);
        }
        match line.chars().next()? {
            '+' => Some(DiffLine::Added),
            '-' => Some(DiffLine::Removed),
            '@' if line.starts_with("@@") => Some(DiffLine::Hunk),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A tool call: `Edit src/app.py`.
    Call { tool: String, subject: String },
    /// What a call returned.
    Result { ok: bool },
    /// What the agent said.
    Say,
    /// A line that is part of a diff.
    Diff(DiffLine),
    /// The session start and the final report.
    Milestone,
    /// Anything else (swarmux's own notes, rate limits, todos).
    Note,
}

/// One row of the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// `01:25`.
    pub time: String,
    pub actor: String,
    /// 0 for the lead, 1 for what a subagent did.
    pub depth: u8,
    pub kind: RowKind,
    pub text: String,
    /// The last row of its call's branch (`└`, not `├`).
    pub last: bool,
}

/// Splits `01:25 lead       tool_use    Edit src/app.py`.
fn split(line: &str) -> Option<(&str, &str, &str, &str)> {
    let (time, rest) = line.split_once(' ')?;
    let rest = rest.trim_start();
    let (actor, rest) = rest.split_once(' ')?;
    let rest = rest.trim_start();
    let (kind, text) = rest.split_once(' ').unwrap_or((rest, ""));
    Some((time, actor, kind, text.trim_start()))
}

/// The rows of `log`, oldest first.
pub fn rows(log: &VecDeque<String>) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(log.len());
    for line in log {
        let Some((time, actor, kind, text)) = split(line) else {
            continue;
        };
        let depth = match actor {
            "lead" | "swarmux" | "reviewer" => 0,
            _ => 1,
        };
        let kind = match kind {
            "tool_use" => {
                let (tool, subject) = text.split_once(' ').unwrap_or((text, ""));
                RowKind::Call {
                    tool: tool.to_string(),
                    subject: subject.trim().to_string(),
                }
            }
            "tool_result" => RowKind::Result {
                ok: !text.starts_with("ERROR"),
            },
            "text" => match DiffLine::of_text(text) {
                Some(d) => RowKind::Diff(d),
                None => RowKind::Say,
            },
            "init" | "result" | "cancelled" | "error" => RowKind::Milestone,
            _ => RowKind::Note,
        };
        let text = match &kind {
            RowKind::Result { ok: false } => text.trim_start_matches("ERROR").trim_start(),
            RowKind::Call { .. } => "",
            _ => text,
        };
        out.push(Row {
            time: time.to_string(),
            actor: actor.to_string(),
            depth,
            kind,
            text: text.to_string(),
            last: true,
        });
    }
    // A result closes the branch of the call before it: that call's earlier
    // results are not last.
    for i in 1..out.len() {
        if matches!(out[i].kind, RowKind::Result { .. })
            && matches!(out[i - 1].kind, RowKind::Result { .. })
            && out[i - 1].actor == out[i].actor
        {
            out[i - 1].last = false;
        }
    }
    out
}

/// A multi-line text as rows of diff or plain lines (for the diff pane).
pub fn diff_lines(text: &str) -> Vec<(DiffLine, &str)> {
    text.lines()
        .map(|l| (DiffLine::of(l).unwrap_or(DiffLine::Context), l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[&str]) -> VecDeque<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn calls_results_and_subagents() {
        let rows = rows(&log(&[
            "00:01 lead       tool_use    Edit /w/src/app.py",
            "00:02 lead       tool_result ok",
            "00:03 worker     tool_use    Bash pytest -q",
            "00:04 worker     tool_result ERROR 2 failed",
            "00:05 lead       text        +added line",
            "00:06 lead       result      $0.41 · 9 turns",
        ]));
        assert_eq!(rows.len(), 6);
        assert_eq!(
            rows[0].kind,
            RowKind::Call {
                tool: "Edit".into(),
                subject: "/w/src/app.py".into()
            }
        );
        assert_eq!(rows[1].kind, RowKind::Result { ok: true });
        assert_eq!((rows[2].depth, rows[2].actor.as_str()), (1, "worker"));
        assert_eq!(rows[3].kind, RowKind::Result { ok: false });
        assert_eq!(rows[3].text, "2 failed");
        assert_eq!(rows[4].kind, RowKind::Diff(DiffLine::Added));
        assert_eq!(rows[5].kind, RowKind::Milestone);
    }

    #[test]
    fn consecutive_results_share_a_branch() {
        let rows = rows(&log(&[
            "00:01 lead       tool_use    Task a",
            "00:02 lead       tool_result one",
            "00:03 lead       tool_result two",
        ]));
        assert!(!rows[1].last);
        assert!(rows[2].last);
    }

    #[test]
    fn odd_lines_never_panic() {
        let rows = rows(&log(&["", "garbage", "00:01", "00:01 lead", "100:05 a b"]));
        assert!(rows.len() <= 3);
    }

    #[test]
    fn diff_lines_by_first_character() {
        assert_eq!(DiffLine::of("+a"), Some(DiffLine::Added));
        assert_eq!(DiffLine::of("-a"), Some(DiffLine::Removed));
        assert_eq!(DiffLine::of("@@ -1 +1 @@"), Some(DiffLine::Hunk));
        assert_eq!(DiffLine::of("+++ b/x"), Some(DiffLine::Hunk));
        assert_eq!(DiffLine::of("plain"), None);
        assert_eq!(DiffLine::of(""), None);
        assert_eq!(DiffLine::of_text("- a bullet"), None);
        assert_eq!(DiffLine::of_text("-removed"), Some(DiffLine::Removed));
    }
}
