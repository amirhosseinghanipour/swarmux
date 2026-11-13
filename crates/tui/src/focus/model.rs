//! What the Focus view shows, gathered once per frame from the app's
//! services so the renderers only draw.

use std::path::PathBuf;

use swarmux_application::{AppResult, CheckpointService};
use swarmux_domain::telemetry::Todo;
use swarmux_domain::{
    CheckpointReason, JobId, JobProgress, JobStatus, Millis, ReviewState, ReviewVerdict,
    SessionState,
};

use super::sat::{self, Sat, Signals};
use super::stream::{self, Row};
use crate::app::App;
use crate::mission::{Chip, Tone, gate_chips, review_chip};
use crate::ui::mission::sigil;

/// Whose job this is.
#[derive(Clone, Debug)]
pub struct Who {
    pub name: String,
    pub sigil: char,
    pub model: String,
    pub status: JobStatus,
    pub waiting: bool,
    pub permission_mode: String,
    pub lean: bool,
    pub cwd: Option<PathBuf>,
    /// The prompt's first line.
    pub goal: String,
    pub elapsed_ms: Option<Millis>,
    pub cost_usd: f64,
    pub context: Option<f64>,
    /// Share of the job's input served from the prompt cache; unknown
    /// while it runs or when it did not report.
    pub cache: Option<f64>,
}

/// A point the session can be resumed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointDot {
    pub label: String,
    pub at: Millis,
}

/// Where the job's work lives and the points it can go back to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkpoints {
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub dots: Vec<CheckpointDot>,
}

impl Checkpoints {
    /// Reads where `job` works and its checkpoints, oldest first. The
    /// worktree's branch and folder come from the newest checkpoint once
    /// the worktree itself is gone.
    pub fn load(service: &CheckpointService, job: JobId) -> AppResult<Checkpoints> {
        let list = service.list(job)?;
        let (branch, worktree) = match service.worktree(job)? {
            Some(w) => (Some(w.branch), Some(w.path.display().to_string())),
            None => match list.last() {
                Some(c) => (
                    Some(c.branch.clone()),
                    Some(c.worktree.display().to_string()),
                ),
                None => (None, None),
            },
        };
        let dots = list
            .iter()
            .map(|c| CheckpointDot {
                label: format!("{} {}", reason_label(c.reason), short_sha(&c.commit)),
                at: c.at,
            })
            .collect();
        Ok(Checkpoints {
            branch,
            worktree,
            dots,
        })
    }

    /// What the app last read for `job`; empty when it has read nothing.
    pub fn of(app: &App, job: JobId) -> Checkpoints {
        app.checkpoints_of(job).cloned().unwrap_or_default()
    }
}

fn reason_label(reason: CheckpointReason) -> &'static str {
    match reason {
        CheckpointReason::GatePass => "gate pass",
        CheckpointReason::PreCompact => "before compact",
        CheckpointReason::Manual => "manual",
        CheckpointReason::Recovery => "recovery",
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    /// What the user did: prompts and answers.
    You,
    Lead,
    Subagent,
    Reviewer,
    /// Guard and routing decisions.
    Guard,
}

#[derive(Clone, Debug)]
pub struct TimeLane {
    pub label: String,
    pub kind: LaneKind,
    /// Ticks (tenths of a second into the job).
    pub marks: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Timeline {
    pub end: u32,
    pub lanes: Vec<TimeLane>,
}

#[derive(Clone, Debug)]
pub struct ReviewColumn {
    pub model: String,
    /// `None` until there is a verdict.
    pub verdict: Option<ReviewVerdict>,
    pub headline: String,
    pub concerns: Vec<String>,
    pub diffs_checked: Option<u32>,
    pub cost_usd: Option<f64>,
    pub running: bool,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug)]
pub struct AgentLine {
    pub kind: String,
    pub model: Option<String>,
    pub status: swarmux_domain::SubagentStatus,
}

pub struct Focus {
    pub who: Who,
    pub sat: Sat,
    pub chips: Vec<Chip>,
    pub checkpoints: Checkpoints,
    /// Why the loop breaker stopped agents, oldest first.
    pub recovery: Vec<String>,
    pub timeline: Timeline,
    pub rows: Vec<Row>,
    pub answer: Option<String>,
    pub review: Option<ReviewColumn>,
    pub todos: Vec<Todo>,
    pub agents: Vec<AgentLine>,
    /// Rows from here on arrived since the user last looked.
    pub here: Option<usize>,
}

impl Focus {
    /// The followed job, or the latest one; `None` before any job.
    pub fn of(app: &App) -> Option<Focus> {
        if app.sessions.is_empty() {
            return None;
        }
        let job = app.current_job()?;
        let review_run = app.review_run();
        let fact = app.mission.facts.iter().find(|f| f.job_id == job.job_id);
        let session = fact
            .map(|f| f.session_id)
            .and_then(|id| app.sessions.iter().find(|s| s.id == id))
            .or_else(|| app.sessions.iter().find(|s| s.name == job.session_name));
        let held = app
            .engine
            .guard
            .approvals()
            .into_iter()
            .find(|a| a.request.job_id == Some(job.job_id))
            .map(|a| format!("{} {}", a.request.tool_name, a.request.subject));
        let gate = app.mission.gates.get(&job.job_id);
        let todos: Vec<Todo> = app
            .engine
            .jobs
            .job_activity(job.job_id)
            .map(|a| a.todos.clone())
            .unwrap_or_default();
        let answer = fact
            .and_then(|f| f.result.clone())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| app.dashboard.replay_answer.clone());
        let goal = fact
            .map(|f| f.prompt.as_str())
            .or(app.dashboard.replay_prompt.as_deref())
            .unwrap_or("");
        let rows = stream::rows(&job.log);
        let review_state = job.review.as_ref();
        let state = fact.and_then(|f| f.session_state);
        let sat = sat::card(&Signals {
            status: job.status,
            goal,
            rows: &rows,
            todos: &todos,
            gate,
            review: review_state,
            loop_stops: job.loop_stops.len(),
            state,
            held: held.clone(),
            error: fact.and_then(|f| f.error.as_deref()),
            answer: answer.as_deref(),
        });

