//! Mission control's state: the attention inbox, the change log, the session
//! wall and the telemetry snapshot, refreshed once per update.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use swarmux_application::{
    AlarmStore, AttentionService, Change, GateService, GateState, GuardService, JobFact,
    JobService, TelemetryService, TelemetrySettings, TelemetrySnapshot,
};
use swarmux_domain::gate::GateOutcome;
use swarmux_domain::telemetry::UsageReport;
use swarmux_domain::{
    AttentionId, AttentionItem, JobId, JobStatus, Millis, ReviewState, ReviewVerdict, Session,
    SessionId,
};

/// Changes kept for the change log.
pub const CHANGES_KEPT: usize = 40;
/// How often the telemetry snapshot is taken.
const SNAPSHOT_EVERY_MS: Millis = 1_000;
/// A feed is stale once its data is older than this many cadences.
const STALE_AFTER: Millis = 2;
/// How often a running session is expected to report its status line.
/// Claude Code refreshes it as the conversation moves; a long tool call
/// can hold it back, so this is generous.
const STATUSLINE_EVERY_MS: Millis = 60_000;

/// How often each telemetry feed refreshes, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    pub usage: Millis,
    pub system: Millis,
    pub git: Millis,
    pub github: Millis,
    /// `claude agents --json`.
    pub agents: Millis,
    /// Status lines of running sessions.
    pub statusline: Millis,
}

impl Cadence {
    pub fn from_settings(t: &TelemetrySettings) -> Cadence {
        let ms = |secs: u64| Millis::try_from(secs).unwrap_or(Millis::MAX / 1000) * 1000;
        Cadence {
            usage: ms(t.usage_secs),
            system: ms(t.system_secs),
            git: ms(t.git_secs),
            github: ms(t.github_secs),
            agents: ms(t.agents_secs),
            statusline: STATUSLINE_EVERY_MS,
        }
    }
}

impl Default for Cadence {
    fn default() -> Cadence {
        Cadence::from_settings(&TelemetrySettings::default())
    }
}

/// Which Mission control panel the arrow keys move in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Inbox,
    Wall,
}

/// What the home screen shows.
pub struct MissionState {
    /// Ranked, most urgent first.
    pub items: Vec<AttentionItem>,
    /// Newest first.
    pub changes: Vec<Change>,
    pub facts: Vec<JobFact>,
    /// Each job's latest done-gate verdict.
    pub gates: HashMap<JobId, GateState>,
    pub snapshot: TelemetrySnapshot,
    pub cadence: Cadence,
    /// The panel Tab last chose.
    pub pane: Pane,
    /// The selected tile of the wall, by place.
    pub wall_at: usize,
    selected: Option<AttentionId>,
    /// Where the selection was in the list, to stay near it when it goes.
    selected_at: usize,
    /// The user moved off the top: until then the selection follows the
    /// most urgent item as it changes.
    moved: bool,
    attention: AttentionService,
    feed: Option<TelemetryService>,
    snapshot_at: Option<Millis>,
    focused: Option<PathBuf>,
}

impl Default for MissionState {
    fn default() -> MissionState {
        MissionState {
            items: Vec::new(),
            changes: Vec::new(),
            facts: Vec::new(),
            gates: HashMap::new(),
            snapshot: TelemetrySnapshot::default(),
            cadence: Cadence::default(),
            pane: Pane::default(),
            wall_at: 0,
            selected: None,
            selected_at: 0,
            moved: false,
            attention: AttentionService::new(),
            feed: None,
            snapshot_at: None,
            focused: None,
        }
    }
}

impl MissionState {
    /// Keeps the inbox's memory (shelved items, tallies, the alarm log) in
    /// `store`.
    pub fn with_alarms(mut self, store: Rc<dyn AlarmStore>, now: Millis) -> MissionState {
        self.attention = std::mem::take(&mut self.attention).with_store(store, now);
        self
    }

    pub fn alarms(&self) -> &AttentionService {
        &self.attention
    }

    pub fn alarms_mut(&mut self) -> &mut AttentionService {
        &mut self.attention
    }

