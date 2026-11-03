//! Things that need the user: what they are, how urgent and in which order
//! they are shown. Pure rules; collecting them is the application's job.

use std::fmt;

use crate::guard::{BUILTIN_DENY, Pattern};
use crate::{JobId, Millis, SessionId};

/// A stable key for an item, so selection and acknowledgement survive
/// reordering: `approval:<request_id>`, `job:<id>:failed`, …
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttentionId(String);

impl AttentionId {
    pub fn new(key: impl Into<String>) -> AttentionId {
        AttentionId(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AttentionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an item is in the inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttentionKind {
    /// A tool call is held for the user's answer.
    Approval,
    /// An agent asked the user something.
    Question,
    JobFailed,
    /// A reviewer found concerns in a finished job.
    ReviewConcerns,
    /// A job finished and its result has not been looked at.
    ResultReady,
    /// A running job has been silent for too long.
    Stalled,
}

impl AttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionKind::Approval => "approval",
            AttentionKind::Question => "question",
            AttentionKind::JobFailed => "failed",
            AttentionKind::ReviewConcerns => "concerns",
            AttentionKind::ResultReady => "result",
            AttentionKind::Stalled => "stalled",
        }
    }

    /// Items the user answers with `y`/`n`/`a` rather than `x`.
    pub fn is_decision(self) -> bool {
        matches!(self, AttentionKind::Approval)
    }
}

/// How soon the user should look. `P1` sorts first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// Irreversible or risky: look now.
    P1,
    /// Something is blocked or broken.
    P2,
    /// Worth a look when convenient.
    P3,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }
}

/// How sure swarmux is that the item is a real problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Likelihood {
    Possible,
    Likely,
    Certain,
}

impl Likelihood {
    pub fn as_str(self) -> &'static str {
        match self {
            Likelihood::Possible => "possible",
            Likelihood::Likely => "likely",
            Likelihood::Certain => "certain",
        }
    }
}

/// The silence after which a running job counts as stalled.
pub const STALL_AFTER_MS: Millis = 10 * 60 * 1000;

/// One entry of the attention inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct AttentionItem {
    pub id: AttentionId,
    pub kind: AttentionKind,
    pub priority: Priority,
    pub title: String,
    pub detail: String,
    pub session: Option<SessionId>,
    pub job: Option<JobId>,
    /// The held tool call, for approvals.
    pub request: Option<u64>,
    /// When it started needing the user.
    pub since: Millis,
    /// When it resolves without the user (an approval times out).
    pub deadline: Option<Millis>,
    pub likelihood: Likelihood,
}

impl AttentionItem {
    /// A held tool call. Destructive calls are P1, the rest P2.
    pub fn approval(call: &HeldCall<'_>, since: Millis, deadline: Millis) -> AttentionItem {
        let priority = if is_destructive(call.tool, call.subject, call.cwd) {
            Priority::P1
        } else {
            Priority::P2
        };
        AttentionItem {
            id: AttentionId::new(format!("approval:{}", call.request_id)),
            kind: AttentionKind::Approval,
            priority,
            title: format!("{} {}", call.tool, call.subject).trim().to_string(),
            detail: "waiting for your answer".into(),
            session: call.session,
            job: call.job,
            request: Some(call.request_id),
            since,
            deadline: Some(deadline),
            likelihood: Likelihood::Certain,
        }
    }

    /// A job that failed (P2).
    pub fn job_failed(
        job: JobId,
        session: SessionId,
        title: &str,
        error: &str,
        since: Millis,
    ) -> AttentionItem {
        AttentionItem::of_job(
            job,
            session,
            AttentionKind::JobFailed,
            Priority::P2,
            Likelihood::Certain,
            title,
            error,
            since,
        )
    }

