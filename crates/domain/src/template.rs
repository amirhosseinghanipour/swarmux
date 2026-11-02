//! `{{ name }}` placeholders in prompt bodies.
//!
//! A placeholder is `{{`, optional whitespace, a name of ASCII letters,
//! digits or `_`, optional whitespace, `}}`. Anything else is plain text.

use std::collections::HashMap;

use crate::DomainError;

/// One placeholder: byte range in the body and the variable name.
struct Placeholder<'a> {
    start: usize,
    end: usize,
    name: &'a str,
}

fn is_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The first placeholder at or after byte `from`.
fn next_placeholder(body: &str, mut from: usize) -> Option<Placeholder<'_>> {
    while let Some(offset) = body[from..].find("{{") {
        let start = from + offset;
        let inner = start + 2;
        let close = inner + body[inner..].find("}}")?;
        let name = body[inner..close].trim();
        if is_name(name) {
            return Some(Placeholder {
                start,
                end: close + 2,
                name,
            });
        }
        from = start + 1;
    }
    None
}

fn placeholders(body: &str) -> impl Iterator<Item = Placeholder<'_>> {
    let mut from = 0;
    std::iter::from_fn(move || {
        let p = next_placeholder(body, from)?;
        from = p.end;
        Some(p)
    })
}

/// Variable names used in `body`, each once, in order of first appearance.
pub fn variables(body: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for p in placeholders(body) {
        if !names.iter().any(|n| n == p.name) {
            names.push(p.name.to_string());
        }
    }
    names
}

/// Replaces every placeholder with its value. Fails on the first one without a value.
pub fn render(body: &str, values: &HashMap<String, String>) -> Result<String, DomainError> {
    let mut out = String::with_capacity(body.len());
    let mut copied = 0;
    for p in placeholders(body) {
        let value = values
            .get(p.name)
            .ok_or_else(|| DomainError::MissingVariable(p.name.to_string()))?;
        out.push_str(&body[copied..p.start]);
        out.push_str(value);
        copied = p.end;
    }
    out.push_str(&body[copied..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn finds_unique_names_in_order() {
        assert_eq!(
            variables("{{b}} {{ a }} {{b}} {{\tc_1\n}}"),
            ["b", "a", "c_1"]
        );
    }

    #[test]
    fn ignores_malformed_braces() {
        assert!(variables("{{}} {{ }} {{a b}} {{a-b}} {a} {{x").is_empty());
        assert_eq!(variables("{{{x}}}"), ["x"]);
        assert_eq!(variables("{{ {{y}} }}"), ["y"]);
    }

    #[test]
    fn renders_repeated_and_spaced_placeholders() {
        let out = render("{{a}}-{{ a }}-{{b}}", &values(&[("a", "1"), ("b", "2")])).unwrap();
        assert_eq!(out, "1-1-2");
    }

    #[test]
    fn leaves_non_placeholders_untouched() {
        let body = "fn f() {{ x+1 }} {{}} {a} {{x";
        assert_eq!(render(body, &values(&[])).unwrap(), body);
        assert_eq!(render("{{{x}}}", &values(&[("x", "v")])).unwrap(), "{v}");
    }

    #[test]
    fn unicode_text_and_values() {
        let out = render("héllo {{who}} 🐝 — {{who}}", &values(&[("who", "мир")])).unwrap();
        assert_eq!(out, "héllo мир 🐝 — мир");
        assert_eq!(variables("é{{ ü }}{{ok}}"), ["ok"]);
    }

    #[test]
    fn missing_value_is_an_error() {
        assert_eq!(
            render("{{a}} {{b}}", &values(&[("a", "1")])),
            Err(DomainError::MissingVariable("b".into()))
        );
    }

    #[test]
    fn values_are_not_re_expanded() {
        let out = render("{{a}}", &values(&[("a", "{{b}}")])).unwrap();
        assert_eq!(out, "{{b}}");
    }
}