    /// Notes that the user has now looked at the selected item.
    pub fn attend_selected(&mut self, now: Millis) {
        if let Some(item) = self.selected_item().cloned() {
            self.attention.attend(&item, now);
        }
    }

    pub fn with_feed(mut self, feed: TelemetryService, cadence: Cadence) -> MissionState {
        self.feed = Some(feed);
        self.cadence = cadence;
        self
    }

    /// Re-reads everything from the services. `changes` and `items` come
    /// from elsewhere (the hooks, the watchdog). `cwd` is the folder the git
    /// and GitHub feeds should follow.
    pub fn refresh(
        &mut self,
        (guard, jobs, gate): (&GuardService, &JobService, Option<&GateService>),
        (hook_changes, items): (&[Change], Vec<AttentionItem>),
        now: Millis,
        cwd: Option<&Path>,
    ) {
        self.facts = jobs.facts();
        let mut escalations = gate.map(|g| g.escalations(&self.facts)).unwrap_or_default();
        escalations.extend(items);
        self.items = self.attention.collect_with(guard, jobs, escalations, now);
        let mut extra = hook_changes.to_vec();
        if let Some(g) = gate {
            extra.extend(g.changes());
            self.gates = g.states().clone();
        }
        self.changes = self.attention.changes(guard, jobs, &extra, CHANGES_KEPT);
        self.keep_selection();
        if let Some(feed) = &self.feed {
            if let Some(cwd) = cwd
                && self.focused.as_deref() != Some(cwd)
            {
                feed.focus(cwd);
                self.focused = Some(cwd.to_path_buf());
            }
            if self
                .snapshot_at
                .is_none_or(|at| now - at >= SNAPSHOT_EVERY_MS)
            {
                self.snapshot = feed.snapshot(jobs);
                self.snapshot_at = Some(now);
            }
        }
    }

    /// The selection stays on its item; when the item is gone it moves to
    /// the item now in its place.
    fn keep_selection(&mut self) {
        let found = self
            .selected
            .as_ref()
            .filter(|_| self.moved)
            .and_then(|id| self.items.iter().position(|i| &i.id == id));
        let at = found.unwrap_or_else(|| self.selected_at.min(self.items.len().saturating_sub(1)));
        self.selected = self.items.get(at).map(|i| i.id.clone());
        self.selected_at = at;
    }

    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.items.iter().position(|i| &i.id == id)
    }

    pub fn selected_item(&self) -> Option<&AttentionItem> {
        self.selected_index().and_then(|i| self.items.get(i))
    }

    /// Moves the selection by `delta` rows, stopping at the ends.
    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let at = self.selected_index().unwrap_or(0);
        let to = at
            .saturating_add_signed(delta)
            .min(self.items.len().saturating_sub(1));
        self.selected = self.items.get(to).map(|i| i.id.clone());
        self.selected_at = to;
        self.moved = to != 0;
    }

    /// Hides the selected item (not a held tool call). Returns whether it did.
    pub fn acknowledge_selected(&mut self) -> bool {
        let Some(item) = self.selected_item().cloned() else {
            return false;
        };
        let done = self.attention.acknowledge(&item);
        self.items.retain(|i| i.id != item.id || !done);
        self.keep_selection();
        done
    }

    /// Looking at a job counts as having read what it left in the inbox.
    pub fn acknowledge_job(&mut self, job: JobId) {
        self.attention.acknowledge_job(&self.items, job);
        self.items
            .retain(|i| i.job != Some(job) || i.kind.is_decision());
        self.keep_selection();
    }

    /// The stale or failing feeds, for the staleness tells.
    pub fn tells(&self, now: Millis) -> Vec<Tell> {
        let s = &self.snapshot;
        let c = &self.cadence;
        let mut out = Vec::new();
        let mut feed = |name: &'static str, enabled: bool, at: Option<Millis>, every, failed| {
            if !enabled {
                return;
            }
            match at {
                Some(at) if now - at > every * STALE_AFTER => out.push(Tell {
                    feed: name,
                    kind: TellKind::Old(now - at),
                }),
                None if failed => out.push(Tell {
                    feed: name,
                    kind: TellKind::Failed,
                }),
                _ => {}
            }
        };
        feed(
            "usage",
            s.usage.enabled,
            s.usage.updated_at,
            c.usage,
            s.usage.error.is_some(),
        );
        feed(
            "system",
            s.system.enabled,
            s.system.updated_at,
            c.system,
            s.system.error.is_some(),
        );
        feed("git", s.git.enabled, s.git.updated_at, c.git, false);
        feed(
            "github",
            s.github.enabled,
            s.github.updated_at,
            c.github,
            false,
        );
        feed(
            "agents",
            s.external.enabled,
            s.external.updated_at,
            c.agents,
            s.external.error.is_some(),
        );
        if let Some(tell) = self.statusline_tell(now) {
            out.push(tell);
        }
        out
    }

    /// Status lines are expected only while a job runs. They are late once
    /// the newest one (or, before any arrived, the longest-running job's
    /// start) is older than two cadences.
    fn statusline_tell(&self, now: Millis) -> Option<Tell> {
        let running: Vec<Millis> = self
            .facts
            .iter()
            .filter(|f| f.status == JobStatus::Running)
            .filter_map(|f| f.started_at)
            .collect();
        let earliest = running.iter().min()?;
        let newest = self
            .snapshot
            .statuslines
            .iter()
            .map(|v| v.observed_at)
            .max()
            .map_or(*earliest, |seen| seen.max(*earliest));
        let age = now - newest;
        (age > self.cadence.statusline * STALE_AFTER).then_some(Tell {
            feed: "statusline",
            kind: TellKind::Old(age),
        })
    }

    /// The usage report, when the transcripts feed has one.
    pub fn usage(&self) -> Option<&UsageReport> {
        self.snapshot.usage.data.as_ref()
    }
}