    /// A finished review with concerns (P3).
    pub fn review_concerns(
        job: JobId,
        session: SessionId,
        title: &str,
        concerns: &[String],
        since: Millis,
    ) -> AttentionItem {
        let detail = match concerns {
            [] => "the reviewer has concerns".to_string(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
        };
        AttentionItem::of_job(
            job,
            session,
            AttentionKind::ReviewConcerns,
            Priority::P3,
            Likelihood::Likely,
            title,
            &detail,
            since,
        )
    }

    /// A finished job whose result waits to be read (P3).
    pub fn result_ready(
        job: JobId,
        session: SessionId,
        title: &str,
        detail: &str,
        since: Millis,
    ) -> AttentionItem {
        AttentionItem::of_job(
            job,
            session,
            AttentionKind::ResultReady,
            Priority::P3,
            Likelihood::Certain,
            title,
            detail,
            since,
        )
    }

    /// A running job with no event since `since` (P3).
    pub fn stalled(job: JobId, session: SessionId, title: &str, since: Millis) -> AttentionItem {
        AttentionItem::of_job(
            job,
            session,
            AttentionKind::Stalled,
            Priority::P3,
            Likelihood::Possible,
            title,
            "no events for 10 minutes",
            since,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn of_job(
        job: JobId,
        session: SessionId,
        kind: AttentionKind,
        priority: Priority,
        likelihood: Likelihood,
        title: &str,
        detail: &str,
        since: Millis,
    ) -> AttentionItem {
        let suffix = match kind {
            AttentionKind::JobFailed => "failed",
            AttentionKind::ReviewConcerns => "review",
            AttentionKind::ResultReady => "result",
            AttentionKind::Stalled => "stalled",
            AttentionKind::Approval => "approval",
            AttentionKind::Question => "question",
        };
        AttentionItem {
            id: AttentionId::new(format!("job:{job}:{suffix}")),
            kind,
            priority,
            title: title.to_string(),
            detail: detail.to_string(),
            session: Some(session),
            job: Some(job),
            request: None,
            since,
            deadline: None,
            likelihood,
        }
    }
}

/// A held tool call, as far as ranking needs to know it.
#[derive(Clone, Copy, Debug)]
pub struct HeldCall<'a> {
    pub request_id: u64,
    pub job: Option<JobId>,
    pub session: Option<SessionId>,
    pub tool: &'a str,
    pub subject: &'a str,
    pub cwd: &'a str,
}

/// Orders items: P1 before P2 before P3; within a priority the earlier
/// deadline first (no deadline last), then the oldest, then by id, so the
/// order never depends on the input order.
pub fn rank(mut items: Vec<AttentionItem>) -> Vec<AttentionItem> {
    items.sort_by(|a, b| {
        (
            a.priority,
            a.deadline.unwrap_or(Millis::MAX),
            a.since,
            &a.id,
        )
            .cmp(&(
                b.priority,
                b.deadline.unwrap_or(Millis::MAX),
                b.since,
                &b.id,
            ))
    });
    items
}

/// Programs that destroy data or history when run by a shell.
const DESTRUCTIVE_PROGRAMS: [&str; 11] = [
    "rm",
    "rmdir",
    "rd",
    "del",
    "erase",
    "shred",
    "mkfs",
    "dd",
    "remove-item",
    "format",
    "truncate",
];

/// Whether a tool call can destroy work that is hard to get back: the
/// built-in deny class, file removal, forced or history-rewriting git.
pub fn is_destructive(tool: &str, subject: &str, cwd: &str) -> bool {
    let builtin = BUILTIN_DENY
        .iter()
        .filter_map(|rule| Pattern::parse(rule).ok())
        .any(|p| {
            if p.is_command_rule() {
                p.covers_tool(tool) && p.matches_command(subject)
            } else {
                p.matches(tool, subject, cwd)
            }
        });
    builtin
        || (matches!(tool, "Bash" | "PowerShell" | "Shell")
            && subject
                .split(['\n', ';', '|', '&'])
                .any(destructive_command))
}

fn destructive_command(command: &str) -> bool {
    let mut words = command
        .split_whitespace()
        .skip_while(|w| matches!(*w, "sudo" | "command" | "env" | "time"));
    let Some(program) = words.next() else {
        return false;
    };
    let program = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let program = program.trim_end_matches(".exe");
    if DESTRUCTIVE_PROGRAMS.contains(&program) {
        return true;
    }
    if program != "git" {
        return false;
    }
    let rest: Vec<&str> = words.collect();
    match rest.first().copied() {
        Some("push") => true,
        Some("reset") => rest.contains(&"--hard"),
        Some("clean") => true,
        Some("checkout" | "restore") => rest.contains(&"--") || rest.contains(&"."),
        Some("branch") => rest.contains(&"-D"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(id: u64, tool: &'a str, subject: &'a str) -> HeldCall<'a> {
        HeldCall {
            request_id: id,
            job: Some(JobId(7)),
            session: None,
            tool,
            subject,
            cwd: "C:/work",
        }
    }

    fn item(
        key: &str,
        priority: Priority,
        deadline: Option<Millis>,
        since: Millis,
    ) -> AttentionItem {
        AttentionItem {
            id: AttentionId::new(key),
            kind: AttentionKind::ResultReady,
            priority,
            title: key.into(),
            detail: String::new(),
            session: None,
            job: None,
            request: None,
            since,
            deadline,
            likelihood: Likelihood::Certain,
        }
    }

    #[test]
    fn destructive_commands_are_p1_and_the_rest_p2() {
        for subject in [
            "rm -rf target",
            "git push origin main",
            "git reset --hard HEAD~1",
            "cargo build && rm -r build",
            "Remove-Item -Recurse out",
            "C:\\tools\\rm.exe a.txt",
        ] {
            let it = AttentionItem::approval(&call(1, "Bash", subject), 0, 60_000);
            assert_eq!(it.priority, Priority::P1, "{subject}");
        }
        for subject in [
            "cargo test",
            "git status",
            "git reset --soft HEAD~1",
            "ls rmdir-notes",
        ] {
            let it = AttentionItem::approval(&call(1, "Bash", subject), 0, 60_000);
            assert_eq!(it.priority, Priority::P2, "{subject}");
        }
        let edit = AttentionItem::approval(&call(2, "Edit", "src/main.rs"), 0, 1);
        assert_eq!(edit.priority, Priority::P2);
        assert_eq!(edit.id.as_str(), "approval:2");
        assert_eq!(edit.likelihood, Likelihood::Certain);
    }

    #[test]
    fn job_items_have_stable_ids_and_the_mapped_priorities() {
        let s = SessionId::random();
        let failed = AttentionItem::job_failed(JobId(3), s, "fix it", "exit 1", 5);
        assert_eq!(failed.id.as_str(), "job:3:failed");
        assert_eq!(failed.priority, Priority::P2);
        let concerns =
            AttentionItem::review_concerns(JobId(3), s, "fix it", &["a".into(), "b".into()], 5);
        assert_eq!(concerns.priority, Priority::P3);
        assert_eq!(concerns.detail, "a (+1 more)");
        assert_eq!(concerns.likelihood, Likelihood::Likely);
        let stalled = AttentionItem::stalled(JobId(3), s, "fix it", 5);
        assert_eq!(stalled.priority, Priority::P3);
        assert_eq!(stalled.likelihood, Likelihood::Possible);
        assert_eq!(stalled.id.as_str(), "job:3:stalled");
    }

    #[test]
    fn ranking_is_priority_then_deadline_then_age_then_id() {
        let items = vec![
            item("e", Priority::P3, None, 1),
            item("d", Priority::P2, None, 9),
            item("c", Priority::P2, Some(500), 8),
            item("b", Priority::P2, Some(100), 9),
            item("a", Priority::P1, None, 50),
            item("g", Priority::P3, None, 1),
            item("f", Priority::P3, None, 0),
        ];
        let order = |v: Vec<AttentionItem>| {
            rank(v)
                .into_iter()
                .map(|i| i.id.to_string())
                .collect::<Vec<_>>()
                .join("")
        };
        assert_eq!(order(items.clone()), "abcdfeg");
        let mut reversed = items;
        reversed.reverse();
        assert_eq!(order(reversed), "abcdfeg", "input order does not matter");
    }
}
