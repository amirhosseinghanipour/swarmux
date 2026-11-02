//! Adds guard rules to `config.toml` without disturbing the rest of the file.
//!
//! Only plain layouts are edited (a `[guard]` header and an `allow = [...]`
//! array); anything else is refused. Every edit is re-parsed and checked
//! before the file is replaced.

use std::io::ErrorKind;
use std::path::PathBuf;

use swarmux_application::ports::RuleWriter;
use swarmux_application::{AppError, AppResult};
use toml::{Table, Value};

/// `RuleWriter` for the user's `config.toml`.
pub struct ConfigRuleWriter {
    path: PathBuf,
}

impl ConfigRuleWriter {
    pub fn new(path: PathBuf) -> ConfigRuleWriter {
        ConfigRuleWriter { path }
    }
}

impl RuleWriter for ConfigRuleWriter {
    fn add_allow_rule(&self, rule: &str) -> AppResult<()> {
        let fail = |m: String| AppError::Config(format!("{}: {m}", self.path.display()));
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(fail(e.to_string())),
        };
        let Some(updated) = add_allow(&text, rule).map_err(fail)? else {
            return Ok(());
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, updated).map_err(|e| fail(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| fail(e.to_string()))
    }
}

/// The config text with `rule` added to `[guard] allow`, or `None` if it is
/// already there. Fails when the file's layout is too unusual to edit safely.
pub fn add_allow(text: &str, rule: &str) -> Result<Option<String>, String> {
    let before: Table = toml::from_str(text).map_err(|_| "the file has errors".to_string())?;
    let guard = before.get("guard");
    let allow = guard.and_then(|g| g.get("allow"));
    if let Some(Value::Array(list)) = allow
        && list.iter().any(|v| v.as_str() == Some(rule))
    {
        return Ok(None);
    }
    let item = quote(rule);
    let updated = match (guard, allow) {
        (None, _) => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[guard]\nallow = [{item}]\n"));
            out
        }
        (Some(_), None) => {
            let header = guard_header(text).ok_or("[guard] isn't a plain table")?;
            let at = line_end(text, header);
            format!("{}allow = [{item}]\n{}", &text[..at], &text[at..])
        }
        (Some(_), Some(_)) => insert_into_array(text, &item)?,
    };
    check(&before, &updated, rule)?;
    Ok(Some(updated))
}

/// Byte offset of the line holding the `[guard]` header.
fn guard_header(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("[guard]")
            && (rest.trim().is_empty() || rest.trim_start().starts_with('#'))
        {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

/// Offset just past the end of the line starting at `at`.
fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i + 1)
}

/// Adds `item` after the last element of the `allow` array in `[guard]`.
fn insert_into_array(text: &str, item: &str) -> Result<String, String> {
    let complex = || "the allow list is laid out in a way swarmux can't edit".to_string();
    let header = guard_header(text).ok_or_else(complex)?;
    let mut offset = line_end(text, header);
    let mut open = None;
    for line in text[offset..].split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with('[') {
            break;
        }
        if let Some(rest) = t.strip_prefix("allow")
            && rest.trim_start().starts_with('=')
        {
            let eq = offset + line.find('=').ok_or_else(complex)?;
            let bracket = text[eq + 1..]
                .find(|c: char| !c.is_whitespace())
                .map(|i| eq + 1 + i)
                .filter(|&i| text.as_bytes()[i] == b'[')
                .ok_or_else(complex)?;
            open = Some(bracket);
            break;
        }
        offset += line.len();
    }
    let open = open.ok_or_else(complex)?;
    let (last, close) = scan_array(text, open).ok_or_else(complex)?;
    let at = last.map_or(open + 1, |i| i + 1);
    let insert = match last.map(|i| text.as_bytes()[i]) {
        None => item.to_string(),
        Some(b',') => format!(" {item},"),
        Some(_) => format!(", {item}"),
    };
    debug_assert!(at <= close);
    Ok(format!("{}{insert}{}", &text[..at], &text[at..]))
}

