//! The Board: typed shared state between the agents of a mission.
//!
//! Each mission job gets a `swarmux-board` MCP server whose tools reach
//! [`BoardService`] through the hook transport; the app stays the only
//! writer of the store. New sessions are given a digest of the board
//! ([`BoardDigest`]), and what an agent asks the user shows in the inbox.
//!
//! Everything on the board is untrusted text from agents: it is cleaned on
//! the way in and quoted, with provenance, on the way out (see
//! `swarmux_domain::board`).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use swarmux_domain::board::{
    BoardEntry, BoardKind, DETAIL_MAX, DIGEST_TOKENS, READ_TOKENS, TEXT_MAX, claim_holder, clean,
    clean_items, digest, render_read,
};
use swarmux_domain::taskgraph::{areas_overlap, claims_overlap};
use swarmux_domain::{AttentionItem, HookEvent, JobId, Millis};

use crate::hookbus::{HookNotice, SessionStartContext, SessionStartInfo};
use crate::missions::ScopeClaims;
use crate::ports::{Clock, JobRepository};
use crate::{AppError, AppResult};

/// Entries kept in memory and shown, newest.
const KEPT: usize = 2000;
/// Entries one job may write.
const PER_JOB_MAX: usize = 300;

/// Where entries are kept. Implemented by the database.
pub trait BoardStore {
    /// Stores a new entry; returns its id.
    fn append(&self, entry: &BoardEntry) -> AppResult<i64>;
    /// The newest `limit` entries, oldest first.
    fn recent(&self, limit: usize) -> AppResult<Vec<BoardEntry>>;
    /// Stores the changed state (session, answer, closed) of an entry.
    fn update(&self, entry: &BoardEntry) -> AppResult<()>;
}

/// What an agent asked of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardRequest {
    Claim {
        paths: Vec<String>,
        note: String,
    },
    Report {
        status: String,
        summary: String,
    },
    Fact {
        text: String,
        evidence: String,
    },
    Decision {
        text: String,
        rationale: String,
    },
    AskHuman {
        question: String,
        options: Vec<String>,
    },
    Followup {
        text: String,
        after_minutes: Option<u32>,
    },
    Read {
        kinds: Vec<BoardKind>,
        /// Only entries with an id above this.
        since: Option<i64>,
    },
}

/// One request, as the transport received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardCall {
    pub id: u64,
    pub job: Option<JobId>,
    pub request: BoardRequest,
}

/// The answer to a call. An error is shown to the agent as a failed tool
/// call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardReply {
    pub text: String,
    pub error: bool,
}

impl BoardReply {
    fn ok(text: impl Into<String>) -> BoardReply {
        BoardReply {
            text: text.into(),
            error: false,
        }
    }

    pub fn failed(text: impl Into<String>) -> BoardReply {
        BoardReply {
            text: text.into(),
            error: true,
        }
    }
}

/// Where board calls arrive. Implemented by the hook transport.
pub trait BoardInbox {
    /// Calls received since the last time.
    fn poll(&self) -> Vec<BoardCall>;
    fn respond(&self, call: u64, reply: BoardReply);
}

/// Which mission, and which of its tasks, a job works for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub mission: String,
    pub task: Option<String>,
}

#[derive(Default)]
struct State {
    roster: HashMap<JobId, Member>,
    entries: Vec<BoardEntry>,
}

