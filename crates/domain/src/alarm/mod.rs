//! Alarm management for the inbox: how risky a held call is, how it ranks,
//! which asks can be answered together, how long an item may be held back,
//! how many items a person can take and how much of their time the inbox
//! already uses. Pure rules; collecting items is the application's job.

mod confirm;
mod deferral;
mod kpi;
mod load;
mod risk;
mod signature;
mod suggest;

pub use confirm::{ARMING_MS, Challenge, target_of};
pub use deferral::{
    Breakpoint, IDLE_BREAKPOINT_MS, P1_BOUND_MS, P2_BOUND_MS, P3_BOUND_MS, P3_FOCUS_BOUND_MS,
    bound, deliver_by, releases,
};
pub use kpi::{
    FLOOD_AT, FLOOD_WINDOW_MS, Flood, FloodGroup, Kpis, Raised, STALE_AFTER_MS, STANDING_AFTER_MS,
    compute_kpis, detect_flood,
};
pub use load::{HOLD_AT, LOAD_WINDOW_MS, LoadMeter};
pub use risk::{
    Reversibility, Risk, RiskInputs, RiskTier, assess, blast_radius, reversibility, secret_flag,
};
pub use signature::{Signature, signature_of};
pub use suggest::{SUGGEST_AFTER, Tally, novelty};
