//! The keys of `[guard]` and `[jev]` that are read line by line: a value
//! that is wrong is replaced by its default and said so in
//! `Settings::warnings`, never a reason to refuse the file.

use swarmux_application::{OnUnreachable, Settings};
use swarmux_domain::RuleScope;
use swarmux_domain::alarm::Reversibility;
use toml::Value;

/// Longest an "always allow" may last, in days.
const MAX_EXPIRY_DAYS: i64 = 3650;

/// The tolerant keys of `[guard]`, as written.
#[derive(Default)]
pub(crate) struct GuardExtras {
    pub on_unreachable: Option<Value>,
    pub always_scope: Option<Value>,
    pub always_expiry_days: Option<Value>,
    pub protected_paths: Option<Value>,
    pub hook_on_failure_block: Option<Value>,
}

/// Applies `[guard]` extras and `max_allow` of `[jev]` to `settings`,
/// adding a warning for each value that was replaced.
pub(crate) fn apply(
    extras: GuardExtras,
    max_allow: Option<Value>,
    text: &str,
    settings: &mut Settings,
) {
    let mut warnings: Vec<String> = Vec::new();
    let mut warn = |key: &str, message: String| {
        let line =
            super::config::key_line(text, key).map_or(String::new(), |l| format!(" (line {l})"));
        warnings.push(format!("config.toml{line}: {message}"));
    };
    let guard = &mut settings.guard;
    if let Some(v) = extras.on_unreachable {
        match v.as_str().and_then(OnUnreachable::parse) {
            Some(mode) => guard.on_unreachable = mode,
            None => warn(
                "on_unreachable",
                format!(
                    "guard.on_unreachable {v} is not \"auto\", \"deny\", \"ask\" or \"pass\"; using \"auto\" (deny for unattended jobs, pass for the rest)"
                ),
            ),
        }
    }
    if let Some(v) = extras.always_scope {
        match v.as_str().and_then(RuleScope::parse) {
            Some(scope) => guard.always_scope = scope,
            None => warn(
                "always_scope",
                format!(
                    "guard.always_scope {v} is not \"repo\", \"workspace\" or \"global\"; using \"repo\""
                ),
            ),
        }
    }
    if let Some(v) = extras.always_expiry_days {
        match &v {
            Value::String(s) if s.trim() == "never" => guard.always_expiry_days = None,
            Value::Integer(n) if (1..=MAX_EXPIRY_DAYS).contains(n) => {
                guard.always_expiry_days = u32::try_from(*n).ok();
            }
            _ => warn(
                "always_expiry_days",
                format!(
                    "guard.always_expiry_days {v} is not a number of days from 1 to {MAX_EXPIRY_DAYS} or \"never\"; using {} days",
                    guard.always_expiry_days.map_or(0, u64::from)
                ),
            ),
        }
    }
    if let Some(v) = extras.protected_paths {
        let list: Option<Vec<String>> = v.as_array().and_then(|a| {
            a.iter()
                .map(|x| {
                    x.as_str()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .collect()
        });
        match list {
            Some(list) => guard.protected_paths = list,
            None => warn(
                "protected_paths",
                format!(
                    "guard.protected_paths {v} is not a list of path globs; keeping only the built-in protected paths"
                ),
            ),
        }
    }
    if let Some(v) = extras.hook_on_failure_block {
        match v.as_bool() {
            Some(b) => guard.hook_on_failure_block = b,
            None => warn(
                "hook_on_failure_block",
                format!("guard.hook_on_failure_block {v} is not true or false; using false"),
            ),
        }
    }
    if let Some(v) = max_allow {
        let tier = v.as_str().map(str::trim).and_then(|s| {
            [
                Reversibility::R0,
                Reversibility::R1,
                Reversibility::R2,
                Reversibility::R3,
            ]
            .into_iter()
            .find(|r| r.as_str() == s)
        });
        match tier {
            Some(r) => settings.jev.max_allow = r,
            None => warn(
                "max_allow",
                format!(
                    "jev.max_allow {v} is not \"R0\", \"R1\", \"R2\" or \"R3\" (irreversible calls are never JEV's to allow); using \"R1\""
                ),
            ),
        }
    }
    settings.warnings.extend(warnings);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use swarmux_application::{OnUnreachable, Settings};
    use swarmux_domain::RuleScope;
    use swarmux_domain::alarm::Reversibility;

    use crate::config::parse_settings;

    fn parse(text: &str) -> Settings {
        parse_settings(text, Path::new("config.toml")).unwrap()
    }

    #[test]
    fn defaults_without_the_keys() {
        let s = parse("");
        assert_eq!(s.guard.on_unreachable, OnUnreachable::Auto);
        assert_eq!(s.guard.always_scope, RuleScope::Repo);
        assert_eq!(s.guard.always_expiry_days, Some(30));
        assert!(s.guard.protected_paths.is_empty());
        assert!(!s.guard.hook_on_failure_block);
        assert_eq!(s.jev.max_allow, Reversibility::R1);
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn good_values_are_read() {
        let s = parse(
            "[guard]\non_unreachable = \"deny\"\nalways_scope = \"workspace\"\nalways_expiry_days = 7\nprotected_paths = [\"deploy/**\", \"Makefile\"]\nhook_on_failure_block = true\n[jev]\nmax_allow = \"R2\"\n",
        );
        assert_eq!(s.guard.on_unreachable, OnUnreachable::Deny);
        assert_eq!(s.guard.always_scope, RuleScope::Workspace);
        assert_eq!(s.guard.always_expiry_days, Some(7));
        assert_eq!(s.guard.protected_paths, ["deploy/**", "Makefile"]);
        assert!(s.guard.hook_on_failure_block);
        assert_eq!(s.jev.max_allow, Reversibility::R2);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(s.guard.protected_globs().contains(&"deploy/**".to_string()));
        assert!(s.guard.protected_globs().contains(&".mcp.json".to_string()));
    }

    #[test]
    fn never_is_an_explicit_choice() {
        let s = parse("[guard]\nalways_expiry_days = \"never\"\n");
        assert_eq!(s.guard.always_expiry_days, None);
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn bad_lines_fall_back_to_defaults_with_a_warning_each() {
        let s = parse(
            "max_concurrent = 2\n[guard]\non_unreachable = \"sometimes\"\nalways_scope = 3\nalways_expiry_days = 0\nprotected_paths = \"x\"\nhook_on_failure_block = \"yes\"\n[jev]\nmax_allow = \"R4\"\n",
        );
        assert_eq!(s.max_concurrent, 2, "the rest of the file is read");
        assert_eq!(s.guard.on_unreachable, OnUnreachable::Auto);
        assert_eq!(s.guard.always_scope, RuleScope::Repo);
        assert_eq!(s.guard.always_expiry_days, Some(30));
        assert!(s.guard.protected_paths.is_empty());
        assert!(!s.guard.hook_on_failure_block);
        assert_eq!(s.jev.max_allow, Reversibility::R1, "R4 is never JEV's");
        assert_eq!(s.warnings.len(), 6, "{:#?}", s.warnings);
        assert!(s.warnings[0].contains("line 3"), "{}", s.warnings[0]);
        assert!(s.warnings[0].contains("on_unreachable"));
    }

    #[test]
    fn an_old_config_with_only_allow_rules_reads_as_before() {
        let s = parse(
            "[guard]\nallow = [\"Bash(make)\"]\ndeny = [\"Bash(curl:*)\"]\nunmatched = \"pass\"\n",
        );
        assert_eq!(s.guard.allow.len(), 1);
        assert_eq!(s.guard.on_unreachable, OnUnreachable::Auto);
        assert!(s.warnings.is_empty());
    }
}
