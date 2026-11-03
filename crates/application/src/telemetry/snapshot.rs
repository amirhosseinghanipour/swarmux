//! Plain data the dashboard stations render. Everything unknown is `None`.

use swarmux_domain::Millis;
use swarmux_domain::telemetry::{
    Environment, ExternalSession, FileTouch, FlowBucket, LimitProjection, LimitSource, LimitState,
    ToolCall, ToolStat, UsageReport, local_clock,
};

use super::truth::{InstructionsView, StatuslineView};

/// Everything the stations show, in one value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TelemetrySnapshot {
    /// When the snapshot was assembled.
    pub taken_at: Millis,
    /// Local time offset in minutes (days and clock times use it).
    pub offset_min: i32,
    /// Spend, burn, billing blocks, calendar, session wall and machine-wide
    /// token flow, from Claude Code transcripts.
    pub usage: Collected<UsageReport>,
    /// CPU, memory, network, disk, battery, uptime and `claude` processes.
    pub system: Collected<SystemStats>,
    /// The focused folder's repository.
    pub git: Collected<GitStatus>,
    /// Open pull requests and CI of the focused repository (`gh`).
    pub github: Collected<GithubStatus>,
    /// Usage limits, shortest window first (from swarmux jobs; kept across runs).
    pub limits: Vec<LimitView>,
    /// Token flow and tool calls across swarmux jobs of this run.
    pub activity: ActivityView,
    /// Latest environment per Claude session swarmux ran, newest first.
    pub environments: Vec<EnvironmentView>,
    /// The latest status line of each session that reports one, newest first:
    /// context, cost, prompt cache and pull request.
    pub statuslines: Vec<StatuslineView>,
    /// Claude Code sessions swarmux didn't start (`claude agents`).
    pub external: Collected<Vec<ExternalSession>>,
    /// Instruction files loaded by each session.
    pub instructions: Vec<InstructionsView>,
}

/// One collector's latest result and health.
#[derive(Clone, Debug, PartialEq)]
pub struct Collected<T> {
    /// Turned on in `[telemetry]`.
    pub enabled: bool,
    /// The latest successful reading.
    pub data: Option<T>,
    /// When `data` was read.
    pub updated_at: Option<Millis>,
    /// Why the latest attempt failed (the previous data is kept).
    pub error: Option<String>,
    /// What the user can do about it ("run `gh auth login`").
    pub hint: Option<String>,
    /// How long the latest collection took.
    pub took_ms: Option<u64>,
}

impl<T> Default for Collected<T> {
    fn default() -> Collected<T> {
        Collected {
            enabled: false,
            data: None,
            updated_at: None,
            error: None,
            hint: None,
            took_ms: None,
        }
    }
}

impl<T> Collected<T> {
    /// An enabled source with nothing read yet.
    pub fn pending() -> Collected<T> {
        Collected {
            enabled: true,
            ..Collected::default()
        }
    }

    /// Older than `max_age`, or never read.
    pub fn is_stale(&self, now: Millis, max_age: Millis) -> bool {
        self.updated_at.is_none_or(|at| now - at > max_age)
    }
}

/// btop-style machine figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemStats {
    /// Total CPU use, 0 to 100.
    pub cpu: f64,
    /// Per core, 0 to 100.
    pub cores: Vec<f64>,
    /// Total CPU, oldest first (up to 60 samples).
    pub cpu_history: Vec<f64>,
    /// Per core, oldest first (up to 60 samples each).
    pub core_history: Vec<Vec<f64>>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// Bytes per second over all interfaces; `None` until two readings.
    pub net_rx_per_sec: Option<u64>,
    pub net_tx_per_sec: Option<u64>,
    /// (rx, tx) bytes per second, oldest first (up to 60 samples).
    pub net_history: Vec<(u64, u64)>,
    /// The disk holding the home folder.
    pub disk: Option<DiskUsage>,
    pub battery: Option<Battery>,
    pub uptime_secs: u64,
    /// Claude Code processes, most memory first.
    pub claude_processes: Vec<ProcessStat>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    /// 0 to 100.
    pub percent: f64,
    pub charging: bool,
    /// On mains power.
    pub plugged_in: bool,
    /// Estimated time left on battery, when the OS knows.
    pub secs_left: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessStat {
    pub pid: u32,
    pub name: String,
    /// Resident memory in bytes.
    pub rss: u64,
    /// CPU use, 0 to 100 × cores.
    pub cpu: f64,
}

/// Working tree state of one repository.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitStatus {
    /// The folder asked about.
    pub cwd: String,
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Short commit id of HEAD; `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    /// Uncommitted changes against HEAD (staged and not).
    pub diff: DiffStat,
    /// Newest first, at most five.
    pub commits: Vec<Commit>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub at: Millis,
}

/// GitHub state of the focused repository.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GithubStatus {
    /// `owner/name`.
    pub repo: Option<String>,
    pub pull_requests: Vec<PullRequest>,
    /// The latest workflow run of the current branch.
    pub ci: Option<CiRun>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// `OPEN`, `CLOSED`, `MERGED`.
    pub state: String,
    /// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`; `None` if no review is required.
    pub review_decision: Option<String>,
    pub draft: bool,
    pub branch: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiRun {
    pub workflow: String,
    pub branch: String,
    /// `queued`, `in_progress`, `completed`, …
    pub status: String,
    /// `success`, `failure`, `cancelled`, …, once completed.
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

/// One usage-limit window as the stations show it.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitView {
    /// `five_hour`, `seven_day`, …
    pub window: String,
    pub status: String,
    /// 0 to 1.
    pub utilization: Option<f64>,
    pub resets_at: Option<Millis>,
    /// Time until the reset (0 once passed).
    pub resets_in: Option<Millis>,
    pub surpassed_threshold: Option<f64>,
    /// When Claude Code reported it.
    pub observed_at: Millis,
    /// When the current pace reaches the limit, if before the reset.
    pub projection: Option<LimitProjection>,
    /// `projection` as local `HH:MM`: "at this pace you hit the limit at 16:40".
    pub hits_at_clock: Option<String>,
    /// The window has reset since the reading.
    pub stale: bool,
    /// Where the reading came from; the freshest source wins.
    pub source: LimitSource,
}

impl LimitView {
    pub fn new(state: &LimitState, now: Millis, offset_min: i32, source: LimitSource) -> LimitView {
        let projection = state.projection(now);
        LimitView {
            window: state.limit.window.clone(),
            status: state.limit.status.clone(),
            utilization: state.limit.utilization,
            resets_at: state.limit.resets_at,
            resets_in: state.resets_in(now),
            surpassed_threshold: state.limit.surpassed_threshold,
            observed_at: state.observed_at,
            hits_at_clock: projection.map(|p| local_clock(p.hits_at, offset_min)),
            projection,
            stale: state.is_stale(now),
            source,
        }
    }
}

/// Activity across swarmux jobs of this run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActivityView {
    /// Tokens per minute, last hour, oldest first.
    pub flow: Vec<FlowBucket>,
    pub cache_hit_ratio: Option<f64>,
    /// Per tool over the last hour, most used first.
    pub tools: Vec<ToolStat>,
    /// Latest tool calls, newest first.
    pub ticker: Vec<ToolCall>,
    /// Most read and edited files.
    pub files: Vec<FileTouch>,
}

/// A session's environment and when it was reported.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentView {
    pub environment: Environment,
    pub seen_at: Millis,
}
