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
    /// Legacy: records of the hosted JEV model, which was removed. Never
    /// written any more; kept so history made with it still reads.
    Jev,
    /// Legacy: JEV was asked but gave no usable answer. See [`Source::Jev`].
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

    /// What a list shows: the stored word, and for the sources of the
    /// removed JEV a note that it no longer exists.
    pub fn label(self) -> &'static str {
        match self {
            Source::Jev => "jev (removed)",
            Source::JevError => "jev-error (removed)",
            other => other.as_str(),
        }
    }
}

/// What a decision record carries beyond the call and its outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionExtra {
    /// The subagent that made the call, when it was one.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    /// What resolved the call: `human-typed`, `human-key`, `rule` or
    /// `timeout` (`model` on records made while JEV existed); unset for
    /// calls decided at once.
    pub resolver: Option<String>,
    /// The hash of the rule set in force when it was decided.
    pub policy_sha256: Option<String>,
    /// The tier and file of the rule that decided it, when one did.
    pub rule_tier: Option<String>,
    pub rule_source: Option<String>,
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
    /// Only for model answers that came with one (those of the removed JEV);
    /// never made up.
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

    #[test]
    fn records_made_by_the_removed_jev_still_read() {
        // Stored decisions and ledger entries name their source as text.
        assert_eq!(Source::parse("jev"), Some(Source::Jev));
        assert_eq!(Source::parse("jev-error"), Some(Source::JevError));
        assert_eq!(Source::Jev.as_str(), "jev");
        assert_eq!(Source::JevError.as_str(), "jev-error");
        assert_eq!(Source::Jev.label(), "jev (removed)");
        assert_eq!(Source::JevError.label(), "jev-error (removed)");
        assert_eq!(Source::Haiku.label(), "haiku");
        // Nothing new is ever made with them: the default is the guard.
        assert_eq!(Source::default(), Source::Guard);
    }
}
