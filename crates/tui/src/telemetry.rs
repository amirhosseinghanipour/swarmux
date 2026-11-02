//! View models for the telemetry panels (usage limits, burn rate, git, …).
//!
//! The interface doesn't collect any of this itself: whoever owns a feed
//! maps its snapshot into a [`Telemetry`] and hands it to
//! [`crate::App::set_telemetry`]. A field left `None` renders as the
//! panel's "waiting for data" state. Ratios are `0.0..=1.0`, series are
//! oldest first, and text is shown as given.

/// A labelled gauge, e.g. `("5h window", 0.42, "resets 14:00")`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gauge {
    pub label: String,
    /// Fill, `0.0..=1.0`.
    pub ratio: f64,
    /// Shown right of the bar, e.g. `"42%"` or `"84k / 200k"`.
    pub detail: String,
}

impl Gauge {
    pub fn new(label: impl Into<String>, ratio: f64, detail: impl Into<String>) -> Gauge {
        Gauge {
            label: label.into(),
            ratio,
            detail: detail.into(),
        }
    }
}

/// A named series of samples, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    pub name: String,
    pub points: Vec<f64>,
}

impl Series {
    pub fn new(name: impl Into<String>, points: Vec<f64>) -> Series {
        Series {
            name: name.into(),
            points,
        }
    }
}

/// Where something stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activity {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

/// Plan usage limits: one gauge per window (5-hour, weekly, Opus weekly…).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageView {
    pub gauges: Vec<Gauge>,
    /// e.g. `"resets in 2h 14m"`.
    pub note: Option<String>,
}

/// Spend and how fast it grows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BurnView {
    /// USD spent today.
    pub today_usd: f64,
    /// USD per hour at the current pace.
    pub per_hour_usd: f64,
    /// Spend per interval (e.g. per minute), oldest first.
    pub series: Vec<f64>,
    /// e.g. `"limit in ~3h at this pace"`.
    pub projection: Option<String>,
}

/// Tokens per interval, e.g. `in`, `out` and `cache` series.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TokenFlowView {
    pub series: Vec<Series>,
    /// The y-axis unit, e.g. `"tok/min"`.
    pub unit: String,
}

/// Context windows: one gauge per session or agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextView {
    pub gauges: Vec<Gauge>,
}

/// One node of the agent tree, in display order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentNode {
    /// 0 for the lead, 1 for its subagents, …
    pub depth: u8,
    pub name: String,
    pub model: Option<String>,
    pub state: Activity,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentTreeView {
    pub nodes: Vec<AgentNode>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TodoState {
    #[default]
    Pending,
    Active,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Todo {
    pub text: String,
    pub state: TodoState,
}

/// The lead's todo list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodoView {
    pub items: Vec<Todo>,
}

/// Tool calls: how many per tool, and the latest few as text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolView {
    /// `(tool, calls)`, any order.
    pub counts: Vec<(String, f64)>,
    /// Latest calls, oldest first, e.g. `"Bash cargo test"`.
    pub recent: Vec<String>,
}

/// Files touched: `(path, edits)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesView {
    pub files: Vec<(String, u32)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitView {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    /// e.g. `"a1b2c3d feat: health endpoint"`.
    pub last_commit: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    #[default]
    Pending,
    Pass,
    Fail,
    Skipped,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub detail: String,
}

/// CI checks and the pull request they belong to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CiView {
    /// e.g. `"#42 add /health · open"`.
    pub pr: Option<String>,
    pub checks: Vec<Check>,
}

/// The machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemView {
    /// CPU busy, `0.0..=1.0`.
    pub cpu: f64,
    /// Memory used, `0.0..=1.0`.
    pub mem: f64,
    /// CPU busy samples, oldest first.
    pub cpu_series: Vec<f64>,
    /// e.g. `"9.1 / 32 GB"`.
    pub mem_detail: String,
    /// e.g. `"claude ×3 · 1.2 GB"`.
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallTile {
    pub name: String,
    pub model: String,
    pub state: Activity,
    pub cost_usd: f64,
    /// Context used, `0.0..=1.0`.
    pub context: f64,
}

/// Every session at a glance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WallView {
    pub sessions: Vec<WallTile>,
}

/// Jobs (or tokens) per day, oldest first; the last entry is today.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalendarView {
    pub days: Vec<u32>,
    /// Today's weekday, 0 = Monday.
    pub today_weekday: u8,
    /// e.g. `"412 jobs in the last year"`.
    pub caption: Option<String>,
}

/// Key facts about the environment: `("claude", "2.1.3")`, …
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvView {
    pub pairs: Vec<(String, String)>,
}