/// From the `[` at `open`: the offset of the last significant character
/// inside the array (if any) and of the closing `]`.
fn scan_array(text: &str, open: usize) -> Option<(Option<usize>, usize)> {
    let bytes = text.as_bytes();
    let mut i = open + 1;
    let mut last = None;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            b'"' | b'\'' => {
                let q = bytes[i];
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != q {
                    if q == b'"' && bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i >= bytes.len() || text[start..=i].starts_with("\"\"\"") {
                    return None;
                }
                last = Some(i);
            }
            b'[' => {
                depth += 1;
                last = Some(i);
            }
            b']' if depth == 0 => return Some((last, i)),
            b']' => {
                depth -= 1;
                last = Some(i);
            }
            c if c.is_ascii_whitespace() => {}
            _ => last = Some(i),
        }
        i += 1;
    }
    None
}

/// The edit added exactly `rule` and changed nothing else.
fn check(before: &Table, updated: &str, rule: &str) -> Result<(), String> {
    let refused = || "the edit couldn't be checked, so it wasn't saved".to_string();
    let mut after: Table = toml::from_str(updated).map_err(|_| refused())?;
    let had_guard = before.contains_key("guard");
    let had_allow = before.get("guard").and_then(|g| g.get("allow")).is_some();
    let guard = after
        .get_mut("guard")
        .and_then(Value::as_table_mut)
        .ok_or_else(refused)?;
    let allow = guard
        .get_mut("allow")
        .and_then(Value::as_array_mut)
        .ok_or_else(refused)?;
    let pos = allow
        .iter()
        .rposition(|v| v.as_str() == Some(rule))
        .ok_or_else(refused)?;
    allow.remove(pos);
    if !had_allow && allow.is_empty() {
        guard.remove("allow");
    }
    if !had_guard && guard.is_empty() {
        after.remove("guard");
    }
    (after == *before).then_some(()).ok_or_else(refused)
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_of(text: &str) -> Vec<String> {
        let t: Table = toml::from_str(text).unwrap();
        t["guard"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn creates_the_section() {
        let out = add_allow("max_concurrent = 2", "Bash(make)")
            .unwrap()
            .unwrap();
        assert_eq!(
            out,
            "max_concurrent = 2\n\n[guard]\nallow = [\"Bash(make)\"]\n"
        );
        let empty = add_allow("", "Bash(make)").unwrap().unwrap();
        assert_eq!(empty, "[guard]\nallow = [\"Bash(make)\"]\n");
    }

    #[test]
    fn adds_the_key_under_an_existing_header() {
        let text = "[guard] # mine\nunmatched = \"ask\"\n";
        let out = add_allow(text, "Read").unwrap().unwrap();
        assert_eq!(
            out,
            "[guard] # mine\nallow = [\"Read\"]\nunmatched = \"ask\"\n"
        );
    }

    #[test]
    fn appends_to_one_line_and_multi_line_arrays() {
        let one = "[guard]\nallow = [\"A\"] # keep\ndeny = [\"B\"]\n";
        let out = add_allow(one, "C").unwrap().unwrap();
        assert!(out.contains("allow = [\"A\", \"C\"] # keep"), "{out}");
        assert_eq!(allow_of(&out), ["A", "C"]);

        let multi = "[guard]\nallow = [\n  \"A\", # first\n  \"B(x])\",\n]\n";
        let out = add_allow(multi, "C").unwrap().unwrap();
        assert_eq!(allow_of(&out), ["A", "B(x])", "C"]);

        let empty = "[guard]\nallow = []\n";
        assert_eq!(allow_of(&add_allow(empty, "C").unwrap().unwrap()), ["C"]);
    }

    #[test]
    fn existing_rule_is_left_alone() {
        assert_eq!(add_allow("[guard]\nallow = [\"A\"]", "A").unwrap(), None);
    }

    #[test]
    fn quotes_and_backslashes_survive() {
        let rule = r#"Bash(echo "a\b")"#;
        let out = add_allow("", rule).unwrap().unwrap();
        assert_eq!(allow_of(&out), [rule]);
    }

    #[test]
    fn unusual_layouts_are_refused() {
        assert!(add_allow("guard = { allow = [\"A\"] }", "B").is_err());
        assert!(add_allow("guard.allow = [\"A\"]", "B").is_err());
        assert!(add_allow("not toml [", "B").is_err());
    }

    #[test]
    fn writer_updates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let w = ConfigRuleWriter::new(path.clone());
        w.add_allow_rule("Bash(make)").unwrap();
        w.add_allow_rule("Read(x)").unwrap();
        w.add_allow_rule("Bash(make)").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(allow_of(&text), ["Bash(make)", "Read(x)"]);
        assert!(crate::config::load_settings(&path).is_ok());
    }
}
