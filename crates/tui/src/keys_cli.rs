//! What `swarmux keys list`, `lint` and `export` print: plain text over
//! the effective keymap, so there is nothing to colour and `NO_COLOR` has
//! nothing to turn off.

use swarmux_domain::keys::{key_name, table_name};

use crate::keys::{Effective, keys_text};

/// Every action with its context, its keys now and by default. A changed
/// one is marked `changed`, one whose keys can't be changed `fixed`.
pub fn list(found: &Effective) -> String {
    let mut rows: Vec<[String; 5]> = vec![[
        "CONTEXT".into(),
        "ACTION".into(),
        "KEYS".into(),
        "DEFAULT".into(),
        "".into(),
    ]];
    for context in found.catalog.contexts() {
        for def in found.catalog.of(context) {
            let now = keys_text(&found.keymap, context, def.action);
            let default = keys_text(&found.defaults, context, def.action);
            let note = if def.locked {
                "fixed"
            } else if now != default {
                "changed"
            } else {
                ""
            };
            rows.push([
                context.id().to_string(),
                def.action.id().to_string(),
                now,
                default,
                note.to_string(),
            ]);
        }
    }
    let widths: Vec<usize> = (0..4)
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for r in rows {
        let line = format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// The report of `swarmux keys lint` and whether anything in it is an
/// error (the command then exits with failure). `configured` is whether
/// the config has a `[keys]` section at all.
pub fn lint(found: &Effective, source: &str, configured: bool) -> (String, bool) {
    let mut out = String::new();
    for problem in &found.problems {
        out.push_str(&format!("error: {problem}\n"));
    }
    for shadow in &found.warnings {
        out.push_str(&format!("warning: {shadow}\n"));
    }
    let (errors, warnings) = (found.problems.len(), found.warnings.len());
    if !configured {
        out.push_str(&format!(
            "keys: no [keys] in {source}: the default keys apply\n"
        ));
    } else if errors == 0 && warnings == 0 {
        out.push_str(&format!("keys: {source} is fine\n"));
    } else {
        out.push_str(&format!(
            "keys: {errors} error{}, {warnings} warning{} in {source}\n",
            if errors == 1 { "" } else { "s" },
            if warnings == 1 { "" } else { "s" },
        ));
    }
    if errors > 0 {
        out.push_str(
            "      swarmux ignores each entry above and keeps the default keys for its action.\n\
             The answers to approvals (y, n, a, c) and Esc can't be rebound, on purpose.\n",
        );
    }
    (out, errors > 0)
}

/// The keys in use, as TOML to paste into `config.toml`. Actions whose
/// keys are fixed are left out: a config can't change them.
pub fn export(found: &Effective) -> String {
    let mut out = String::from(
        "# The keys swarmux uses now, defaults included: paste this into config.toml.\n\
         # An action with [] has no key. Esc, Ctrl+C and the answers to approvals\n\
         # (y, n, a, c) are fixed and not listed.\n",
    );
    for context in found.catalog.contexts() {
        let defs: Vec<_> = found.catalog.of(context).filter(|d| !d.locked).collect();
        if defs.is_empty() {
            continue;
        }
        out.push_str(&format!("\n[keys.{}]\n", table_name(context.id())));
        for def in defs {
            let chords: Vec<String> = found
                .keymap
                .chords_of(context, def.action)
                .map(|c| quoted(&c.to_string()))
                .collect();
            let value = match chords.as_slice() {
                [one] => one.clone(),
                many => format!("[{}]", many.join(", ")),
            };
            out.push_str(&format!("{} = {value}\n", key_name(def.action.id())));
        }
    }
    out
}

/// `text` as a TOML basic string.
fn quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests;
