//! The console's rules: where an outcome stands on the Work queue, which
//! actions its state allows, and how the cards of the Deck relate to each
//! other. Pure functions over plain facts; the application layer gathers
//! the facts and the clients (the terminal, a daemon API, a phone) render
//! what these rules decide.
//!
//! Nothing here is invented: every group, action and edge is derived from
//! state swarmux already keeps (the attention items, the mission and job
//! states, the gate's verdicts, the plan and its task graph, the
//! repositories' contracts and the files tasks claim).

mod actions;
mod brief;
mod brief_config;
mod deck_config;
mod graph;
mod metrics;
mod rank;

pub use actions::{ActionFacts, ActionKind, Allowed, InterveneFacts, LandFacts, allowed};
pub use brief::{
    CallState, Digest as BriefDigest, Fallback as GuardFallback, GuardCall, GuardDigest,
    HiddenNotRun, Moment as BriefMoment, RuleAdded, Spend as BriefSpend, TIMELINE_KEPT,
    Tone as BriefTone, WorktreeRun, digest as compute_brief_digest,
};
pub use brief_config::{
    BriefConfig, BriefProblem, BriefSettings, DEFAULT_AWAY_AFTER_MINUTES, EscGoesTo,
    KEYS as BRIEF_KEYS, MAX_AWAY_AFTER_MINUTES, resolve as resolve_brief,
};
pub use deck_config::{
    DEFAULT_PANELS, DEFAULT_SESSION_ROWS, DeckConfig, DeckLayout, DeckProblem, Direction,
    KEYS as DECK_KEYS, MAX_SESSION_ROWS, Panel, Zone, resolve as resolve_deck,
};
pub use graph::{
    ContractFlow, DepState, Edge, EdgeKind, Node, Progress, claim_edges, contract_edges, depths,
    task_edges,
};
pub use metrics::{FirstRun, Metrics, Spread, compute as compute_metrics};
pub use rank::{Ask, Group, Placement, Ranked, Stage, Standing, place, rank};
