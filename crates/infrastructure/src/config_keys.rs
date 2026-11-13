//! The `[keys]` section of `config.toml`: hotkeys.
//!
//! This only reads what is written. Whether the keys exist, clash or are
//! allowed is for the domain to say (`swarmux_domain::keys::apply`), so a
//! mistake in `[keys]` never stops the file from loading: the app starts,
//! with the defaults for what could not be used, and says so.

use swarmux_domain::keys::{KeyConfig, KeyEntry, Malformed};
use toml::Value;

/// Reads the value of `[keys]`.
pub(crate) fn parse_keys(value: &Value) -> KeyConfig {
    let mut config = KeyConfig::default();
    match value {
        Value::Table(table) => {
            for (name, value) in table {
                walk("", name, value, &mut config);
            }
        }
        other => config.malformed.push(Malformed {
            at: String::new(),
            found: format!("{} where a table of contexts belongs", describe(other)),
        }),
    }
    config
}

/// One line, or a table to look into: `section` is the tables around it.
fn walk(section: &str, name: &str, value: &Value, config: &mut KeyConfig) {
    let at = if section.is_empty() {
        name.to_string()
    } else {
        format!("{section}.{name}")
    };
    match value {
        Value::String(key) => config.entries.push(KeyEntry {
            section: section.to_string(),
            name: name.to_string(),
            keys: vec![key.clone()],
        }),
        Value::Array(items) => {
            let keys: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if keys.len() == items.len() {
                config.entries.push(KeyEntry {
                    section: section.to_string(),
                    name: name.to_string(),
                    keys,
                });
            } else {
                let bad = items.iter().find(|v| !v.is_str()).map(describe);
                config.malformed.push(Malformed {
                    at,
                    found: format!(
                        "a list with {} in it",
                        bad.unwrap_or_else(|| "something else".into())
                    ),
                });
            }
        }
        // `[keys.mission.wall]` is a table in a table: the context is
        // "mission.wall", and so are `[keys."mission.wall"]` and a dotted
        // action name.
        Value::Table(table) => {
            for (inner, value) in table {
                walk(&at, inner, value, config);
            }
        }
        other => config.malformed.push(Malformed {
            at,
            found: describe(other),
        }),
    }
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(_) => "a string".into(),
        Value::Integer(n) => format!("the number {n}"),
        Value::Float(n) => format!("the number {n}"),
        Value::Boolean(b) => format!("`{b}`"),
        Value::Datetime(_) => "a date".into(),
        Value::Array(_) => "a list".into(),
        Value::Table(_) => "a table".into(),
    }
}
