//! Tool calls: how often and how long each tool runs, a ticker of recent
//! calls, and the files agents read and edit.

use std::collections::{HashMap, VecDeque};

use crate::Millis;

/// Tools whose summary is the file they read.
const READ_TOOLS: &[&str] = &["Read", "NotebookRead"];
/// Tools whose summary is the file they change.
const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// One tool call, open until its result arrives.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// What it acts on (command, path, pattern …), shortened.
    pub summary: String,
    /// `lead` or the subagent's tool_use id.
    pub actor: String,
    /// The swarmux job, for calls tallied across jobs.
    pub job: Option<i64>,
    pub started_at: Millis,
    pub ended_at: Option<Millis>,
    pub is_error: bool,
}

impl ToolCall {
    pub fn duration(&self) -> Option<Millis> {
        self.ended_at.map(|e| (e - self.started_at).max(0))
    }
}

/// Calls of one tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolStat {
    pub name: String,
    pub calls: u32,
    pub errors: u32,
    /// Mean time to a result over finished calls.
    pub avg_ms: Option<Millis>,
}

/// How often a file was read and changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTouch {
    pub path: String,
    pub reads: u32,
    pub edits: u32,
    pub last_at: Millis,
}

/// Recent tool calls (capped) and file touches.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolLedger {
    calls: VecDeque<ToolCall>,
    cap: usize,
    files: HashMap<String, FileTouch>,
}

impl ToolLedger {
    /// Keeps at most `cap` calls (oldest drop first).
    pub fn new(cap: usize) -> ToolLedger {
        ToolLedger {
            calls: VecDeque::new(),
            cap: cap.max(1),
            files: HashMap::new(),
        }
    }

    pub fn started(&mut self, call: ToolCall) {
        let path = call.summary.trim();
        let kind = (
            READ_TOOLS.contains(&call.name.as_str()),
            EDIT_TOOLS.contains(&call.name.as_str()),
        );
        if (kind.0 || kind.1) && !path.is_empty() && path != call.name {
            let f = self
                .files
                .entry(path.to_string())
                .or_insert_with(|| FileTouch {
                    path: path.to_string(),
                    reads: 0,
                    edits: 0,
                    last_at: call.started_at,
                });
            f.reads += u32::from(kind.0);
            f.edits += u32::from(kind.1);
            f.last_at = f.last_at.max(call.started_at);
        }
        self.calls.push_back(call);
        while self.calls.len() > self.cap {
            self.calls.pop_front();
        }
    }

    /// Closes the call `id`. Unknown ids are ignored.
    pub fn finished(&mut self, id: &str, is_error: bool, at: Millis) {
        if let Some(c) = self
            .calls
            .iter_mut()
            .rev()
            .find(|c| c.id == id && c.ended_at.is_none())
        {
            c.ended_at = Some(at.max(c.started_at));
            c.is_error = is_error;
        }
    }

    /// The newest unfinished call of `actor` (in `job`, when given).
    pub fn current(&self, actor: &str, job: Option<i64>) -> Option<&ToolCall> {
        self.calls
            .iter()
            .rev()
            .find(|c| c.actor == actor && c.ended_at.is_none() && (job.is_none() || c.job == job))
    }

    /// The newest `n` calls, newest first.
    pub fn ticker(&self, n: usize) -> Vec<ToolCall> {
        self.calls.iter().rev().take(n).cloned().collect()
    }

    /// Calls of `actor` among those kept.
    pub fn calls_by(&self, actor: &str) -> usize {
        self.calls.iter().filter(|c| c.actor == actor).count()
    }

    /// Per-tool counts over calls started at or after `since` (all kept calls
    /// when `None`), most used first.
    pub fn stats(&self, since: Option<Millis>) -> Vec<ToolStat> {
        let mut acc: Vec<(String, u32, u32, Millis, u32)> = Vec::new();
        for c in self
            .calls
            .iter()
            .filter(|c| since.is_none_or(|s| c.started_at >= s))
        {
            let i = match acc.iter().position(|a| a.0 == c.name) {
                Some(i) => i,
                None => {
                    acc.push((c.name.clone(), 0, 0, 0, 0));
                    acc.len() - 1
                }
            };
            let a = &mut acc[i];
            a.1 += 1;
            a.2 += u32::from(c.is_error);
            if let Some(d) = c.duration() {
                a.3 += d;
                a.4 += 1;
            }
        }
        let mut out: Vec<ToolStat> = acc
            .into_iter()
            .map(|(name, calls, errors, total, done)| ToolStat {
                name,
                calls,
                errors,
                avg_ms: (done > 0).then(|| total / Millis::from(done)),
            })
            .collect();
        out.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// The `n` most touched files: most reads + edits first, then most edits,
    /// then most recent.
    pub fn top_files(&self, n: usize) -> Vec<FileTouch> {
        let mut files: Vec<FileTouch> = self.files.values().cloned().collect();
        files.sort_by(|a, b| {
            (b.reads + b.edits)
                .cmp(&(a.reads + a.edits))
                .then(b.edits.cmp(&a.edits))
                .then(b.last_at.cmp(&a.last_at))
                .then_with(|| a.path.cmp(&b.path))
        });
        files.truncate(n);
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, summary: &str, at: Millis) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            summary: summary.into(),
            actor: "lead".into(),
            job: Some(1),
            started_at: at,
            ended_at: None,
            is_error: false,
        }
    }

    #[test]
    fn counts_and_average_durations() {
        let mut l = ToolLedger::new(100);
        l.started(call("a", "Bash", "ls", 0));
        l.started(call("b", "Bash", "cargo test", 100));
        l.started(call("c", "Read", "src/a.rs", 200));
        l.finished("a", false, 300);
        l.finished("b", true, 600);
        l.finished("zzz", false, 700);
        let s = l.stats(None);
        assert_eq!(s[0].name, "Bash");
        assert_eq!((s[0].calls, s[0].errors, s[0].avg_ms), (2, 1, Some(400)));
        assert_eq!((s[1].name.as_str(), s[1].avg_ms), ("Read", None));
        assert_eq!(l.stats(Some(150)).len(), 1);
        assert_eq!(l.current("lead", Some(1)).map(|c| c.id.as_str()), Some("c"));
        assert_eq!(l.current("lead", Some(2)), None);
    }

    #[test]
    fn ticker_is_newest_first_and_capped() {
        let mut l = ToolLedger::new(2);
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            l.started(call(id, "Grep", "x", i as Millis));
        }
        let ids: Vec<String> = l.ticker(5).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["c", "b"]);
        assert_eq!(l.calls_by("lead"), 2);
    }

    #[test]
    fn files_rank_by_touches_then_edits() {
        let mut l = ToolLedger::new(100);
        l.started(call("1", "Read", "a.rs", 1));
        l.started(call("2", "Read", "a.rs", 2));
        l.started(call("3", "Edit", "b.rs", 3));
        l.started(call("4", "MultiEdit", "b.rs", 4));
        l.started(call("5", "Read", "c.rs", 5));
        l.started(call("6", "Bash", "cat d.rs", 6));
        l.started(call("7", "Write", "Write", 7));
        let top = l.top_files(10);
        let order: Vec<&str> = top.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["b.rs", "a.rs", "c.rs"]);
        assert_eq!((top[0].reads, top[0].edits, top[0].last_at), (0, 2, 4));
        assert_eq!(l.top_files(1).len(), 1);
    }
}
