//! The `[brief]` section of `config.toml`: when the Brief shows and where
//! `Esc` goes from it.
//!
//! This only reads what is written. What can be used is for the domain to
//! say (`swarmux_domain::console::resolve_brief`), so a mistake in `[brief]`
//! never stops the file from loading.

use swarmux_domain::console::BriefConfig;
use toml::Value;

/// Reads the value of `[brief]`.
pub(crate) fn parse_brief(value: &Value) -> BriefConfig {
    let mut config = BriefConfig {
        present: true,
        ..BriefConfig::default()
    };
    let Value::Table(table) = value else {
        config.malformed.push((
            String::new(),
            format!("{} where a table belongs", describe(value)),
        ));
        return config;
    };
    for (key, value) in table {
        match (key.as_str(), value) {
            ("away_after_minutes", Value::Integer(n)) => config.away_after_minutes = Some(*n),
            ("away_after_minutes", other) => config.malformed.push((
                "away_after_minutes".into(),
                format!(
                    "{} where a whole number of minutes belongs",
                    describe(other)
                ),
            )),
            ("show_on_attach", Value::Boolean(b)) => config.show_on_attach = Some(*b),
            ("show_on_attach", other) => config.malformed.push((
                "show_on_attach".into(),
                format!("{} where true or false belongs", describe(other)),
            )),
            ("esc_goes_to", Value::String(s)) => config.esc_goes_to = Some(s.clone()),
            ("esc_goes_to", other) => config.malformed.push((
                "esc_goes_to".into(),
                format!("{} where \"work\" or \"deck\" belongs", describe(other)),
            )),
            (other, _) => config.unknown.push(other.to_string()),
        }
    }
    config
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(_) => "a string".into(),
        Value::Integer(_) => "a whole number".into(),
        Value::Float(_) => "a decimal number".into(),
        Value::Boolean(_) => "true or false".into(),
        Value::Datetime(_) => "a date".into(),
        Value::Array(_) => "a list".into(),
        Value::Table(_) => "a table".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> BriefConfig {
        let value: Value = toml::from_str(text).unwrap();
        parse_brief(&value["brief"])
    }

    #[test]
    fn the_three_settings_are_read_as_written() {
        let c = read(
            "[brief]\naway_after_minutes = 45\nshow_on_attach = false\nesc_goes_to = \"deck\"\n",
        );
        assert!(c.present);
        assert_eq!(c.away_after_minutes, Some(45));
        assert_eq!(c.show_on_attach, Some(false));
        assert_eq!(c.esc_goes_to.as_deref(), Some("deck"));
        assert!(c.unknown.is_empty() && c.malformed.is_empty());
    }

    #[test]
    fn an_empty_section_is_present_and_says_nothing() {
        let c = read("[brief]\n");
        assert!(c.present);
        assert_eq!(c.away_after_minutes, None);
    }

    #[test]
    fn wrong_types_and_unknown_keys_are_kept_for_the_lint() {
        let c = read(
            "[brief]\naway_after_minutes = \"soon\"\nshow_on_attach = 1\nesc_goes_to = 2\ncolour = 1\n",
        );
        assert_eq!(c.away_after_minutes, None);
        assert_eq!(c.show_on_attach, None);
        assert_eq!(c.esc_goes_to, None);
        assert_eq!(c.unknown, ["colour"]);
        let mut keys: Vec<&str> = c.malformed.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["away_after_minutes", "esc_goes_to", "show_on_attach"]
        );
    }

    #[test]
    fn something_that_is_not_a_table_is_malformed() {
        let value: Value = toml::from_str("brief = 3\n").unwrap();
        assert_eq!(parse_brief(&value["brief"]).malformed.len(), 1);
    }
}
