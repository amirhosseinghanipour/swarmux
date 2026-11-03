//! Readings that arrive from the sessions themselves: status lines and the
//! instruction files they loaded. Kept in one place so the hook bus and the
//! telemetry service see the same facts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use swarmux_domain::telemetry::{
    InstructionFile, LimitState, PromptCache, PullRef, Statusline, remember,
};
use swarmux_domain::{JobId, Millis};

use crate::ports::Clock;

/// Sessions whose status line is remembered.
const SESSIONS_KEPT: usize = 50;
/// A status line older than this is no longer listed.
const LISTED_FOR: Millis = swarmux_domain::telemetry::DAY;

/// Where status lines come from: swarmux's socket, a spool folder, … Never
/// blocks.
pub trait StatuslineFeed {
    /// Status lines received since the last call.
    fn poll_statuslines(&self) -> Vec<Statusline>;
}

/// Several feeds read as one.
pub struct CombinedStatusline(pub Vec<Rc<dyn StatuslineFeed>>);

impl StatuslineFeed for CombinedStatusline {
    fn poll_statuslines(&self) -> Vec<Statusline> {
        self.0.iter().flat_map(|f| f.poll_statuslines()).collect()
    }
}

/// A session's latest status line, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct StatuslineView {
    pub session_id: String,
    pub job: Option<JobId>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub cost_usd: Option<f64>,
    /// 0 to 1.
    pub context_used: Option<f64>,
    pub cache: Option<CacheView>,
    pub pr: Option<PullRef>,
    pub observed_at: Millis,
}

/// Prompt-cache health at the time of the snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheView {
    /// The next request is served from the cache.
    pub warm: bool,
    /// `5m` or `1h`.
    pub ttl: Option<String>,
    /// 0 to 1.
    pub hit_ratio: Option<f64>,
    /// Time until the cache is cold (0 once cold).
    pub expires_in: Option<Millis>,
    pub requests: Option<u64>,
    pub misses: Option<u64>,
}

impl CacheView {
    fn new(cache: &PromptCache, now: Millis) -> CacheView {
        CacheView {
            warm: !cache.is_cold(now),
            ttl: cache.ttl.clone(),
            hit_ratio: cache.hit_ratio,
            expires_in: cache.expires_in(now),
            requests: cache.requests,
            misses: cache.misses,
        }
    }
}

/// The instruction files one session loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct InstructionsView {
    pub session_id: String,
    pub job: Option<JobId>,
    /// In the order they were loaded.
    pub files: Vec<InstructionFile>,
}

#[derive(Default)]
struct Inner {
    statuses: HashMap<String, Statusline>,
    /// Account windows as the status lines report them.
    windows: HashMap<String, LimitState>,
    instructions: HashMap<String, (Option<JobId>, Vec<InstructionFile>)>,
}

/// What the sessions themselves reported.
pub struct TruthBoard {
    clock: Rc<dyn Clock>,
    inner: RefCell<Inner>,
}

impl TruthBoard {
    pub fn new(clock: Rc<dyn Clock>) -> TruthBoard {
        TruthBoard {
            clock,
            inner: RefCell::new(Inner::default()),
        }
    }

    /// Takes a status line. An older one than what is kept is ignored.
    pub fn record_statusline(&self, line: Statusline) {
        let mut inner = self.inner.borrow_mut();
        for limit in line.rate_limits() {
            match inner.windows.get_mut(&limit.window) {
                Some(state) if state.observed_at > line.at => {}
                Some(state) => state.update(limit, line.at),
                None => {
                    inner
                        .windows
                        .insert(limit.window.clone(), LimitState::new(limit, line.at));
                }
            }
        }
        let newer = inner
            .statuses
            .get(&line.session_id)
            .is_none_or(|old| old.at <= line.at);
        if !newer {
            return;
        }
        inner.statuses.insert(line.session_id.clone(), line);
        if inner.statuses.len() > SESSIONS_KEPT
            && let Some(oldest) = inner
                .statuses
                .values()
                .min_by_key(|s| s.at)
                .map(|s| s.session_id.clone())
        {
            inner.statuses.remove(&oldest);
        }
    }

    /// Notes an instruction file `session` loaded.
    pub fn record_instruction(&self, session: &str, job: Option<JobId>, file: InstructionFile) {
        let mut inner = self.inner.borrow_mut();
        let entry = inner
            .instructions
            .entry(session.to_string())
            .or_insert_with(|| (job, Vec::new()));
        if job.is_some() {
            entry.0 = job;
        }
        remember(&mut entry.1, file);
        if inner.instructions.len() > SESSIONS_KEPT {
            let oldest = inner
                .instructions
                .iter()
                .min_by_key(|(_, (_, files))| files.last().map_or(0, |f| f.loaded_at))
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                inner.instructions.remove(&oldest);
            }
        }
    }

    /// The account windows the status lines reported.
    pub fn windows(&self) -> Vec<LimitState> {
        self.inner.borrow().windows.values().cloned().collect()
    }

    /// Latest status line per session, newest first.
    pub fn statuslines(&self) -> Vec<StatuslineView> {
        let now = self.clock.now();
        let inner = self.inner.borrow();
        let mut out: Vec<StatuslineView> = inner
            .statuses
            .values()
            .filter(|s| now - s.at <= LISTED_FOR)
            .map(|s| StatuslineView {
                session_id: s.session_id.clone(),
                job: s.job,
                model: s.model.clone(),
                cwd: s.cwd.clone(),
                cost_usd: s.cost_usd,
                context_used: s.context_used,
                cache: s.cache.as_ref().map(|c| CacheView::new(c, now)),
                pr: s.pr.clone(),
                observed_at: s.at,
            })
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.observed_at));
        out
    }

    /// Instruction files per session, most recently loaded first.
    pub fn instructions(&self) -> Vec<InstructionsView> {
        let inner = self.inner.borrow();
        let mut out: Vec<InstructionsView> = inner
            .instructions
            .iter()
            .map(|(session, (job, files))| InstructionsView {
                session_id: session.clone(),
                job: *job,
                files: files.clone(),
            })
            .collect();
        out.sort_by_key(|v| std::cmp::Reverse(v.files.last().map_or(0, |f| f.loaded_at)));
        out
    }
}
