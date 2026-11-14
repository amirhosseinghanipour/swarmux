//! The `[brief]` section of `config.toml`, as rules: when the Brief shows
//! by itself, how long away counts, and where `Esc` goes from it.
//!
//! Reading the file is the infrastructure's job and only copies what is
//! written ([`BriefConfig`]); [`resolve`] decides what can be used. A mistake
//! never stops swarmux: what cannot be used is ignored, the default applies
//! in its place, and each such case is a [`BriefProblem`] that the lint
//! (`swarmux brief lint`) prints and the app says once at start.

use std::fmt;

/// How long the user must have been away, in minutes, before the Brief
/// shows by itself.
pub const DEFAULT_AWAY_AFTER_MINUTES: u32 = 30;
/// The longest gap that may be asked for: a week.
pub const MAX_AWAY_AFTER_MINUTES: u32 = 7 * 24 * 60;

/// Where `Esc` goes from the Brief.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EscGoesTo {
    #[default]
    Work,
    Deck,
}

impl EscGoesTo {
    pub fn name(self) -> &'static str {
        match self {
            EscGoesTo::Work => "work",
            EscGoesTo::Deck => "deck",
        }
    }

    pub fn parse(text: &str) -> Option<EscGoesTo> {
        match text.trim().to_ascii_lowercase().as_str() {
            "work" => Some(EscGoesTo::Work),
            "deck" => Some(EscGoesTo::Deck),
            _ => None,
        }
    }
}

/// What `[brief]` says, as written: not yet checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BriefConfig {
    /// The file has a `[brief]` section at all.
    pub present: bool,
    pub away_after_minutes: Option<i64>,
    pub show_on_attach: Option<bool>,
    pub esc_goes_to: Option<String>,
    /// Keys of `[brief]` that mean nothing.
    pub unknown: Vec<String>,
    /// Values of the wrong type: the key and what was found there.
    pub malformed: Vec<(String, String)>,
}

/// The keys `[brief]` understands.
pub const KEYS: [&str; 3] = ["away_after_minutes", "show_on_attach", "esc_goes_to"];

/// One thing in `[brief]` that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefProblem {
    pub at: String,
    pub message: String,
}

impl fmt::Display for BriefProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[brief] {}: {}", self.at, self.message)
    }
}

/// What the Brief runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefSettings {
    pub away_after_minutes: u32,
    pub show_on_attach: bool,
    pub esc_goes_to: EscGoesTo,
    /// What could not be used and was left out.
    pub problems: Vec<BriefProblem>,
}

impl Default for BriefSettings {
    fn default() -> BriefSettings {
        resolve(&BriefConfig::default())
    }
}

impl BriefSettings {
    /// The gap that counts as away, in milliseconds.
    pub fn away_after_ms(&self) -> i64 {
        i64::from(self.away_after_minutes) * 60_000
    }
}

/// Checks `config`: what can be used is used, what cannot is a problem.
pub fn resolve(config: &BriefConfig) -> BriefSettings {
    let mut problems = Vec::new();
    let mut problem = |at: &str, message: String| {
        problems.push(BriefProblem {
            at: at.to_string(),
            message,
        });
    };
    for key in &config.unknown {
        problem(
            key,
            format!(
                "not a setting of the brief (it has {}); ignored",
                KEYS.join(", ")
            ),
        );
    }
    for (key, found) in &config.malformed {
        problem(key, format!("{found}; the default applies"));
    }
    let away_after_minutes = match config.away_after_minutes {
        None => DEFAULT_AWAY_AFTER_MINUTES,
        Some(n) => match u32::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_AWAY_AFTER_MINUTES).contains(n))
        {
            Some(n) => n,
            None => {
                problem(
                    "away_after_minutes",
                    format!(
                        "must be between 1 and {MAX_AWAY_AFTER_MINUTES} minutes, not {n}; {DEFAULT_AWAY_AFTER_MINUTES} applies"
                    ),
                );
                DEFAULT_AWAY_AFTER_MINUTES
            }
        },
    };
    let esc_goes_to = match config.esc_goes_to.as_deref() {
        None => EscGoesTo::Work,
        Some(text) => EscGoesTo::parse(text).unwrap_or_else(|| {
            problem(
                "esc_goes_to",
                format!("\"{text}\" is not a screen Esc can skip to (\"work\" or \"deck\"); \"work\" applies"),
            );
            EscGoesTo::Work
        }),
    };
    BriefSettings {
        away_after_minutes,
        show_on_attach: config.show_on_attach.unwrap_or(true),
        esc_goes_to,
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BriefConfig {
        BriefConfig {
            present: true,
            ..BriefConfig::default()
        }
    }

    #[test]
    fn no_section_is_thirty_minutes_shown_on_attach_and_esc_to_work() {
        let s = resolve(&BriefConfig::default());
        assert_eq!(s.away_after_minutes, 30);
        assert_eq!(s.away_after_ms(), 1_800_000);
        assert!(s.show_on_attach);
        assert_eq!(s.esc_goes_to, EscGoesTo::Work);
        assert!(s.problems.is_empty());
    }

    #[test]
    fn what_is_written_is_used() {
        let s = resolve(&BriefConfig {
            away_after_minutes: Some(5),
            show_on_attach: Some(false),
            esc_goes_to: Some(" Deck ".into()),
            ..config()
        });
        assert_eq!(s.away_after_minutes, 5);
        assert!(!s.show_on_attach);
        assert_eq!(s.esc_goes_to, EscGoesTo::Deck);
        assert!(s.problems.is_empty());
    }

    #[test]
    fn a_gap_out_of_range_is_a_problem_and_the_default_applies() {
        for bad in [0, -3, 10_081, i64::MAX] {
            let s = resolve(&BriefConfig {
                away_after_minutes: Some(bad),
                ..config()
            });
            assert_eq!(s.away_after_minutes, 30, "{bad}");
            assert_eq!(s.problems.len(), 1, "{bad}");
            assert!(
                s.problems[0]
                    .to_string()
                    .starts_with("[brief] away_after_minutes:"),
                "{}",
                s.problems[0]
            );
        }
        let edge = resolve(&BriefConfig {
            away_after_minutes: Some(10_080),
            ..config()
        });
        assert_eq!(edge.away_after_minutes, 10_080);
    }

    #[test]
    fn an_unknown_screen_is_a_problem_and_work_applies() {
        let s = resolve(&BriefConfig {
            esc_goes_to: Some("record".into()),
            ..config()
        });
        assert_eq!(s.esc_goes_to, EscGoesTo::Work);
        assert!(s.problems[0].message.contains("\"record\""));
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_listed() {
        let s = resolve(&BriefConfig {
            unknown: vec!["colour".into()],
            malformed: vec![(
                "show_on_attach".into(),
                "a string where true or false belongs".into(),
            )],
            ..config()
        });
        assert_eq!(s.problems.len(), 2);
        assert!(
            s.show_on_attach,
            "the default for a value that can't be read"
        );
    }
}
