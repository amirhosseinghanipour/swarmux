//! `swarmux deck`: check the `[deck]` of the config, and show the layout
//! the Deck draws with.

use std::process::ExitCode;

use swarmux_domain::console::{DeckConfig, DeckLayout, Zone, resolve_deck};
use swarmux_infrastructure::{Paths, load_settings};

/// What `swarmux deck` was asked to do.
#[derive(Clone, Copy)]
pub enum Action {
    Lint,
    Show,
}

pub fn run(action: Action) -> anyhow::Result<ExitCode> {
    let paths = Paths::default_home()?;
    let settings = load_settings(&paths.config)?;
    let layout = resolve_deck(&settings.deck);
    let source = paths.config.display().to_string();
    match action {
        Action::Show => print!("{}", show(&layout)),
        Action::Lint => {
            let (report, failed) = lint(&settings.deck, &layout, &source);
            print!("{report}");
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The report of `swarmux deck lint` and whether anything in it is a
/// problem (the command then exits with failure).
pub fn lint(config: &DeckConfig, layout: &DeckLayout, source: &str) -> (String, bool) {
    let mut out = String::new();
    for problem in &layout.problems {
        out.push_str(&format!("error: {problem}\n"));
    }
    let n = layout.problems.len();
    if !config.present {
        out.push_str(&format!(
            "deck: no [deck] in {source}: the default panels apply\n"
        ));
    } else if n == 0 {
        out.push_str(&format!("deck: {source} is fine\n"));
    } else {
        out.push_str(&format!(
            "deck: {n} problem{} in {source}\n      swarmux ignores each entry above and uses the default for it; \
             the deck says so above its panels.\n",
            if n == 1 { "" } else { "s" },
        ));
    }
    (out, n > 0)
}

/// The panels in the order they are drawn, by zone.
pub fn show(layout: &DeckLayout) -> String {
    let names = |zone: Zone| {
        let list = layout.zone(zone);
        if list.is_empty() {
            "(none)".to_string()
        } else {
            list.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ")
        }
    };
    format!(
        "main column: {}\nside column: {}\nfoot:        {}\ndirection:   {}\nsession rows: {}\n",
        names(Zone::Main),
        names(Zone::Side),
        names(Zone::Foot),
        layout.direction.name(),
        layout.session_rows,
    )
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
    fn no_section_says_the_defaults_apply_and_is_not_a_failure() {
        let c = DeckConfig::default();
        let (out, failed) = lint(&c, &resolve_deck(&c), "config.toml");
        assert!(!failed);
        assert!(out.contains("no [deck] in config.toml: the default panels apply"));
    }

    #[test]
    fn a_good_section_is_fine() {
        let c = config(&["graph", "needs"]);
        let (out, failed) = lint(&c, &resolve_deck(&c), "config.toml");
        assert!(!failed);
        assert_eq!(out, "deck: config.toml is fine\n");
    }

    #[test]
    fn every_problem_is_listed_and_the_command_fails() {
        let mut c = config(&["graph", "weather", "limits", "limits"]);
        c.session_rows = Some(99);
        c.direction = Some("tb".into());
        let (out, failed) = lint(&c, &resolve_deck(&c), "config.toml");
        assert!(failed);
        assert!(
            out.contains("error: [deck] panels: unknown panel \"weather\""),
            "{out}"
        );
        assert!(
            out.contains("error: [deck] panels: \"limits\" is listed twice"),
            "{out}"
        );
        assert!(out.contains("error: [deck] direction:"), "{out}");
        assert!(out.contains("error: [deck] session_rows:"), "{out}");
        assert!(out.contains("deck: 4 problems in config.toml"), "{out}");
    }

    #[test]
    fn show_names_the_panels_by_zone_in_order() {
        let out = show(&resolve_deck(&config(&[
            "limits", "graph", "needs", "guard",
        ])));
        assert!(out.contains("main column: graph\n"), "{out}");
        assert!(out.contains("side column: limits, needs\n"), "{out}");
        assert!(out.contains("foot:        guard\n"), "{out}");
        assert!(out.contains("direction:   lr"), "{out}");
    }
}
