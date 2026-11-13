//! The `[deck]` section of `config.toml`: which panels the Deck shows.
//!
//! This only reads what is written. What can be used is for the domain to
//! say (`swarmux_domain::console::resolve_deck`), so a mistake in `[deck]`
//! never stops the file from loading: the Deck draws with the defaults for
//! what could not be used, and says so.

use swarmux_domain::console::DeckConfig;
use toml::Value;

/// Reads the value of `[deck]`.
pub(crate) fn parse_deck(value: &Value) -> DeckConfig {
    let mut config = DeckConfig {
        present: true,
        ..DeckConfig::default()
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
            ("panels", Value::Array(items)) => {
                let names: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                if names.len() == items.len() {
                    config.panels = Some(names);
                } else {
                    let bad = items.iter().find(|v| !v.is_str()).map(describe);
                    config.malformed.push((
                        "panels".into(),
                        format!(
                            "a list with {} in it where panel names belong",
                            bad.unwrap_or_else(|| "something else".into())
                        ),
                    ));
                }
            }
            ("panels", other) => config.malformed.push((
                "panels".into(),
                format!("{} where a list of panel names belongs", describe(other)),
            )),
            ("direction", Value::String(s)) => config.direction = Some(s.clone()),
            ("direction", other) => config.malformed.push((
                "direction".into(),
                format!("{} where a direction (\"lr\") belongs", describe(other)),
            )),
            ("session_rows", Value::Integer(n)) => config.session_rows = Some(*n),
            ("session_rows", other) => config.malformed.push((
                "session_rows".into(),
                format!("{} where a whole number belongs", describe(other)),
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

    fn read(text: &str) -> DeckConfig {
        let value: Value = toml::from_str(text).unwrap();
        parse_deck(&value["deck"])
    }

    #[test]
    fn the_three_settings_are_read_as_written() {
        let c =
            read("[deck]\npanels = [\"graph\", \"guard\"]\ndirection = \"lr\"\nsession_rows = 4\n");
        assert!(c.present);
        assert_eq!(
            c.panels,
            Some(vec!["graph".to_string(), "guard".to_string()])
        );
        assert_eq!(c.direction.as_deref(), Some("lr"));
        assert_eq!(c.session_rows, Some(4));
        assert!(c.unknown.is_empty() && c.malformed.is_empty());
    }

    #[test]
    fn an_empty_section_is_present_and_says_nothing() {
        let c = read("[deck]\n");
        assert!(c.present);
        assert_eq!(c.panels, None);
    }

    #[test]
    fn wrong_types_and_unknown_keys_are_kept_for_the_lint() {
        let c = read(
            "[deck]\npanels = [\"graph\", 3]\ndirection = 4\nsession_rows = \"all\"\ncolour = 1\n",
        );
        assert_eq!(c.panels, None);
        assert_eq!(c.direction, None);
        assert_eq!(c.session_rows, None);
        assert_eq!(c.unknown, ["colour"]);
        let mut keys: Vec<&str> = c.malformed.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["direction", "panels", "session_rows"]);
        let panels = c.malformed.iter().find(|(k, _)| k == "panels").unwrap();
        assert!(panels.1.contains("a whole number"), "{}", panels.1);
    }

    #[test]
    fn something_that_is_not_a_table_is_malformed() {
        let value: Value = toml::from_str("deck = 3\n").unwrap();
        let c = parse_deck(&value["deck"]);
        assert_eq!(c.malformed.len(), 1);
    }
}
