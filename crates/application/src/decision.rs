//! The decision log's records: what was decided, by whom and how surely.

use swarmux_domain::{JobId, Millis, Verdict};

/// How a decision ended, as logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Deny,
    /// Held for the user; a later record says how it ended.
    Ask,
    Pass,
    /// The loop breaker stopped the job.
    Stop,
    /// A prompt was sent to the session the router picked.
    Route,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allow => "allow",
            Outcome::Deny => "deny",
            Outcome::Ask => "ask",
            Outcome::Pass => "pass",
            Outcome::Stop => "stop",
            Outcome::Route => "route",
        }
    }

    pub fn parse(s: &str) -> Option<Outcome> {
        Some(match s {
            "allow" => Outcome::Allow,
            "deny" => Outcome::Deny,
            "ask" => Outcome::Ask,
            "pass" => Outcome::Pass,
            "stop" => Outcome::Stop,
            "route" => Outcome::Route,
            _ => return None,
        })
    }
}

impl From<Verdict> for Outcome {
    fn from(v: Verdict) -> Outcome {
        match v {
            Verdict::Allow => Outcome::Allow,
            Verdict::Deny => Outcome::Deny,
            Verdict::Ask => Outcome::Ask,
            Verdict::Pass => Outcome::Pass,
        }
    }
}

/// Who made a decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// The guard's rules, or the user answering an approval.
    #[default]
    Guard,
    Loop,
    /// The hosted JEV model.
    Jev,
    /// JEV was asked but gave no usable answer.
    JevError,
    /// The Haiku prompt router.
    Haiku,
    /// The Haiku router failed; the selected session was kept.
    HaikuError,
    /// No model was asked (one session, or no router).
    Fallback,
}

impl Source {
    pub const ALL: [Source; 7] = [
        Source::Guard,
        Source::Loop,
        Source::Jev,
        Source::JevError,
        Source::Haiku,
        Source::HaikuError,
        Source::Fallback,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Guard => "guard",
            Source::Loop => "loop",
            Source::Jev => "jev",
            Source::JevError => "jev-error",
            Source::Haiku => "haiku",
            Source::HaikuError => "haiku-error",
            Source::Fallback => "fallback",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|x| x.as_str() == s)
    }
}

/// How a model's answer came about: enough to tell, later, which model said
/// what about which exact question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JevTrace {
    pub provider: String,
    pub model: String,
    /// SHA-256 (hex) of what was sent.
    pub prompt_sha256: String,
    /// The summary of the call was cut before it was sent.
    pub truncated: bool,
}

/// What a decision record carries beyond the call and its outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionExtra {
    /// The subagent that made the call, when it was one.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    /// Set on records of a model's answer.
    pub jev: Option<JevTrace>,
    /// What resolved the call: `human-typed`, `human-key`, `rule`, `model`
    /// or `timeout`; unset for calls decided at once.
    pub resolver: Option<String>,
}

/// One line of the decision log.
#[derive(Clone, Debug, PartialEq)]
pub struct DecisionRecord {
    pub job_id: Option<JobId>,
    pub tool: String,
    pub subject: String,
    pub outcome: Outcome,
    pub rule: Option<String>,
    pub latency_us: u64,
    pub ts: Millis,
    pub source: Source,
    /// Only for model answers that came with one; never made up.
    pub confidence: Option<f64>,
    pub extra: DecisionExtra,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_and_sources_round_trip() {
        for o in [
            Outcome::Allow,
            Outcome::Deny,
            Outcome::Ask,
            Outcome::Pass,
            Outcome::Stop,
            Outcome::Route,
        ] {
            assert_eq!(Outcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(Outcome::parse("x"), None);
        for s in Source::ALL {
            assert_eq!(Source::parse(s.as_str()), Some(s));
        }
        assert_eq!(Source::parse("x"), None);
    }
}
