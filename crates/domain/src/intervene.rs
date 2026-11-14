//! Intervene: the operator's message to an agent that is working.
//!
//! The rules here are pure: how a message is cleaned and capped, the two
//! modes it can travel in, why it can be refused, and the receipt that
//! says where it stands. Delivering it is the application layer's job.
//!
//! swarmux does not write into a running `claude`'s input: a job is one
//! turn, and the process is told so by closing its input after the first
//! prompt. A message therefore reaches the agent the way a note already
//! does (send back): the job's Claude session continues, in the same
//! working folder, with the message as its next prompt.
//!
//! - **Queue** waits for the turn to end and then continues the session.
//! - **Steer** stops the turn now (the process is killed, like Kill) and
//!   continues the session at once. Whatever the agent was in the middle
//!   of is cut short.

use crate::{JobId, JobStatus, Millis};

/// Default longest message, in characters.
pub const DEFAULT_MAX_CHARS: usize = 2000;
/// Longest a message may be allowed to be set to.
pub const MAX_MAX_CHARS: usize = 20_000;
/// Most messages that may wait on one job.
pub const MAX_PENDING: usize = 5;

/// How a message travels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Delivered when the agent's turn ends.
    #[default]
    Queue,
    /// The turn is stopped and the message delivered at once.
    Steer,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Queue, Mode::Steer];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Queue => "queue",
            Mode::Steer => "steer",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "queue" => Some(Mode::Queue),
            "steer" => Some(Mode::Steer),
            _ => None,
        }
    }

    /// The other mode, for a key that switches between them.
    pub fn other(self) -> Mode {
        match self {
            Mode::Queue => Mode::Steer,
            Mode::Steer => Mode::Queue,
        }
    }

    /// What the mode does, in one line, for the composer.
    pub fn promise(self) -> &'static str {
        match self {
            Mode::Queue => "delivered when the agent's turn ends; nothing is interrupted",
            Mode::Steer => {
                "stops the agent's turn now, then continues it with your message; what it was doing is cut short"
            }
        }
    }
}

/// The user's settings for messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The mode the composer opens in.
    pub mode: Mode,
    /// The longest message, in characters.
    pub max_chars: usize,
    /// Steering asks for a second confirmation first.
    pub confirm_steer: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            mode: Mode::Queue,
            max_chars: DEFAULT_MAX_CHARS,
            confirm_steer: true,
        }
    }
}

/// Why a message was not taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    TooLong {
        chars: usize,
        max: usize,
    },
    NoSuchJob,
    /// The job is not running: a queued job has not started, a finished
    /// one is sent back with a note instead.
    NotRunning(JobStatus),
    /// swarmux does not run this job's session (it started in an earlier
    /// run, or by another program).
    NotControlled,
    /// A reviewer is swarmux's own, not the operator's agent.
    Reviewer,
    /// A task graph's tasks are not messaged one by one.
    TaskGraph,
    /// A call is held for the operator; steering would strand it.
    ApprovalPending,
    /// Steering needs the operator's second confirmation.
    ConfirmSteer,
    /// swarmux is stopping.
    Stopping,
    TooMany(usize),
    /// The message could not be written to the ledger, so it was not taken.
    Ledger(String),
    /// Stopping the turn failed.
    CouldNotStop(String),
}

