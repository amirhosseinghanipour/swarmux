//! The `[deck]` section of `config.toml`, as rules: which panels the Deck
//! shows and in which order, which way its graph runs, how many sessions its
//! strip lists.
//!
//! Reading the file is the infrastructure's job and only copies what is
//! written ([`DeckConfig`]); [`resolve`] decides what can be used. A mistake
//! never stops swarmux: what cannot be used is left out, the default
//! applies in its place, and each such case is a [`DeckProblem`] the lint
//! (`swarmux deck lint`) prints and the Deck says in a warning.

use std::fmt;

/// A panel of the Deck. The name is what `panels = [...]` spells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The task graph: the cards of the work and what relates them.
    Graph,
    /// One row per session: what it runs, its context and spend.
    Sessions,
    /// First-run pass rate, claim to verified, checks run.
    Verification,
    /// The usage windows, their forecast and what was spent.
    Limits,
    /// Held and denied calls; routine allows as one number.
    Guard,
    /// What happened lately.
    Timeline,
    /// The decisions waiting for the user.
    Needs,
}

impl Panel {
    /// Every panel, in the default order.
    pub const ALL: [Panel; 7] = [
        Panel::Graph,
        Panel::Sessions,
        Panel::Verification,
        Panel::Limits,
        Panel::Guard,
        Panel::Timeline,
        Panel::Needs,
    ];

    /// The name a config spells it with.
    pub fn name(self) -> &'static str {
        match self {
            Panel::Graph => "graph",
            Panel::Sessions => "sessions",
            Panel::Verification => "verification",
            Panel::Limits => "limits",
            Panel::Guard => "guard",
            Panel::Timeline => "timeline",
            Panel::Needs => "needs",
        }
    }

    pub fn parse(name: &str) -> Option<Panel> {
        Panel::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Where the panel sits: the Deck has a main column, a side column
    /// and a foot, and the order of `panels` is the order within each.
    pub fn zone(self) -> Zone {
        match self {
            Panel::Graph | Panel::Sessions => Zone::Main,
            Panel::Needs | Panel::Verification | Panel::Limits => Zone::Side,
            Panel::Guard | Panel::Timeline => Zone::Foot,
        }
    }
}

impl fmt::Display for Panel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Where on the Deck a panel is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The wide column at the left: the graph, the sessions.
    Main,
    /// The narrow column at the right.
    Side,
    /// The row along the bottom.
    Foot,
}

/// Which way the graph runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    /// Producers at the left, consumers at the right.
    #[default]
    LeftToRight,
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::LeftToRight => "lr",
        }
    }
}

/// What the strip of sessions lists at most, by default.
pub const DEFAULT_SESSION_ROWS: usize = 7;
/// The most rows the strip may be asked to list.
pub const MAX_SESSION_ROWS: usize = 30;

/// The default panels: everything, in the order of the design.
pub const DEFAULT_PANELS: [Panel; 7] = [
    Panel::Graph,
    Panel::Sessions,
    Panel::Verification,
    Panel::Limits,
    Panel::Guard,
    Panel::Timeline,
    Panel::Needs,
];

/// What `[deck]` says, as written: not yet checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeckConfig {
    /// The file has a `[deck]` section at all.
    pub present: bool,
    pub panels: Option<Vec<String>>,
    pub direction: Option<String>,
    pub session_rows: Option<i64>,
    /// Keys of `[deck]` that mean nothing.
    pub unknown: Vec<String>,
    /// Values of the wrong type: the key and what was found there.
    pub malformed: Vec<(String, String)>,
}

/// The keys `[deck]` understands.
pub const KEYS: [&str; 3] = ["panels", "direction", "session_rows"];

/// One thing in `[deck]` that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckProblem {
    /// The key it is about, as written: `panels`, `deck.direction`.
    pub at: String,
    pub message: String,
}