/// A feed whose data can't be trusted at face value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tell {
    pub feed: &'static str,
    pub kind: TellKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TellKind {
    /// The data is this old, in milliseconds.
    Old(Millis),
    /// Nothing could be read.
    Failed,
}

/// Where a session stands, for the wall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    Running,
    /// A running job whose tool call waits for the user.
    Waiting,
    Queued,
    Done,
    Failed,
    Idle,
}

/// One session on the wall.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub session: SessionId,
    pub name: String,
    pub model: String,
    pub permission_mode: String,
    pub lean: bool,
    pub state: TileState,
    /// The latest prompt's first line.
    pub task: String,
    pub context: Option<f64>,
    /// Tokens per minute, oldest first.
    pub flow: Vec<f64>,
    /// `None` while a job runs and nothing has priced it yet.
    pub cost: Option<f64>,
    pub elapsed_ms: Option<Millis>,
    pub job: Option<JobId>,
    /// Todos done and all, when the lead keeps a list.
    pub todos: Option<(u32, u32)>,
    /// Done-gate, review and guard state as short chips.
    pub chips: Vec<Chip>,
    /// The session's workspace and its repositories, when it has one.
    pub workspace: Option<String>,
}

/// How a chip reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Info,
}

/// A short fact about a session: `✓ cargo test`, `review pass`, `loop stop`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub text: String,
    pub tone: Tone,
}