        let mut chips = gate.map(gate_chips).unwrap_or_default();
        chips.extend(review_state.map(review_chip));
        if held.is_some() {
            chips.push(Chip {
                text: "held call".into(),
                tone: Tone::Warn,
            });
        }
        let now = app.now();
        let who = Who {
            name: job.session_name.clone(),
            sigil: sigil(&job.session_name),
            model: job.model.clone(),
            status: job.status,
            waiting: held.is_some() || state == Some(SessionState::RequiresAction),
            permission_mode: session
                .map(|s| s.permission_mode.clone())
                .unwrap_or_default(),
            lean: session.is_some_and(|s| s.lean),
            cwd: session.map(|s| s.cwd.clone()),
            goal: goal.lines().next().unwrap_or("").trim().to_string(),
            elapsed_ms: fact.and_then(|f| {
                let start = f.started_at?;
                Some((f.finished_at.unwrap_or(now) - start).max(0))
            }),
            cost_usd: job.cost_usd,
            context: fact.and_then(|f| f.context),
            cache: fact.and_then(|f| f.cache_ratio),
        };

        let started = fact.and_then(|f| f.started_at.or(Some(f.created_at)));
        let timeline = timeline(app, job, review_run, started);
        let review = review_column(app, job, review_run);
        let agents = job
            .subagents
            .iter()
            .map(|s| AgentLine {
                kind: s.kind.clone(),
                model: s.model.clone(),
                status: s.status,
            })
            .collect();
        let here = app
            .focus_state
            .marker_for(job.job_id)
            .filter(|m| *m < rows.len());
        Some(Focus {
            who,
            sat,
            chips,
            checkpoints: Checkpoints::of(app, job.job_id),
            recovery: job
                .loop_stops
                .iter()
                .chain(&job.recovery)
                .cloned()
                .collect(),
            timeline,
            rows,
            answer,
            review,
            todos,
            agents,
            here,
        })
    }
}

fn timeline(
    app: &App,
    job: &JobProgress,
    review_run: Option<&JobProgress>,
    started: Option<Millis>,
) -> Timeline {
    let mut lanes = Vec::new();
    let mut human = Vec::new();
    if let Some(start) = started {
        human.push(0);
        human.extend(
            app.focus_state
                .human_events(job.job_id)
                .iter()
                .map(|at| u32::try_from(((at - start).max(0)) / 100).unwrap_or(u32::MAX)),
        );
    }
    lanes.push(TimeLane {
        label: "you".into(),
        kind: LaneKind::You,
        marks: human,
    });
    for (i, lane) in job.lanes.iter().enumerate() {
        lanes.push(TimeLane {
            label: lane.label.clone(),
            kind: if i == 0 {
                LaneKind::Lead
            } else {
                LaneKind::Subagent
            },
            marks: lane.marks.clone(),
        });
    }
    if let Some(r) = review_run {
        let marks = r
            .lanes
            .iter()
            .flat_map(|l| l.marks.iter().map(|m| m + r.log_offset))
            .collect();
        lanes.push(TimeLane {
            label: "reviewer".into(),
            kind: LaneKind::Reviewer,
            marks,
        });
    }
    if !job.decision_marks.is_empty() {
        lanes.push(TimeLane {
            label: "guard".into(),
            kind: LaneKind::Guard,
            marks: job.decision_marks.clone(),
        });
    }
    Timeline {
        end: app.timeline_end().max(1),
        lanes,
    }
}

fn review_column(
    app: &App,
    job: &JobProgress,
    review_run: Option<&JobProgress>,
) -> Option<ReviewColumn> {
    let state = job.review.as_ref();
    if state.is_none() && review_run.is_none() {
        return None;
    }
    let model = review_run.map_or_else(|| app.review_model().to_string(), |r| r.model.clone());
    let rows = review_run.map(|r| stream::rows(&r.log)).unwrap_or_default();
    let mut col = ReviewColumn {
        model,
        verdict: None,
        headline: String::new(),
        concerns: Vec::new(),
        diffs_checked: None,
        cost_usd: None,
        running: false,
        rows,
    };
    match state {
        Some(ReviewState::Requested) => {
            "queued: starts when the job finishes".clone_into(&mut col.headline);
        }
        Some(ReviewState::Running { .. }) | None => {
            "reading the diff".clone_into(&mut col.headline);
            col.running = true;
        }
        Some(ReviewState::Done {
            review, cost_usd, ..
        }) => {
            col.verdict = Some(review.verdict);
            col.headline = match review.verdict {
                ReviewVerdict::Pass => "pass".into(),
                ReviewVerdict::Concerns => format!("{} concerns", review.concerns.len()),
            };
            col.concerns = review.concerns.clone();
            col.diffs_checked = Some(review.diffs_checked);
            col.cost_usd = Some(*cost_usd);
        }
        Some(ReviewState::Failed {
            error, cost_usd, ..
        }) => {
            col.headline = format!("no verdict: {error}");
            col.cost_usd = Some(*cost_usd);
        }
        Some(ReviewState::Skipped { why }) => {
            col.headline = format!("skipped: {why}");
        }
    }
    Some(col)
}