impl fmt::Display for DeckProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[deck] {}: {}", self.at, self.message)
    }
}

/// What the Deck is drawn with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckLayout {
    /// The panels shown, in the order of the config (the default order
    /// where it says nothing).
    pub panels: Vec<Panel>,
    pub direction: Direction,
    /// Most sessions the strip lists.
    pub session_rows: usize,
    /// What could not be used and was left out.
    pub problems: Vec<DeckProblem>,
}

impl Default for DeckLayout {
    fn default() -> DeckLayout {
        resolve(&DeckConfig::default())
    }
}

impl DeckLayout {
    pub fn shows(&self, panel: Panel) -> bool {
        self.panels.contains(&panel)
    }

    /// The panels of `zone`, in order.
    pub fn zone(&self, zone: Zone) -> Vec<Panel> {
        self.panels
            .iter()
            .copied()
            .filter(|p| p.zone() == zone)
            .collect()
    }
}

fn names() -> String {
    Panel::ALL.map(Panel::name).join(", ")
}

/// Checks `config`: what can be used is used, what cannot is a problem.
pub fn resolve(config: &DeckConfig) -> DeckLayout {
    let mut problems = Vec::new();
    let mut problem = |at: &str, message: String| {
        problems.push(DeckProblem {
            at: at.to_string(),
            message,
        });
    };
    for key in &config.unknown {
        problem(
            key,
            format!(
                "not a setting of the deck (it has {}); ignored",
                KEYS.join(", ")
            ),
        );
    }
    for (key, found) in &config.malformed {
        problem(key, format!("{found}; the default applies"));
    }

    let mut panels = Vec::new();
    if let Some(listed) = &config.panels {
        for name in listed {
            match Panel::parse(name) {
                None => problem(
                    "panels",
                    format!(
                        "unknown panel \"{name}\" (the panels are {}); ignored",
                        names()
                    ),
                ),
                Some(p) if panels.contains(&p) => {
                    problem(
                        "panels",
                        format!("\"{name}\" is listed twice; the first counts"),
                    );
                }
                Some(p) => panels.push(p),
            }
        }
        if panels.is_empty() {
            problem(
                "panels",
                format!(
                    "no panel is left to show (the panels are {}); the default panels apply",
                    names()
                ),
            );
        }
    }
    if panels.is_empty() {
        panels = DEFAULT_PANELS.to_vec();
    }

    let direction = match config.direction.as_deref() {
        None | Some("lr") => Direction::LeftToRight,
        Some(other) => {
            problem(
                "direction",
                format!(
                    "\"{other}\" is not a direction the graph can run (only \"lr\": producers at the left); \"lr\" applies"
                ),
            );
            Direction::LeftToRight
        }
    };

    let session_rows = match config.session_rows {
        None => DEFAULT_SESSION_ROWS,
        Some(n) => match usize::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_SESSION_ROWS).contains(n))
        {
            Some(n) => n,
            None => {
                problem(
                    "session_rows",
                    format!(
                        "must be between 1 and {MAX_SESSION_ROWS} (how many sessions the strip lists), not {n}; {DEFAULT_SESSION_ROWS} applies"
                    ),
                );
                DEFAULT_SESSION_ROWS
            }
        },
    };
    DeckLayout {
        panels,
        direction,
        session_rows,
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(panels: &[&str]) -> DeckConfig {
        DeckConfig {
            present: true,
            panels: Some(panels.iter().map(|s| s.to_string()).collect()),
            ..DeckConfig::default()
        }
    }

    #[test]
    fn no_config_is_the_design() {
        let l = resolve(&DeckConfig::default());
        assert_eq!(l.panels, DEFAULT_PANELS);
        assert_eq!(l.direction, Direction::LeftToRight);
        assert_eq!(l.session_rows, 7);
        assert!(l.problems.is_empty());
        assert_eq!(DeckLayout::default(), l);
    }

    #[test]
    fn the_default_order_names_every_panel_once() {
        let mut all = DEFAULT_PANELS.to_vec();
        all.sort_by_key(|p| p.name());
        let mut every = Panel::ALL.to_vec();
        every.sort_by_key(|p| p.name());
        assert_eq!(all, every);
        for p in Panel::ALL {
            assert_eq!(Panel::parse(p.name()), Some(p));
        }
    }

    #[test]
    fn the_listed_panels_are_the_ones_shown_in_that_order() {
        let l = resolve(&config(&["limits", "graph", "needs"]));
        assert_eq!(l.panels, [Panel::Limits, Panel::Graph, Panel::Needs]);
        assert!(l.problems.is_empty());
        assert!(!l.shows(Panel::Guard));
        assert_eq!(l.zone(Zone::Side), [Panel::Limits, Panel::Needs]);
        assert_eq!(l.zone(Zone::Main), [Panel::Graph]);
        assert!(l.zone(Zone::Foot).is_empty());
    }

    #[test]
    fn an_unknown_panel_is_reported_and_left_out() {
        let l = resolve(&config(&["graph", "weather", "guard"]));
        assert_eq!(l.panels, [Panel::Graph, Panel::Guard]);
        assert_eq!(l.problems.len(), 1);
        let text = l.problems[0].to_string();
        assert!(
            text.starts_with("[deck] panels: unknown panel \"weather\""),
            "{text}"
        );
        assert!(
            text.contains("graph, sessions"),
            "it lists the known ones: {text}"
        );
    }

    #[test]
    fn a_panel_listed_twice_counts_once() {
        let l = resolve(&config(&["graph", "guard", "graph"]));
        assert_eq!(l.panels, [Panel::Graph, Panel::Guard]);
        assert!(l.problems[0].message.contains("listed twice"));
    }

    #[test]
    fn nothing_left_to_show_falls_back_to_the_default_panels() {
        for listed in [&[][..], &["nope"][..], &["a", "b"][..]] {
            let l = resolve(&config(listed));
            assert_eq!(l.panels, DEFAULT_PANELS, "{listed:?}");
            assert!(
                l.problems
                    .iter()
                    .any(|p| p.message.contains("no panel is left")),
                "{listed:?}"
            );
        }
    }

    #[test]
    fn the_direction_is_left_to_right_and_nothing_else_yet() {
        let mut c = DeckConfig::default();
        c.direction = Some("lr".into());
        assert!(resolve(&c).problems.is_empty());
        c.direction = Some("tb".into());
        let l = resolve(&c);
        assert_eq!(l.direction, Direction::LeftToRight);
        assert_eq!(l.problems.len(), 1);
        assert!(l.problems[0].message.contains("\"tb\""));
    }

    #[test]
    fn the_session_rows_are_a_count_within_bounds() {
        let rows = |n| {
            let c = DeckConfig {
                session_rows: Some(n),
                ..DeckConfig::default()
            };
            resolve(&c)
        };
        assert_eq!(rows(1).session_rows, 1);
        assert_eq!(rows(30).session_rows, 30);
        for bad in [0, -2, 31, 5000] {
            let l = rows(bad);
            assert_eq!(l.session_rows, 7, "{bad}");
            assert_eq!(l.problems.len(), 1, "{bad}");
            assert!(l.problems[0].message.contains("between 1 and 30"));
        }
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_problems_not_failures() {
        let c = DeckConfig {
            present: true,
            unknown: vec!["colour".into()],
            malformed: vec![(
                "panels".into(),
                "a number where a list of names belongs".into(),
            )],
            ..DeckConfig::default()
        };
        let l = resolve(&c);
        assert_eq!(l.panels, DEFAULT_PANELS);
        assert_eq!(l.problems.len(), 2);
        assert!(l.problems[0].to_string().contains("colour"));
        assert!(l.problems[1].message.contains("the default applies"));
    }
}