/// The done-gate's chips: its state first (cards cut the row), then one
/// per check, then what else it found.
pub(crate) fn gate_chips(g: &GateState) -> Vec<Chip> {
    let chip = |text: String, tone| Chip { text, tone };
    let mut out = Vec::new();
    match g.outcome {
        GateOutcome::Block => out.push(chip(
            format!("gate {}/{}", g.blocks, g.max_blocks),
            Tone::Warn,
        )),
        GateOutcome::Escalate => out.push(chip("gate gave up".into(), Tone::Bad)),
        GateOutcome::Pass => {}
    }
    if let Some(problem) = &g.problem {
        let what = problem.split(':').next().unwrap_or("gate");
        out.push(chip(format!("{what} invalid"), Tone::Bad));
    }
    for (name, passed, flaky) in &g.checks {
        out.push(match (passed, flaky) {
            (true, false) => chip(name.clone(), Tone::Good),
            (true, true) => chip(format!("{name} flaky"), Tone::Warn),
            (false, _) => chip(name.clone(), Tone::Bad),
        });
    }
    if g.protected_changed > 0 {
        out.push(chip("protected changed".into(), Tone::Bad));
    }
    if g.evidence_gaps > 0 {
        out.push(chip("evidence missing".into(), Tone::Warn));
    }
    if g.tampering > 0 {
        out.push(chip("tests weakened".into(), Tone::Bad));
    }
    match g.base_suite {
        Some(true) => out.push(chip("base suite ok".into(), Tone::Good)),
        Some(false) => out.push(chip("base suite failed".into(), Tone::Bad)),
        None => {}
    }
    // Hidden checks are counted, never named; when they exist and did not
    // run, that is said with why, never left as a missing chip.
    if let Some(h) = g.hidden.filter(|h| h.ran > 0) {
        let tone = if h.clean() { Tone::Good } else { Tone::Bad };
        out.push(chip(format!("hidden {}", h.text()), tone));
    }
    if let Some(note) = &g.notes.hidden_not_run {
        out.push(chip(
            note.replacen("hidden checks: ", "hidden: ", 1),
            Tone::Warn,
        ));
    }
    if g.notes.ran_in == Some(swarmux_domain::gate::RanIn::Worktree) {
        out.push(chip("ran in the worktree".into(), Tone::Warn));
    }
    if g.notes.substrate > 0 {
        out.push(chip("test harness changed".into(), Tone::Bad));
    }
    if g.notes.dependencies > 0 {
        out.push(chip("dependencies changed".into(), Tone::Warn));
    }
    for (name, n, of) in &g.notes.flaky {
        out.push(chip(format!("{name} flaky {n}/{of}"), Tone::Warn));
    }
    out
}

fn chips_of(f: &JobFact, gate: Option<&GateState>, held: bool) -> Vec<Chip> {
    let chip = |text: &str, tone| Chip {
        text: text.to_string(),
        tone,
    };
    let mut out = gate.map(gate_chips).unwrap_or_default();
    out.extend(f.review.as_ref().map(review_chip));
    if held {
        out.push(chip("held call", Tone::Warn));
    }
    if f.loop_stops > 0 {
        out.push(chip("loop stop", Tone::Bad));
    }
    if f.agents_running > 0 {
        out.push(Chip {
            text: format!("{} agents", f.agents_running),
            tone: Tone::Info,
        });
    }
    out
}

/// The review as a chip: pending, passed, concerned or failed.
pub(crate) fn review_chip(review: &ReviewState) -> Chip {
    let chip = |text: String, tone| Chip { text, tone };
    match review {
        ReviewState::Requested | ReviewState::Running { .. } => {
            chip("review pending".into(), Tone::Info)
        }
        ReviewState::Done { review, .. } if review.verdict == ReviewVerdict::Concerns => chip(
            format!(
                "review {} concern{}",
                review.concerns.len(),
                if review.concerns.len() == 1 { "" } else { "s" }
            ),
            Tone::Warn,
        ),
        ReviewState::Done { .. } => chip("review pass".into(), Tone::Good),
        ReviewState::Failed { .. } => chip("review failed".into(), Tone::Bad),
        ReviewState::Skipped { .. } => chip("review skipped".into(), Tone::Info),
    }
}

