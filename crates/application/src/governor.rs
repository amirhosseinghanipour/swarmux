//! The usage governor: which queued jobs may start given the subscription
//! windows, what each may spend, how many run on battery, and bringing
//! back work a usage limit stopped once its window resets.

use std::rc::Rc;

use swarmux_domain::budget::{
    Admission, GovernorPolicy, JobClass, admit, budget_line, hold_for_load, power_limits,
    window_label,
};
use swarmux_domain::telemetry::{LimitState, RateLimit};
use swarmux_domain::{JobId, Limits, Millis};

use crate::{AppResult, GovernorSettings, TruthBoard};

/// Where readings of the subscription windows come from: the status lines
/// sessions report, a `/usage` report, … Documented sources only.
pub trait UsageProbe {
    /// The latest reading of every window this source knows.
    fn windows(&self) -> Vec<LimitState>;
}

impl UsageProbe for TruthBoard {
    fn windows(&self) -> Vec<LimitState> {
        TruthBoard::windows(self)
    }
}

/// A job a usage limit stopped, resumed once the window resets.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingResume {
    pub job: JobId,
    /// When the window resets.
    pub at: Millis,
    /// Which window stopped it, e.g. "5h".
    pub window: String,
}

/// Keeps pending resumes across restarts.
pub trait ResumeBook {
    /// Adds or replaces the job's resume.
    fn save(&self, resume: &PendingResume) -> AppResult<()>;
    fn remove(&self, job: JobId) -> AppResult<()>;
    fn pending(&self) -> AppResult<Vec<PendingResume>>;
}

/// The governor's settings, its window sources and the work waiting for a reset.
pub struct Governor {
    settings: GovernorSettings,
    policy: GovernorPolicy,
    probes: Vec<Rc<dyn UsageProbe>>,
    book: Option<Rc<dyn ResumeBook>>,
    pending: Vec<PendingResume>,
    /// Minutes east of UTC, for "resumes 15:58".
    offset_min: i32,
}

impl Governor {
    pub fn new(settings: &GovernorSettings) -> Governor {
        Governor {
            settings: settings.clone(),
            policy: settings.policy(),
            probes: Vec::new(),
            book: None,
            pending: Vec::new(),
            offset_min: 0,
        }
    }

    /// Adds a source of window readings besides the jobs' own streams.
    pub fn with_probe(mut self, probe: Rc<dyn UsageProbe>) -> Governor {
        self.probes.push(probe);
        self
    }

    /// Keeps pending resumes in `book`; [`Governor::restore`] loads them.
    pub fn with_resume_book(mut self, book: Rc<dyn ResumeBook>) -> Governor {
        self.book = Some(book);
        self
    }

    /// Local time for the reasons shown, as minutes east of UTC.
    pub fn with_utc_offset(mut self, minutes: i32) -> Governor {
        self.offset_min = minutes;
        self
    }

    pub fn settings(&self) -> &GovernorSettings {
        &self.settings
    }

    /// Loads the resumes a previous run left pending.
    pub fn restore(&mut self) -> AppResult<()> {
        if let Some(book) = &self.book {
            self.pending = book.pending()?;
        }
        Ok(())
    }

    /// The freshest reading of every window, from the jobs' streams and the probes.
    pub fn windows(&self, stream: &[&LimitState]) -> Vec<LimitState> {
        let mut out: Vec<LimitState> = Vec::new();
        let probed: Vec<LimitState> = self.probes.iter().flat_map(|p| p.windows()).collect();
        for state in stream.iter().copied().chain(probed.iter()) {
            match out
                .iter_mut()
                .find(|w| w.limit.window == state.limit.window)
            {
                Some(w) if w.observed_at >= state.observed_at => {}
                Some(w) => *w = state.clone(),
                None => out.push(state.clone()),
            }
        }
        out
    }