impl Refusal {
    pub fn reason(&self) -> String {
        match self {
            Refusal::Empty => "there is nothing to send: write a message".into(),
            Refusal::TooLong { chars, max } => {
                format!("the message is {chars} characters; the longest allowed is {max}")
            }
            Refusal::NoSuchJob => "there is no such job".into(),
            Refusal::NotRunning(s) => match s {
                JobStatus::Queued => {
                    "the job has not started: its prompt is still waiting to run".into()
                }
                s => format!(
                    "the job is {}: only a running job can be messaged; send an ended one back with a note",
                    s.as_str()
                ),
            },
            Refusal::NotControlled => {
                "swarmux does not run this job's session (it began in an earlier run): open it in a terminal to take over".into()
            }
            Refusal::Reviewer => "a reviewer is swarmux's own: it is not messaged".into(),
            Refusal::TaskGraph => {
                "this mission runs as a task graph: its tasks are not messaged one by one".into()
            }
            Refusal::ApprovalPending => {
                "a call is held for you: answer it (a denial can carry a note) before you steer, or queue the message".into()
            }
            Refusal::ConfirmSteer => {
                "steering stops the agent's turn and cuts its work short: confirm it first".into()
            }
            Refusal::Stopping => "swarmux is stopping: nothing new is delivered".into(),
            Refusal::TooMany(n) => format!("{n} messages already wait on this job"),
            Refusal::Ledger(why) => format!(
                "the message could not be written to the ledger, so nothing was sent: {why}"
            ),
            Refusal::CouldNotStop(why) => format!("the turn could not be stopped: {why}"),
        }
    }
}

/// Where a message stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Accepted and recorded; it goes out when the job's turn has ended.
    Queued,
    /// Handed to the agent as a new job of the same session; that job has
    /// not started yet, so whether the agent read it is not known.
    Handed { job: JobId },
    /// The agent's turn with the message started: `at` is when swarmux saw
    /// its first report.
    Delivered { job: JobId, at: Millis },
    /// Not taken; nothing was sent.
    Refused(Refusal),
    /// Taken, but could not be delivered; the reason says why. The
    /// operator's text is kept to send again.
    Undelivered(String),
}

impl State {
    /// Nothing more will happen to the message.
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            State::Delivered { .. } | State::Refused(_) | State::Undelivered(_)
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Handed { .. } => "handed_over",
            State::Delivered { .. } => "delivered",
            State::Refused(_) => "refused",
            State::Undelivered(_) => "undelivered",
        }
    }
}

/// What the operator is told about a message, and what the screen shows
/// beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// Numbered from 1 in the run.
    pub id: u64,
    pub job: JobId,
    pub mode: Mode,
    /// The cleaned message.
    pub text: String,
    pub state: State,
    /// When it was taken.
    pub at: Millis,
    /// The ledger entry that records it, when there is a ledger.
    pub seq: Option<u64>,
}

impl Receipt {
    /// One line for the status bar and the pane: what happened, and what
    /// is not known.
    pub fn summary(&self) -> String {
        match &self.state {
            State::Queued => "queued: delivered when the agent's turn ends".into(),
            State::Handed { job } => format!(
                "handed to the agent as job #{job}: delivered once that turn starts (not confirmed yet)"
            ),
            State::Delivered { job, .. } => {
                format!("delivered: the agent's turn with it started as job #{job}")
            }
            State::Refused(r) => format!("not sent: {}", r.reason()),
            State::Undelivered(why) => format!("not delivered: {why}"),
        }
    }
}

/// The message as the user wrote it, made safe to carry: line endings
/// unified, control characters other than newline and tab dropped, the
/// ends trimmed. Empty, or over `max` characters, is refused.
pub fn clean(text: &str, max: usize) -> Result<String, Refusal> {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let cleaned: String = unified
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        return Err(Refusal::Empty);
    }
    let chars = cleaned.chars().count();
    if chars > max {
        return Err(Refusal::TooLong { chars, max });
    }
    Ok(cleaned)
}

