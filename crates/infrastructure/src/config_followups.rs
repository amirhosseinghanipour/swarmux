//! The `[followups]` section of `config.toml`: what is collected from
//! finished agents, the owner brief times and the standing orders.

use std::path::Path;

use serde::Deserialize;
use swarmux_application::FollowSettings;
use swarmux_domain::followup::{BriefSlots, FollowKind, OrderAction, StandingOrder};

use crate::config::{ConfigError, key_line};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFollowups {
    enabled: Option<bool>,
    extract: Option<bool>,
    extract_model: Option<String>,
    brief_times: Option<Vec<String>>,
    max_cards: Option<usize>,
    defer_hours: Option<u32>,
    spot_check_days: Option<u32>,
    protected: Option<Vec<String>>,
    order: Option<Vec<RawOrder>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawOrder {
    id: Option<String>,
    kinds: Option<Vec<String>>,
    repos: Option<Vec<String>>,
    paths: Option<Vec<String>>,
    keywords: Option<Vec<String>>,
    max_cost_usd: Option<f64>,
    action: String,
    allow_risky: Option<bool>,
}

pub(crate) fn parse_followups(
    raw: RawFollowups,
    text: &str,
    path: &Path,
) -> Result<FollowSettings, ConfigError> {
    let error = |key: &str, message: &str| ConfigError {
        path: path.to_path_buf(),
        line: key_line(text, key),
        message: message.to_string(),
    };
    let mut s = FollowSettings::default();
    if let Some(on) = raw.enabled {
        s.enabled = on;
    }
    if let Some(on) = raw.extract {
        s.extract = on;
    }
    if let Some(model) = raw.extract_model {
        let model = model.trim();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err(error(
                "extract_model",
                "extract_model must be one word, like \"haiku\"",
            ));
        }
        s.extract_model = model.to_string();
    }
    if let Some(times) = raw.brief_times {
        s.slots = BriefSlots::parse(&times).ok_or_else(|| {
            error(
                "brief_times",
                "brief_times are local times like \"09:00\" (two digits each); [] turns the scheduled brief off",
            )
        })?;
    }
    if let Some(n) = raw.max_cards {
        s.max_cards = n.clamp(1, 20);
    }
    if let Some(h) = raw.defer_hours {
        s.defer_hours = h.max(1);
    }
    if let Some(d) = raw.spot_check_days {
        s.spot_check_days = d.max(1);
    }
    s.protected = raw.protected.unwrap_or_default();
    for (i, o) in raw.order.unwrap_or_default().into_iter().enumerate() {
        let id =
            o.id.map(|i| i.trim().to_string())
                .filter(|i| !i.is_empty())
                .unwrap_or_else(|| format!("order-{}", i + 1));
        if s.orders.iter().any(|x| x.id == id) {
            return Err(error("order", &format!("two orders are called \"{id}\"")));
        }
        let action = OrderAction::parse(&o.action).ok_or_else(|| {
            error(
                "action",
                "an order's action is \"auto-approve\", \"always-ask\" or \"park\"",
            )
        })?;
        let mut order = StandingOrder::new(&id, action);
        for k in o.kinds.unwrap_or_default() {
            order.kinds.push(FollowKind::parse(&k).ok_or_else(|| {
                error(
                    "kinds",
                    "kinds are question, decision, task, risk or blocker",
                )
            })?);
        }
        order.repos = o.repos.unwrap_or_default();
        order.paths = o.paths.unwrap_or_default();
        order.keywords = o.keywords.unwrap_or_default();
        order.max_cost_usd = o.max_cost_usd.filter(|c| c.is_finite() && *c >= 0.0);
        order.allow_risky = o.allow_risky.unwrap_or(false);
        if action == OrderAction::AutoApprove && order.is_unconditional() {
            return Err(error(
                "action",
                &format!(
                    "order \"{id}\" would approve everything: give it kinds, repos, paths, keywords or max_cost_usd"
                ),
            ));
        }
        s.orders.push(order);
    }
    Ok(s)
}