    /// Whether a job on `model` may start now; everything is admitted
    /// while the governor is off.
    pub fn admit(
        &self,
        windows: &[LimitState],
        model: &str,
        class: JobClass,
        now: Millis,
    ) -> Admission {
        if !self.settings.enabled {
            return Admission::Admit;
        }
        admit(&self.policy, windows, model, class, now)
    }

    /// [`Governor::admit`], and new work waits while the user's inbox load
    /// (`load`, when it is known) is over the line. Verification never does.
    pub fn admit_under_load(
        &self,
        windows: &[LimitState],
        model: &str,
        class: JobClass,
        load: Option<f64>,
        now: Millis,
    ) -> Admission {
        let admission = self.admit(windows, model, class, now);
        if !self.settings.enabled {
            return admission;
        }
        hold_for_load(&self.policy, admission, class, load)
    }

    /// The run limits, fewer at once on battery.
    pub fn limits(&self, base: Limits, on_battery: bool) -> Limits {
        if !self.settings.enabled {
            return base;
        }
        power_limits(base, on_battery, self.settings.battery_concurrency)
    }

    /// A job's dollar cap when it has none of its own.
    pub fn default_cap(&self) -> Option<f64> {
        self.settings.max_job_usd
    }

    /// The line told to a job about its budget, when that is on.
    pub fn line(&self, windows: &[LimitState], cap: Option<f64>, now: Millis) -> Option<String> {
        if !self.settings.budget_line {
            return None;
        }
        let windows: &[LimitState] = if self.settings.enabled { windows } else { &[] };
        budget_line(windows, cap, self.policy.hold_at, now, self.offset_min)
    }

    /// What the queue shows for an admission.
    pub fn reason(&self, admission: &Admission) -> Option<String> {
        admission.reason(self.offset_min)
    }

    /// When a job stopped by a usage limit may continue: the reset of the
    /// window that refused it, else of the fullest window at the hold
    /// threshold. `None` when no reset is known.
    pub fn reset_for(
        &self,
        refused: Option<&RateLimit>,
        windows: &[LimitState],
        now: Millis,
    ) -> Option<(Millis, String)> {
        if let Some(r) = refused
            && let Some(at) = r.resets_at.filter(|at| *at > now)
        {
            return Some((at, window_label(&r.window)));
        }
        windows
            .iter()
            .filter(|w| !w.is_stale(now))
            .filter(|w| {
                w.limit.status == "rejected"
                    || w.limit
                        .utilization
                        .is_some_and(|u| u >= self.policy.hold_at)
            })
            .filter_map(|w| Some((w.limit.resets_at?, window_label(&w.limit.window))))
            .max_by_key(|(at, _)| *at)
    }

    /// Resumes `job` by itself once `at` has passed.
    pub fn resume_at(&mut self, job: JobId, at: Millis, window: &str) -> AppResult<()> {
        let resume = PendingResume {
            job,
            at,
            window: window.to_string(),
        };
        if let Some(book) = &self.book {
            book.save(&resume)?;
        }
        self.pending.retain(|p| p.job != job);
        self.pending.push(resume);
        Ok(())
    }

    /// Drops the job's pending resume (it was resumed or retried by hand).
    pub fn forget(&mut self, job: JobId) -> AppResult<()> {
        if !self.pending.iter().any(|p| p.job == job) {
            return Ok(());
        }
        self.pending.retain(|p| p.job != job);
        match &self.book {
            Some(book) => book.remove(job),
            None => Ok(()),
        }
    }

    /// Resumes whose window has reset by `now`, oldest reset first; they
    /// are no longer pending.
    pub fn due(&mut self, now: Millis) -> AppResult<Vec<PendingResume>> {
        let mut due: Vec<PendingResume> = self
            .pending
            .iter()
            .filter(|p| p.at <= now)
            .cloned()
            .collect();
        due.sort_by_key(|p| (p.at, p.job));
        for p in &due {
            self.forget(p.job)?;
        }
        Ok(due)
    }

    /// Jobs waiting for a window to reset.
    pub fn pending(&self) -> &[PendingResume] {
        &self.pending
    }
}

#[cfg(test)]
mod tests;