/// The prompt the agent receives: the operator's words, framed so the
/// agent knows where they came from and what happened to its turn.
pub fn prompt(mode: Mode, messages: &[&str]) -> String {
    let body = messages
        .iter()
        .map(|m| m.trim())
        .collect::<Vec<_>>()
        .join("\n\n");
    let lead = match (mode, messages.len()) {
        (Mode::Queue, 1) => "The operator sent you a message while you were working:",
        (Mode::Queue, _) => "The operator sent you these messages while you were working:",
        (Mode::Steer, _) => {
            "The operator stopped your turn to tell you this. Your last step may not have finished:"
        }
    };
    let tail = match mode {
        Mode::Queue => "Carry on in the same session and the same working folder.",
        Mode::Steer => {
            "Check the state of the working folder before you go on, then carry on in the same session and the same working folder."
        }
    };
    format!("{lead}\n\n{body}\n\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_switch() {
        for m in Mode::ALL {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
            assert_eq!(m.other().other(), m);
        }
        assert_eq!(Mode::parse(" steer "), Some(Mode::Steer));
        assert_eq!(Mode::parse("now"), None);
        assert_eq!(Mode::default(), Mode::Queue);
    }

    #[test]
    fn defaults_are_the_safe_ones() {
        let c = Config::default();
        assert_eq!(c.mode, Mode::Queue);
        assert!(c.confirm_steer);
        assert_eq!(c.max_chars, DEFAULT_MAX_CHARS);
    }

    #[test]
    fn clean_trims_unifies_and_drops_control_characters() {
        assert_eq!(
            clean("  use \r\nthe\rstaging\u{0}bucket\u{7}\t! \n", 100).unwrap(),
            "use \nthe\nstagingbucket\t!"
        );
    }

    #[test]
    fn empty_and_blank_messages_are_refused() {
        for t in ["", "   ", "\n\t \r\n", "\u{0}\u{1}"] {
            assert_eq!(clean(t, 10), Err(Refusal::Empty), "{t:?}");
        }
    }

    #[test]
    fn the_cap_counts_characters_not_bytes() {
        assert!(clean(&"é".repeat(10), 10).is_ok());
        assert_eq!(
            clean(&"é".repeat(11), 10),
            Err(Refusal::TooLong { chars: 11, max: 10 })
        );
    }

    #[test]
    fn every_refusal_has_a_reason_a_person_can_act_on() {
        let all = [
            Refusal::Empty,
            Refusal::TooLong { chars: 9, max: 5 },
            Refusal::NoSuchJob,
            Refusal::NotRunning(JobStatus::Done),
            Refusal::NotRunning(JobStatus::Queued),
            Refusal::NotControlled,
            Refusal::Reviewer,
            Refusal::TaskGraph,
            Refusal::ApprovalPending,
            Refusal::ConfirmSteer,
            Refusal::Stopping,
            Refusal::TooMany(5),
            Refusal::Ledger("disk full".into()),
            Refusal::CouldNotStop("gone".into()),
        ];
        for r in all {
            assert!(r.reason().len() > 10, "{r:?}");
        }
        assert!(
            Refusal::NotRunning(JobStatus::Done)
                .reason()
                .contains("send an ended one back")
        );
    }

    #[test]
    fn the_prompt_carries_the_words_and_says_what_happened_to_the_turn() {
        let q = prompt(Mode::Queue, &["use the staging bucket"]);
        assert!(q.contains("use the staging bucket"));
        assert!(q.starts_with("The operator sent you a message while you were working"));
        assert!(!q.contains("stopped your turn"));
        let s = prompt(Mode::Steer, &["stop", "and wait"]);
        assert!(s.contains("stopped your turn"));
        assert!(s.contains("stop\n\nand wait"));
        assert!(s.contains("Check the state of the working folder"));
    }

    #[test]
    fn a_receipt_never_claims_delivery_before_the_turn_started() {
        let mut r = Receipt {
            id: 1,
            job: JobId(3),
            mode: Mode::Queue,
            text: "x".into(),
            state: State::Queued,
            at: 0,
            seq: None,
        };
        assert!(!r.state.is_settled());
        assert!(!r.summary().contains("delivered:"));
        r.state = State::Handed { job: JobId(4) };
        assert!(!r.state.is_settled());
        assert!(r.summary().contains("not confirmed"));
        r.state = State::Delivered {
            job: JobId(4),
            at: 9,
        };
        assert!(r.state.is_settled());
        assert!(r.summary().starts_with("delivered:"));
        r.state = State::Undelivered("the job was stopped".into());
        assert!(r.state.is_settled());
        assert!(r.summary().contains("the job was stopped"));
    }
}