/// The wall, in a stable order: by when the session was created, then by
/// id. State never moves a tile.
pub fn wall_tiles(
    sessions: &[Session],
    facts: &[JobFact],
    gates: &HashMap<JobId, GateState>,
    waiting: &[JobId],
    usage: Option<&UsageReport>,
    now: Millis,
) -> Vec<Tile> {
    let mut ordered: Vec<&Session> = sessions.iter().collect();
    ordered.sort_by_key(|s| (s.created_at, s.id.to_string()));
    ordered
        .into_iter()
        .map(|s| {
            let latest = facts
                .iter()
                .filter(|f| f.session_id == s.id && f.review_of.is_none())
                .max_by_key(|f| (f.created_at, f.job_id));
            let seen = usage.and_then(|u| {
                let id = s.id.to_string();
                u.sessions.iter().find(|x| x.session_id == id)
            });
            let state = match latest.map(|f| f.status) {
                Some(JobStatus::Running) if latest.is_some_and(|f| waiting.contains(&f.job_id)) => {
                    TileState::Waiting
                }
                Some(JobStatus::Running) => TileState::Running,
                Some(JobStatus::Queued) => TileState::Queued,
                Some(JobStatus::Done) => TileState::Done,
                Some(JobStatus::Failed | JobStatus::Interrupted) => TileState::Failed,
                Some(JobStatus::Cancelled) | None => TileState::Idle,
            };
            let elapsed_ms = latest.and_then(|f| {
                let start = f.started_at?;
                Some((f.finished_at.unwrap_or(now) - start).max(0))
            });
            Tile {
                session: s.id,
                name: s.name.clone(),
                model: latest.map_or_else(|| s.model.clone(), |f| f.model.clone()),
                permission_mode: s.permission_mode.clone(),
                lean: s.lean,
                state,
                task: latest
                    .and_then(|f| f.prompt.lines().next())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                context: latest
                    .and_then(|f| f.context)
                    .or_else(|| seen.and_then(|x| x.context.ratio())),
                flow: latest
                    .map(|f| f.flow.iter().map(|v| *v as f64).collect())
                    .unwrap_or_default(),
                cost: match latest {
                    Some(f) if f.status.is_finished() || f.cost_usd > 0.0 => Some(f.cost_usd),
                    _ => seen.map(|x| x.spend.cost),
                },
                elapsed_ms,
                job: latest.map(|f| f.job_id),
                todos: latest
                    .filter(|f| f.todos_total > 0)
                    .map(|f| (f.todos_done, f.todos_total)),
                chips: latest
                    .map(|f| chips_of(f, gates.get(&f.job_id), state == TileState::Waiting))
                    .unwrap_or_default(),
                workspace: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn session(name: &str, created: Millis) -> Session {
        Session::new(
            SessionId::random(),
            name,
            PathBuf::from("/w"),
            "sonnet",
            created,
        )
        .expect("valid session")
    }

    fn fact(job: i64, s: &Session, status: JobStatus) -> JobFact {
        JobFact {
            job_id: JobId(job),
            session_id: s.id,
            session_name: s.name.clone(),
            prompt: "do it\nplease".into(),
            model: "opus".into(),
            status,
            created_at: 10 + job,
            started_at: Some(100),
            last_event_at: Some(100),
            finished_at: None,
            error: None,
            result: None,
            review: None,
            review_of: None,
            cost_usd: 1.5,
            context: Some(0.4),
            cache_ratio: None,
            flow: vec![1, 2],
            todos_done: 2,
            todos_total: 5,
            loop_stops: 0,
            agents_running: 1,
            session_state: None,
        }
    }

    #[test]
    fn tiles_keep_their_order_whatever_the_state() {
        let a = session("alpha", 1);
        let b = session("beta", 2);
        let c = session("gamma", 3);
        // Listed by last use, as the app does: not the wall's order.
        let listed = [c.clone(), a.clone(), b.clone()];
        let names = |t: &[Tile]| t.iter().map(|t| t.name.clone()).collect::<Vec<_>>();

        let idle = wall_tiles(&listed, &[], &HashMap::new(), &[], None, 500);
        assert_eq!(names(&idle), ["alpha", "beta", "gamma"]);

        let busy = [
            fact(1, &c, JobStatus::Running),
            fact(2, &b, JobStatus::Failed),
            fact(3, &a, JobStatus::Done),
        ];
        let tiles = wall_tiles(&listed, &busy, &HashMap::new(), &[], None, 500);
        assert_eq!(names(&tiles), names(&idle));
        let states: Vec<_> = tiles.iter().map(|t| t.state).collect();
        assert_eq!(
            states,
            [TileState::Done, TileState::Failed, TileState::Running]
        );

        let moved = [
            fact(1, &c, JobStatus::Done),
            fact(2, &b, JobStatus::Running),
            fact(3, &a, JobStatus::Queued),
        ];
        let tiles = wall_tiles(&listed, &moved, &HashMap::new(), &[JobId(2)], None, 500);
        assert_eq!(names(&tiles), names(&idle));
        assert_eq!(tiles[1].state, TileState::Waiting);
    }

    #[test]
    fn tile_facts_come_from_the_latest_job() {
        let a = session("alpha", 1);
        let mut old = fact(1, &a, JobStatus::Done);
        old.prompt = "old".into();
        let new = fact(2, &a, JobStatus::Running);
        let tile = &wall_tiles(&[a], &[old, new], &HashMap::new(), &[], None, 1_100)[0];
        assert_eq!(tile.task, "do it");
        assert_eq!(tile.model, "opus");
        assert_eq!(tile.elapsed_ms, Some(1_000));
        assert_eq!(tile.context, Some(0.4));
        assert_eq!(tile.flow, [1.0, 2.0]);
        assert_eq!(tile.job, Some(JobId(2)));
    }

    #[test]
    fn selection_stays_on_its_item_and_then_on_its_place() {
        use swarmux_domain::AttentionId;
        let mk = |k: &str| AttentionItem::stalled(JobId(1), SessionId::random(), k, 0);
        let mut m = MissionState {
            items: (0..4)
                .map(|i| {
                    let mut it = mk("t");
                    it.id = AttentionId::new(format!("k{i}"));
                    it
                })
                .collect(),
            ..MissionState::default()
        };
        m.keep_selection();
        m.move_selection(1);
        m.move_selection(1);
        assert_eq!(
            m.selected_item().map(|i| i.id.to_string()),
            Some("k2".into())
        );
        // An item above goes: the selection stays on k2.
        m.items.remove(0);
        m.keep_selection();
        assert_eq!(
            m.selected_item().map(|i| i.id.to_string()),
            Some("k2".into())
        );
        // k2 goes: the selection takes its place.
        m.items.retain(|i| i.id.as_str() != "k2");
        m.keep_selection();
        assert_eq!(
            m.selected_item().map(|i| i.id.to_string()),
            Some("k3".into())
        );
        m.move_selection(-9);
        assert_eq!(m.selected_index(), Some(0));
        m.move_selection(9);
        assert_eq!(m.selected_index(), Some(1));
    }

    #[test]
    fn feeds_older_than_two_cadences_tell() {
        let mut m = MissionState::default();
        m.snapshot.usage.enabled = true;
        m.snapshot.usage.updated_at = Some(0);
        m.snapshot.system.enabled = true;
        m.snapshot.system.updated_at = Some(60_000);
        m.snapshot.git.enabled = false;
        m.snapshot.github.enabled = true;
        m.snapshot.github.updated_at = None;
        let now = 61_000;
        // usage: every 15 s, 61 s old. system: every 1 s, 1 s old: fine.
        assert_eq!(
            m.tells(now),
            [Tell {
                feed: "usage",
                kind: TellKind::Old(61_000)
            }]
        );
        m.snapshot.usage.updated_at = Some(50_000);
        assert!(m.tells(now).is_empty(), "11 s is within two cadences");
        m.snapshot.usage.updated_at = None;
        m.snapshot.usage.error = Some("no transcripts".into());
        assert_eq!(m.tells(now)[0].kind, TellKind::Failed);
    }

    #[test]
    fn the_agents_feed_tells_like_the_others() {
        let mut m = MissionState::default();
        m.snapshot.external.enabled = true;
        m.snapshot.external.updated_at = Some(0);
        // agents: every 15 s, 31 s old.
        assert_eq!(
            m.tells(31_000),
            [Tell {
                feed: "agents",
                kind: TellKind::Old(31_000)
            }]
        );
        m.snapshot.external.updated_at = None;
        m.snapshot.external.error = Some("claude agents failed".into());
        assert_eq!(m.tells(31_000)[0].kind, TellKind::Failed);
        m.snapshot.external.enabled = false;
        assert!(m.tells(31_000).is_empty(), "turned off: no tell");
    }

    #[test]
    fn status_lines_are_expected_only_while_a_job_runs() {
        let s = session("a", 1);
        let mut m = MissionState::default();
        let line = |at| swarmux_application::StatuslineView {
            session_id: "c1".into(),
            job: Some(JobId(1)),
            model: None,
            cwd: None,
            cost_usd: None,
            context_used: None,
            cache: None,
            pr: None,
            observed_at: at,
        };
        // Nothing runs: silence is fine.
        m.facts = vec![fact(1, &s, JobStatus::Done)];
        assert!(m.tells(1_000_000).is_empty());
        // A job started at 100 and nothing reported for two minutes.
        m.facts = vec![fact(1, &s, JobStatus::Running)];
        assert!(m.tells(100 + 120_000).is_empty(), "within two cadences");
        assert_eq!(
            m.tells(100 + 120_001),
            [Tell {
                feed: "statusline",
                kind: TellKind::Old(120_001)
            }]
        );
        // A fresh line clears it.
        m.snapshot.statuslines = vec![line(100_000)];
        assert!(m.tells(130_000).is_empty());
        assert_eq!(m.tells(230_001)[0].kind, TellKind::Old(130_001));
    }

    #[test]
    fn gate_chips_lead_with_the_gate_state() {
        let s = session("a", 1);
        let f = JobFact {
            agents_running: 0,
            ..fact(1, &s, JobStatus::Running)
        };
        let mut gate = GateState {
            outcome: GateOutcome::Block,
            blocks: 2,
            max_blocks: 3,
            checks: vec![
                ("cargo test".into(), false, false),
                ("lint".into(), true, true),
            ],
            protected_changed: 1,
            evidence_gaps: 1,
            hidden: None,
            tampering: 0,
            checks_weakened: false,
            base_suite: None,
            digest: String::new(),
            problem: None,
            at: 0,
            notes: Default::default(),
        };
        let gates = HashMap::from([(JobId(1), gate.clone())]);
        let tile = &wall_tiles(
            std::slice::from_ref(&s),
            std::slice::from_ref(&f),
            &gates,
            &[],
            None,
            500,
        )[0];
        let chips: Vec<(&str, Tone)> = tile
            .chips
            .iter()
            .map(|c| (c.text.as_str(), c.tone))
            .collect();
        assert_eq!(
            chips,
            [
                ("gate 2/3", Tone::Warn),
                ("cargo test", Tone::Bad),
                ("lint flaky", Tone::Warn),
                ("protected changed", Tone::Bad),
                ("evidence missing", Tone::Warn),
            ]
        );
        gate.outcome = GateOutcome::Pass;
        gate.checks = vec![("cargo test".into(), true, false)];
        gate.protected_changed = 0;
        gate.evidence_gaps = 0;
        let gates = HashMap::from([(JobId(1), gate)]);
        let tile = &wall_tiles(&[s], &[f], &gates, &[], None, 500)[0];
        assert_eq!(
            tile.chips,
            [Chip {
                text: "cargo test".into(),
                tone: Tone::Good
            }]
        );
    }

    #[test]
    fn hidden_checks_show_as_a_count_and_tampering_as_a_chip() {
        let gate = GateState {
            outcome: GateOutcome::Block,
            blocks: 1,
            max_blocks: 3,
            checks: vec![("cargo test".into(), true, false)],
            protected_changed: 0,
            evidence_gaps: 0,
            hidden: Some(swarmux_domain::verifier::HiddenTally { ran: 3, passed: 2 }),
            tampering: 1,
            checks_weakened: false,
            base_suite: None,
            digest: String::new(),
            problem: None,
            at: 0,
            notes: Default::default(),
        };
        let chips: Vec<(String, Tone)> = gate_chips(&gate)
            .into_iter()
            .map(|c| (c.text, c.tone))
            .collect();
        assert_eq!(
            chips,
            [
                ("gate 1/3".to_string(), Tone::Warn),
                ("cargo test".to_string(), Tone::Good),
                ("tests weakened".to_string(), Tone::Bad),
                ("hidden 2/3".to_string(), Tone::Bad),
            ]
        );
        let passed = GateState {
            hidden: Some(swarmux_domain::verifier::HiddenTally { ran: 3, passed: 3 }),
            tampering: 0,
            checks_weakened: false,
            base_suite: None,
            outcome: GateOutcome::Pass,
            ..gate
        };
        assert!(
            gate_chips(&passed)
                .iter()
                .any(|c| c.text == "hidden 3/3" && c.tone == Tone::Good)
        );
    }
}