/// The board as the threads of the app and of the hook transport share
/// it: who is on it, and its entries.
#[derive(Default)]
pub struct Board {
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Board {
    pub fn new() -> Arc<Board> {
        Arc::new(Board::default())
    }

    /// Puts the job on its mission's board.
    pub fn join(&self, job: JobId, mission: &str, task: Option<&str>) {
        lock(&self.state).roster.insert(
            job,
            Member {
                mission: mission.to_string(),
                task: task.map(str::to_string),
            },
        );
    }

    pub fn member(&self, job: JobId) -> Option<Member> {
        lock(&self.state).roster.get(&job).cloned()
    }

    pub fn is_member(&self, job: JobId) -> bool {
        lock(&self.state).roster.contains_key(&job)
    }

    /// The mission's entries, oldest first.
    pub fn entries_of(&self, mission: &str) -> Vec<BoardEntry> {
        lock(&self.state)
            .entries
            .iter()
            .filter(|e| e.mission == mission)
            .cloned()
            .collect()
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> Vec<BoardEntry> {
        lock(&self.state).entries.clone()
    }

    fn push(&self, entry: BoardEntry) {
        let mut s = lock(&self.state);
        s.entries.push(entry);
        let extra = s.entries.len().saturating_sub(KEPT);
        s.entries.drain(..extra);
    }

    fn with_entry<R>(&self, id: i64, f: impl FnOnce(&mut BoardEntry) -> R) -> Option<R> {
        lock(&self.state)
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .map(f)
    }
}

/// Adds the board's digest to what a session is started with, after
/// whatever `inner` adds (the handoff of a checkpoint).
pub struct BoardDigest {
    board: Arc<Board>,
    inner: Option<Arc<dyn SessionStartContext>>,
    cap: usize,
}

impl BoardDigest {
    pub fn new(board: Arc<Board>, inner: Option<Arc<dyn SessionStartContext>>) -> BoardDigest {
        BoardDigest {
            board,
            inner,
            cap: DIGEST_TOKENS,
        }
    }

    /// A different token cap (tests).
    pub fn with_cap(mut self, cap: usize) -> BoardDigest {
        self.cap = cap;
        self
    }
}

impl SessionStartContext for BoardDigest {
    fn context(&self, start: &SessionStartInfo) -> Option<String> {
        let inner = self.inner.as_ref().and_then(|i| i.context(start));
        let board = start
            .job
            .and_then(|job| self.board.member(job))
            .and_then(|m| digest(&self.board.entries_of(&m.mission), self.cap));
        match (inner, board) {
            (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
            (a, b) => a.or(b),
        }
    }
}

/// Handles the calls agents make on the board.
pub struct BoardService {
    store: Rc<dyn BoardStore>,
    board: Arc<Board>,
    claims: Option<Arc<ScopeClaims>>,
    jobs: Rc<dyn JobRepository>,
    clock: Rc<dyn Clock>,
}

impl BoardService {
    /// Loads what earlier runs left on the boards.
    pub fn new(
        store: Rc<dyn BoardStore>,
        board: Arc<Board>,
        jobs: Rc<dyn JobRepository>,
        clock: Rc<dyn Clock>,
    ) -> AppResult<BoardService> {
        for e in store.recent(KEPT)? {
            board.push(e);
        }
        Ok(BoardService {
            store,
            board,
            claims: None,
            jobs,
            clock,
        })
    }

    /// Checks claims against the scopes the task graph gave its tasks.
    pub fn with_claims(mut self, claims: Arc<ScopeClaims>) -> BoardService {
        self.claims = Some(claims);
        self
    }

    pub fn board(&self) -> &Arc<Board> {
        &self.board
    }

    /// Answers the calls that arrived, and closes the claims of jobs that
    /// ended. Returns what could not be stored.
    pub fn pump(&self, inbox: &dyn BoardInbox, finished: &dyn Fn(JobId) -> bool) -> Vec<AppError> {
        let mut errors = Vec::new();
        for call in inbox.poll() {
            let reply = match self.handle(&call) {
                Ok(reply) => reply,
                Err(e) => {
                    let reply = BoardReply::failed("swarmux could not store that; try again");
                    errors.push(e);
                    reply
                }
            };
            inbox.respond(call.id, reply);
        }
        errors.extend(self.sweep(finished).err());
        errors
    }

    /// Releases the claims of jobs that have ended.
    pub fn sweep(&self, finished: &dyn Fn(JobId) -> bool) -> AppResult<()> {
        let stale: Vec<BoardEntry> = self
            .board
            .entries()
            .into_iter()
            .filter(|e| e.is_live_claim() && finished(e.job))
            .collect();
        for mut e in stale {
            e.closed = true;
            self.store.update(&e)?;
            self.board.with_entry(e.id, |x| x.closed = true);
        }
        Ok(())
    }

    /// Answers one call.
    pub fn handle(&self, call: &BoardCall) -> AppResult<BoardReply> {
        let Some(job) = call.job else {
            return Ok(BoardReply::failed(
                "this board server was not started by swarmux for a job",
            ));
        };
        let Some(member) = self.board.member(job) else {
            return Ok(BoardReply::failed(
                "this job is not part of a mission, so it has no board",
            ));
        };
        let now = self.clock.now();
        let mut entry = match &call.request {
            BoardRequest::Read { kinds, since } => {
                return Ok(self.read(&member, kinds, *since));
            }
            BoardRequest::Claim { paths, note } => {
                let areas = clean_items(paths);
                if areas.is_empty() {
                    return Ok(BoardReply::failed("claim needs at least one path or glob"));
                }
                if let Some(why) = self.conflict(job, &member, &areas) {
                    return Ok(BoardReply::failed(why));
                }
                let mut e = BoardEntry::new(&member.mission, job, BoardKind::Claim, note, now);
                e.areas = areas;
                e
            }
            BoardRequest::Report { status, summary } => {
                let mut e = BoardEntry::new(&member.mission, job, BoardKind::Report, summary, now);
                e.detail = clean(status, 40);
                e
            }
            BoardRequest::Fact { text, evidence } => {
                let mut e = BoardEntry::new(&member.mission, job, BoardKind::Fact, text, now);
                e.detail = clean(evidence, DETAIL_MAX);
                e
            }
            BoardRequest::Decision { text, rationale } => {
                let mut e = BoardEntry::new(&member.mission, job, BoardKind::Decision, text, now);
                e.detail = clean(rationale, DETAIL_MAX);
                e
            }
            BoardRequest::AskHuman { question, options } => {
                let mut e =
                    BoardEntry::new(&member.mission, job, BoardKind::Question, question, now);
                e.detail = clean_items(options).join(" | ");
                e
            }
            BoardRequest::Followup {
                text,
                after_minutes,
            } => {
                let mut e = BoardEntry::new(&member.mission, job, BoardKind::Followup, text, now);
                e.after_minutes = *after_minutes;
                e
            }
        };
        if entry.text.is_empty() && entry.kind != BoardKind::Claim {
            return Ok(BoardReply::failed("the text is empty"));
        }
        let written = self
            .board
            .entries()
            .iter()
            .filter(|e| e.job == job && e.mission == member.mission)
            .count();
        if written >= PER_JOB_MAX {
            return Ok(BoardReply::failed(format!(
                "this job has written {PER_JOB_MAX} entries, the most one job may"
            )));
        }
        entry.task = member.task.clone();
        entry.session = self
            .jobs
            .get(job)?
            .map(|j| j.session_id.to_string())
            .unwrap_or_default();
        // A new claim replaces the job's earlier ones.
        if entry.kind == BoardKind::Claim {
            let earlier: Vec<BoardEntry> = self
                .board
                .entries()
                .into_iter()
                .filter(|e| e.is_live_claim() && e.job == job)
                .collect();
            for mut e in earlier {
                e.closed = true;
                self.store.update(&e)?;
                self.board.with_entry(e.id, |x| x.closed = true);
            }
        }
        entry.id = self.store.append(&entry)?;
        let reply = match entry.kind {
            BoardKind::Claim => format!("claimed: {}", entry.areas.join(", ")),
            BoardKind::Question => format!(
                "question #{} is with the user. Carry on with what does not depend on it; \
                 call read with kinds [\"question\"] later to see the answer.",
                entry.id
            ),
            BoardKind::Followup => format!("follow-up #{} recorded for the user", entry.id),
            kind => format!("{} #{} recorded", kind.as_str(), entry.id),
        };
        self.board.push(entry);
        Ok(BoardReply::ok(reply))
    }

    /// Why `areas` can't be claimed by `job`, naming who holds them.
    fn conflict(&self, job: JobId, member: &Member, areas: &[String]) -> Option<String> {
        let entries = self.board.entries_of(&member.mission);
        if let Some(held) = claims_as_held(&self.claims, job, member, areas) {
            return Some(held);
        }
        if let Some(own) = self.claims.as_ref().and_then(|c| c.get(job))
            && let Some(outside) = areas
                .iter()
                .find(|a| !own.areas.iter().any(|o| areas_overlap(o, a)))
        {
            return Some(format!(
                "{outside} is outside this task's scope ({}); claim only files inside it",
                own.areas.join(", ")
            ));
        }
        let holder = claim_holder(&entries, job, areas)?;
        Some(format!(
            "rejected: {} is claimed by job {}{}; pick other files or ask the user",
            holder.areas.join(", "),
            holder.job,
            holder
                .task
                .as_ref()
                .map(|t| format!(" (task {})", clean(t, 40)))
                .unwrap_or_default(),
        ))
    }

    fn read(&self, member: &Member, kinds: &[BoardKind], since: Option<i64>) -> BoardReply {
        let entries = self.board.entries_of(&member.mission);
        let shown: Vec<&BoardEntry> = entries
            .iter()
            .filter(|e| kinds.is_empty() || kinds.contains(&e.kind))
            .filter(|e| since.is_none_or(|s| e.id > s))
            .collect();
        BoardReply::ok(render_read(&shown, READ_TOKENS))
    }

    /// Mirrors Claude Code's own task events as reports.
    pub fn observe(&self, notice: &HookNotice) -> AppResult<()> {
        let status = match notice.event {
            HookEvent::TaskCreated => "task created",
            HookEvent::TaskCompleted => "task completed",
            _ => return Ok(()),
        };
        let Some(job) = notice.job else {
            return Ok(());
        };
        let Some(member) = self.board.member(job) else {
            return Ok(());
        };
        let mut e = BoardEntry::new(
            &member.mission,
            job,
            BoardKind::Report,
            &notice.summary,
            self.clock.now(),
        );
        if e.text.is_empty() {
            return Ok(());
        }
        e.detail = status.to_string();
        e.task = member.task;
        e.session = notice.session.clone();
        e.id = self.store.append(&e)?;
        self.board.push(e);
        Ok(())
    }

    /// Open questions, for the inbox.
    pub fn question_items(&self) -> Vec<AttentionItem> {
        self.board
            .entries()
            .iter()
            .filter(|e| e.is_open_question())
            .map(|e| {
                let detail = if e.detail.is_empty() {
                    format!("job {} asks", e.job)
                } else {
                    format!("job {} asks · options: {}", e.job, clean(&e.detail, 160))
                };
                AttentionItem::board_question(e.id, e.job, &clean(&e.text, 160), &detail, e.at)
            })
            .collect()
    }

    /// The question's text and options, when it is open.
    pub fn open_question(&self, id: i64) -> Option<BoardEntry> {
        self.board
            .entries()
            .into_iter()
            .find(|e| e.id == id && e.is_open_question())
    }

    /// Gives the user's answer to a question; agents see it with `read`.
    pub fn answer(&self, id: i64, answer: &str) -> AppResult<()> {
        let answer = clean(answer, TEXT_MAX);
        if answer.is_empty() {
            return Err(AppError::Mission("the answer is empty".into()));
        }
        let mut entry = self
            .open_question(id)
            .ok_or_else(|| AppError::Mission(format!("question #{id} is not open")))?;
        entry.answer = Some(answer.clone());
        entry.closed = true;
        self.store.update(&entry)?;
        self.board.with_entry(id, |e| {
            e.answer = Some(answer);
            e.closed = true;
        });
        Ok(())
    }

    /// Closes a question without an answer.
    pub fn dismiss(&self, id: i64) -> AppResult<()> {
        let mut entry = self
            .open_question(id)
            .ok_or_else(|| AppError::Mission(format!("question #{id} is not open")))?;
        entry.closed = true;
        self.store.update(&entry)?;
        self.board.with_entry(id, |e| e.closed = true);
        Ok(())
    }

    pub fn now(&self) -> Millis {
        self.clock.now()
    }
}

/// A conflict with a task graph's scope claim of another job.
fn claims_as_held(
    claims: &Option<Arc<ScopeClaims>>,
    job: JobId,
    member: &Member,
    areas: &[String],
) -> Option<String> {
    let held = claims.as_ref()?.held();
    let (other, claim) = held.iter().find(|(j, c)| {
        *j != job && c.mission == member.mission && claims_overlap(&c.areas, areas)
    })?;
    Some(format!(
        "rejected: those files belong to task {} (job {other}), whose scope is {}; \
         pick other files or ask the user",
        clean(&claim.task, 40),
        claim.areas.join(", ")
    ))
}

#[cfg(test)]
mod tests;
