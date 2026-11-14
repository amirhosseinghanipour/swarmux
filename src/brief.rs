//! `swarmux return-brief lint`: check the `[brief]` of the config.

use std::process::ExitCode;

use swarmux_domain::console::{BriefConfig, BriefSettings, resolve_brief};
use swarmux_infrastructure::{Paths, load_effective_settings};

/// What `swarmux return-brief` was asked to do.
#[derive(Clone, Copy)]
pub enum Action {
    Lint,
}

pub fn run(action: Action) -> anyhow::Result<ExitCode> {
    let paths = Paths::default_home()?;
    let settings = load_effective_settings(&paths);
    let resolved = resolve_brief(&settings.brief);
    let source = paths.config.display().to_string();
    match action {
        Action::Lint => {
            let (report, failed) = lint(&settings.brief, &resolved, &source);
            print!("{report}");
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The report of `swarmux return-brief lint` and whether anything in it is a
/// problem (the command then exits with failure).
pub fn lint(config: &BriefConfig, settings: &BriefSettings, source: &str) -> (String, bool) {
    let mut out = String::new();
    for problem in &settings.problems {
        out.push_str(&format!("error: {problem}\n"));
    }
    let n = settings.problems.len();
    if !config.present {
        out.push_str(&format!(
            "brief: no [brief] in {source}: away after {} minutes, shown on attach, Esc to {}\n",
            settings.away_after_minutes,
            settings.esc_goes_to.name()
        ));
    } else if n == 0 {
        out.push_str(&format!("brief: {source} is fine\n"));
    } else {
        out.push_str(&format!(
            "brief: {n} problem{} in {source}\n       swarmux ignores each entry above and uses the default for it.\n",
            if n == 1 { "" } else { "s" },
        ));
    }
    (out, n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_section_says_the_defaults_apply_and_is_not_a_failure() {
        let c = BriefConfig::default();
        let (out, failed) = lint(&c, &resolve_brief(&c), "config.toml");
        assert!(!failed);
        assert!(out.contains("no [brief] in config.toml"), "{out}");
        assert!(out.contains("away after 30 minutes"), "{out}");
    }

    #[test]
    fn a_good_section_is_fine() {
        let c = BriefConfig {
            present: true,
            away_after_minutes: Some(10),
            ..BriefConfig::default()
        };
        let (out, failed) = lint(&c, &resolve_brief(&c), "config.toml");
        assert!(!failed);
        assert_eq!(out, "brief: config.toml is fine\n");
    }

    #[test]
    fn every_problem_is_listed_and_the_command_fails() {
        let c = BriefConfig {
            present: true,
            away_after_minutes: Some(0),
            esc_goes_to: Some("record".into()),
            unknown: vec!["colour".into()],
            ..BriefConfig::default()
        };
        let (out, failed) = lint(&c, &resolve_brief(&c), "config.toml");
        assert!(failed);
        assert!(out.contains("error: [brief] away_after_minutes:"), "{out}");
        assert!(out.contains("error: [brief] esc_goes_to:"), "{out}");
        assert!(out.contains("error: [brief] colour:"), "{out}");
        assert!(out.contains("brief: 3 problems in config.toml"), "{out}");
    }
}
