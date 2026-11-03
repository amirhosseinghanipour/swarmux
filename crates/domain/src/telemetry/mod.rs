//! Telemetry for the dashboard's stations: spend, limits, token flow, tools,
//! todos, environments and context. Pure functions over data passed in; the
//! local UTC offset and "now" always come from the caller.

mod activity;
mod blocks;
mod context;
mod environment;
mod external;
mod flow;
mod instructions;
mod limits;
mod pricing;
mod spend;
mod statusline;
mod time;
mod todo;
mod tools;

pub use activity::{
    AgentNode, JOB_TOOL_CAP, JobActivity, SWARM_TOOL_CAP, SwarmActivity, agent_tree,
};
pub use blocks::{BLOCK_LEN, BillingBlock, BurnRate, UsagePoint, billing_blocks};
pub use context::{ContextGauge, NEAR_COMPACTION, context_window};
pub use environment::{Environment, McpServer};
pub use external::{ExternalKind, ExternalSession, ExternalState};
pub use flow::{FLOW_MINUTES, FlowBucket, TokenFlow, cache_hit_ratio, series_of};
pub use instructions::{FILES_PER_SESSION, InstructionFile, remember};
pub use limits::{
    LimitProjection, LimitState, ProjectionBasis, RateLimit, project_limit, window_length,
};
pub use pricing::{ModelPrice, PriceTable, TokenUsage, model_key};
pub use spend::{
    CALENDAR_DAYS, DaySpend, Interner, LIVE_WITHIN, NamedSpend, SESSION_LIMIT, SessionUsage, Spend,
    UsageBook, UsageEntry, UsageReport, WEEKS, WeekSpend, heat_levels,
};
pub use statusline::{LimitSource, PromptCache, PullRef, Statusline, WindowReading};
pub use time::{
    DAY, HOUR, MINUTE, ago, day_date, day_start, local_clock, local_day, short_span, week_start,
    weekday,
};
pub use todo::{Todo, TodoStatus, parse_todos, todo_counts};
pub use tools::{FileTouch, ToolCall, ToolLedger, ToolStat};