/// Everything the telemetry panels show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Telemetry {
    pub usage: Option<UsageView>,
    pub burn: Option<BurnView>,
    pub tokens: Option<TokenFlowView>,
    pub context: Option<ContextView>,
    pub agents: Option<AgentTreeView>,
    pub todos: Option<TodoView>,
    pub tools: Option<ToolView>,
    pub files: Option<FilesView>,
    pub git: Option<GitView>,
    pub ci: Option<CiView>,
    pub system: Option<SystemView>,
    pub wall: Option<WallView>,
    pub calendar: Option<CalendarView>,
    pub env: Option<EnvView>,
}

#[cfg(test)]
pub(crate) mod sample {
    //! A full snapshot for render tests.
    use super::*;

    pub(crate) fn telemetry() -> Telemetry {
        let wave = |n: usize, k: f64| -> Vec<f64> {
            (0..n)
                .map(|i| (k * (1.0 + (i as f64 / 3.0).sin())).max(0.0))
                .collect()
        };
        Telemetry {
            usage: Some(UsageView {
                gauges: vec![
                    Gauge::new("5h window", 0.42, "42%"),
                    Gauge::new("weekly", 0.71, "71%"),
                    Gauge::new("opus weekly", 0.93, "93%"),
                ],
                note: Some("resets in 2h 14m".into()),
            }),
            burn: Some(BurnView {
                today_usd: 12.84,
                per_hour_usd: 3.2,
                series: wave(60, 0.05),
                projection: Some("limit in ~3h at this pace".into()),
            }),
            tokens: Some(TokenFlowView {
                series: vec![
                    Series::new("in", wave(40, 900.0)),
                    Series::new("out", wave(40, 300.0)),
                    Series::new("cache", wave(40, 4000.0)),
                ],
                unit: "tok/min".into(),
            }),
            context: Some(ContextView {
                gauges: vec![
                    Gauge::new("demo-shop", 0.31, "62k / 200k"),
                    Gauge::new("api", 0.82, "164k / 200k"),
                ],
            }),
            agents: Some(AgentTreeView {
                nodes: vec![
                    AgentNode {
                        depth: 0,
                        name: "lead".into(),
                        model: Some("opus".into()),
                        state: Activity::Running,
                    },
                    AgentNode {
                        depth: 1,
                        name: "worker".into(),
                        model: Some("sonnet".into()),
                        state: Activity::Done,
                    },
                    AgentNode {
                        depth: 1,
                        name: "scout".into(),
                        model: Some("haiku".into()),
                        state: Activity::Failed,
                    },
                ],
            }),
            todos: Some(TodoView {
                items: vec![
                    Todo {
                        text: "map the routes".into(),
                        state: TodoState::Done,
                    },
                    Todo {
                        text: "write the handler".into(),
                        state: TodoState::Active,
                    },
                    Todo {
                        text: "add tests".into(),
                        state: TodoState::Pending,
                    },
                ],
            }),
            tools: Some(ToolView {
                counts: vec![
                    ("Bash".into(), 12.0),
                    ("Edit".into(), 7.0),
                    ("Read".into(), 21.0),
                ],
                recent: vec!["Bash cargo test".into(), "Edit src/app.rs".into()],
            }),
            files: Some(FilesView {
                files: vec![("src/app.rs".into(), 4), ("tests/health.rs".into(), 2)],
            }),
            git: Some(GitView {
                branch: "feat/health".into(),
                ahead: 2,
                behind: 0,
                staged: 1,
                modified: 3,
                untracked: 1,
                last_commit: Some("a1b2c3d feat: health endpoint".into()),
            }),
            ci: Some(CiView {
                pr: Some("#42 add /health · open".into()),
                checks: vec![
                    Check {
                        name: "test".into(),
                        state: CheckState::Pass,
                        detail: "2m 10s".into(),
                    },
                    Check {
                        name: "clippy".into(),
                        state: CheckState::Fail,
                        detail: "1 warning".into(),
                    },
                    Check {
                        name: "release".into(),
                        state: CheckState::Pending,
                        detail: String::new(),
                    },
                ],
            }),
            system: Some(SystemView {
                cpu: 0.37,
                mem: 0.58,
                cpu_series: wave(50, 0.3),
                mem_detail: "18.6 / 32 GB".into(),
                note: Some("claude ×3".into()),
            }),
            wall: Some(WallView {
                sessions: vec![
                    WallTile {
                        name: "demo-shop".into(),
                        model: "opus".into(),
                        state: Activity::Running,
                        cost_usd: 0.41,
                        context: 0.3,
                    },
                    WallTile {
                        name: "api".into(),
                        model: "haiku".into(),
                        state: Activity::Idle,
                        cost_usd: 0.02,
                        context: 0.1,
                    },
                ],
            }),
            calendar: Some(CalendarView {
                days: (0..120).map(|i| (i * 7 % 11) as u32).collect(),
                today_weekday: 2,
                caption: Some("412 jobs in 120 days".into()),
            }),
            env: Some(EnvView {
                pairs: vec![
                    ("claude".into(), "2.1.3".into()),
                    ("os".into(), "windows 11".into()),
                    ("shell".into(), "pwsh 7.5".into()),
                ],
            }),
        }
    }
}
