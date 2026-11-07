//! OpenAPI (and Swagger) documents: the endpoints (`GET /orders/{}`) and
//! the schemas a service publishes. JSON is read with a real parser; YAML
//! with a line reader that understands the block style tools write (no
//! flow style `{a: b}` maps, no anchors, no `$ref`ed path items).

use crate::json::Json;

use super::{Extractor, Fact, SymKind, Symbol};

/// Reads `openapi*.{json,yaml,yml}` and `swagger*.{json,yaml,yml}`.
pub struct OpenApiExtractor;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

impl Extractor for OpenApiExtractor {
    fn id(&self) -> &'static str {
        "openapi"
    }

    fn applies(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        let doc = name.ends_with(".json") || name.ends_with(".yaml") || name.ends_with(".yml");
        doc && (name.contains("openapi") || name.contains("swagger"))
    }

    fn extract(&self, path: &str, text: &str) -> Vec<Fact> {
        if path.to_ascii_lowercase().ends_with(".json") {
            from_json(text)
        } else {
            from_yaml(text)
        }
    }
}

/// `/orders/{id}` as `/orders/{}`.
pub fn canonical_path(path: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in path.trim().chars() {
        match c {
            '{' => {
                depth += 1;
                if depth == 1 {
                    out.push_str("{}");
                }
            }
            '}' => depth = (depth - 1).max(0),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    if out.len() > 1 {
        out = out.trim_end_matches('/').to_string();
    }
    out
}

fn endpoint(method: &str, path: &str) -> Symbol {
    Symbol::new(
        SymKind::Endpoint,
        &format!("{} {}", method.to_ascii_uppercase(), canonical_path(path)),
    )
}

fn line_of(text: &str, needle: &str) -> u32 {
    text.find(needle)
        .map_or(1, |at| 1 + text[..at].matches('\n').count() as u32)
}

fn from_json(text: &str) -> Vec<Fact> {
    let Ok(doc) = Json::parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(Json::Obj(paths)) = doc.get("paths") {
        for (path, item) in paths {
            let line = line_of(text, &format!("\"{path}\""));
            for method in METHODS {
                let Some(op) = item.get(method) else {
                    continue;
                };
                let mut parts = Vec::new();
                if let Some(id) = op.get("operationId").and_then(Json::as_str) {
                    parts.push(format!("operationId {id}"));
                }
                if let Some(Json::Arr(params)) = op.get("parameters") {
                    let mut names: Vec<String> = params
                        .iter()
                        .filter_map(|p| p.get("name").and_then(Json::as_str))
                        .map(str::to_string)
                        .collect();
                    names.sort();
                    parts.push(format!("params {}", names.join(",")));
                }
                if op.get("requestBody").is_some() {
                    parts.push("body".into());
                }
                out.push(Fact::provide(
                    endpoint(method, path),
                    line,
                    &parts.join("; "),
                ));
            }
        }
    }
    let schemas = doc
        .get("components")
        .and_then(|c| c.get("schemas"))
        .or_else(|| doc.get("definitions"));
    if let Some(Json::Obj(schemas)) = schemas {
        for (name, schema) in schemas {
            let mut props: Vec<String> = match schema.get("properties") {
                Some(Json::Obj(p)) => p.iter().map(|(k, _)| k.clone()).collect(),
                _ => Vec::new(),
            };
            props.sort();
            out.push(Fact::provide(
                Symbol::new(SymKind::Schema, name),
                line_of(text, &format!("\"{name}\"")),
                &format!("properties {}", props.join(",")),
            ));
        }
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The key of a `key:` / `key: value` line, unquoted.
fn key_of(trimmed: &str) -> Option<(String, &str)> {
    if trimmed.starts_with('#') || trimmed.starts_with("- ") {
        return None;
    }
    let (key, rest) = if let Some(q) = trimmed.strip_prefix(['\'', '"']) {
        let end = q.find(['\'', '"'])?;
        (&q[..end], q[end + 1..].strip_prefix(':')?)
    } else {
        trimmed.split_once(':')?
    };
    Some((key.trim().to_string(), rest.trim()))
}

fn from_yaml(text: &str) -> Vec<Fact> {
    let mut out = Vec::new();
    // The section of the document: `paths`, `schemas` (components or
    // definitions), or something else.
    let mut section = "";
    let mut in_components = false;
    let mut path: Option<(String, usize)> = None;
    // The open operation: method, its line, indent, details.
    let mut op: Option<(String, u32, usize, Vec<String>)> = None;
    let mut schema: Option<(String, u32, usize, Vec<String>)> = None;
    let mut props_indent: Option<usize> = None;

    let close_op = |op: &mut Option<(String, u32, usize, Vec<String>)>,
                    path: &Option<(String, usize)>,
                    out: &mut Vec<Fact>| {
        if let (Some((m, line, _, parts)), Some((p, _))) = (op.take(), path) {
            out.push(Fact::provide(endpoint(&m, p), line, &parts.join("; ")));
        }
    };
    let close_schema = |schema: &mut Option<(String, u32, usize, Vec<String>)>,
                        out: &mut Vec<Fact>| {
        if let Some((name, line, _, mut props)) = schema.take() {
            props.sort();
            out.push(Fact::provide(
                Symbol::new(SymKind::Schema, &name),
                line,
                &format!("properties {}", props.join(",")),
            ));
        }
    };

    for (n, raw) in text.lines().enumerate() {
        let line_no = n as u32 + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let ind = indent_of(raw);
        if ind == 0 {
            close_op(&mut op, &path, &mut out);
            close_schema(&mut schema, &mut out);
            path = None;
            props_indent = None;
            let key = key_of(trimmed).map(|(k, _)| k).unwrap_or_default();
            in_components = key == "components";
            section = match key.as_str() {
                "paths" => "paths",
                "definitions" => "schemas",
                _ => "",
            };
            continue;
        }
        if in_components && ind == 2 {
            close_schema(&mut schema, &mut out);
            section = if key_of(trimmed).is_some_and(|(k, _)| k == "schemas") {
                "schemas"
            } else {
                ""
            };
            continue;
        }
        match section {
            "paths" => {
                if let Some((_, pind)) = &path
                    && ind > *pind
                {
                    if let Some((_, _, oind, parts)) = &mut op
                        && ind > *oind
                    {
                        if let Some((k, v)) = key_of(trimmed) {
                            if k == "operationId" {
                                parts.push(format!("operationId {}", v.trim_matches(['\'', '"'])));
                            }
                        } else if let Some(name) = trimmed.strip_prefix("- name:") {
                            parts.push(format!("param {}", name.trim().trim_matches(['\'', '"'])));
                        }
                        continue;
                    }
                    close_op(&mut op, &path, &mut out);
                    if let Some((k, _)) = key_of(trimmed)
                        && METHODS.contains(&k.to_ascii_lowercase().as_str())
                    {
                        op = Some((k.to_ascii_lowercase(), line_no, ind, Vec::new()));
                    }
                    continue;
                }
                close_op(&mut op, &path, &mut out);
                path = key_of(trimmed)
                    .filter(|(k, _)| k.starts_with('/'))
                    .map(|(k, _)| (k, ind));
            }
            "schemas" => {
                if let Some((_, _, sind, props)) = &mut schema
                    && ind > *sind
                {
                    if let Some((k, _)) = key_of(trimmed) {
                        if k == "properties" {
                            props_indent = Some(ind + 2);
                        } else if props_indent == Some(ind) {
                            props.push(k);
                        }
                    }
                    continue;
                }
                close_schema(&mut schema, &mut out);
                props_indent = None;
                if let Some((k, _)) = key_of(trimmed) {
                    schema = Some((k, line_no, ind, Vec::new()));
                }
            }
            _ => {}
        }
    }
    close_op(&mut op, &path, &mut out);
    close_schema(&mut schema, &mut out);
    out
}
