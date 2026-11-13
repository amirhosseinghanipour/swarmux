//! What a change to a project manifest or a lockfile does: which of it is
//! harness (a `[patch]` section, a test script, a pytest entry point, a
//! build script) and which is only dependencies, and which packages a
//! lockfile gained.
//!
//! Pure text: the manifests are read with small line scanners, not parsed
//! with a full TOML or YAML reader. They err towards reporting: a line the
//! scanner cannot place is harness.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::FileDiff;
use crate::json::Json;

/// What a manifest change contains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestChange {
    /// Changes that are not dependencies and not harmless metadata, as a
    /// few words each.
    pub harness: Vec<String>,
    /// Names of the dependencies added or changed.
    pub dependencies: Vec<String>,
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What changed between `old` and `new` content of the manifest `path`;
/// `None` when `path` is no manifest this knows or a side is missing.
pub fn analyze(path: &str, old: Option<&str>, new: Option<&str>) -> Option<ManifestChange> {
    let (old, new) = (old?, new?);
    match file_name(path) {
        "Cargo.toml" => Some(toml_change(old, new, Flavor::Cargo)),
        "pyproject.toml" => Some(toml_change(old, new, Flavor::Python)),
        "package.json" => Some(package_json(old, new)),
        "pubspec.yaml" => Some(pubspec(old, new)),
        "go.mod" => Some(go_mod(old, new)),
        _ => None,
    }
}

// --- TOML (Cargo.toml, pyproject.toml) --------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Cargo,
    Python,
}

type Entries = BTreeMap<(String, String), String>;

/// Net depth of brackets and braces on `line`, outside strings.
fn depth_change(line: &str) -> i32 {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => break,
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn is_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '"' | '\''))
}

/// The `(table, key)` entries of a TOML file with each value's text.
/// Repeated `[[tables]]` get `#n` after their name.
fn toml_entries(text: &str) -> Entries {
    let mut out = Entries::new();
    let mut table = String::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut current: Option<(String, String)> = None;
    let mut depth = 0;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if depth > 0 {
            if let Some(key) = &current {
                let e = out.entry(key.clone()).or_default();
                e.push(' ');
                e.push_str(t);
            }
            depth += depth_change(t);
            continue;
        }
        if let Some(name) = t
            .strip_prefix("[[")
            .and_then(|r| r.split("]]").next())
            .filter(|_| t.starts_with("[["))
        {
            let n = counts.entry(name.trim().to_string()).or_default();
            table = format!("{}#{n}", name.trim());
            *n += 1;
            current = None;
            continue;
        }
        if t.starts_with('[')
            && let Some(name) = t[1..].split(']').next()
            && !name.contains(',')
        {
            table = name.trim().to_string();
            current = None;
            continue;
        }
        if let Some((key, value)) = t.split_once('=')
            && is_key(key.trim())
        {
            let k = (table.clone(), key.trim().to_string());
            out.insert(k.clone(), value.trim().to_string());
            depth = depth_change(value);
            current = Some(k);
            continue;
        }
        // Something this cannot place: keep it, so a change to it shows.
        let e = out.entry((table.clone(), "?".to_string())).or_default();
        e.push(' ');
        e.push_str(t);
    }
    out
}

/// The package names in the quoted strings of an array value.
fn quoted_names(value: &str) -> BTreeSet<String> {
    static STR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']([^"']+)["']"#).unwrap());
    STR.captures_iter(value)
        .filter_map(|c| {
            let s = c.get(1)?.as_str();
            let name: String = s
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// The strings of an array value, whole.
fn quoted_strings(value: &str) -> BTreeSet<String> {
    static STR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["']([^"']+)["']"#).unwrap());
    STR.captures_iter(value)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// For a table that holds dependencies: whether its keys are package names
/// (`None`) or it is one package's own table (`Some(name)`).
fn dependency_table(table: &str, flavor: Flavor) -> Option<Option<String>> {
    let t = table.split('#').next().unwrap_or(table);
    let markers: &[&str] = match flavor {
        Flavor::Cargo => &["dependencies", "dev-dependencies", "build-dependencies"],
        Flavor::Python => &[
            "tool.poetry.dependencies",
            "tool.poetry.dev-dependencies",
            "tool.pdm.dev-dependencies",
            "dependency-groups",
            "project.optional-dependencies",
        ],
    };
    if flavor == Flavor::Python {
        if markers.contains(&t) {
            return Some(None);
        }
        let group = t.starts_with("tool.poetry.group.") && t.ends_with(".dependencies");
        return group.then_some(None);
    }
    for m in markers {
        if t == *m || t.ends_with(&format!(".{m}")) {
            return Some(None);
        }
        let inner = format!("{m}.");
        if let Some(at) = t.rfind(&inner)
            && (at == 0 || t.as_bytes()[at - 1] == b'.')
        {
            return Some(Some(
                t[at + inner.len()..].trim_matches(['"', '\'']).to_string(),
            ));
        }
    }
    None
}

/// Keys whose change is no harness: who the package is and what it says
/// about itself.
fn benign_toml(table: &str, key: &str, flavor: Flavor) -> bool {
    let key = key.trim_matches(['"', '\'']);
    let meta = [
        "version",
        "description",
        "authors",
        "maintainers",
        "license",
        "license-file",
        "license-files",
        "repository",
        "homepage",
        "documentation",
        "readme",
        "keywords",
        "categories",
        "classifiers",
        "exclude",
        "include",
        "rust-version",
    ];
    match flavor {
        Flavor::Cargo => matches!(table, "package" | "workspace.package") && meta.contains(&key),
        Flavor::Python => {
            (matches!(table, "project" | "tool.poetry") && meta.contains(&key))
                || matches!(table, "project.urls" | "tool.poetry.urls")
        }
    }
}

fn toml_change(old: &str, new: &str, flavor: Flavor) -> ManifestChange {
    let (a, b) = (toml_entries(old), toml_entries(new));
    let mut out = ManifestChange::default();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut harness: BTreeSet<String> = BTreeSet::new();
    let keys: BTreeSet<&(String, String)> = a.keys().chain(b.keys()).collect();
    for k in keys {
        let (before, after) = (a.get(k), b.get(k));
        if before == after {
            continue;
        }
        let (table, key) = (&k.0, &k.1);
        // Dependencies: a table of packages, one package's own table, or
        // (Python) an array of requirement strings.
        if let Some(sub) = dependency_table(table, flavor) {
            if after.is_some() {
                names.insert(sub.unwrap_or_else(|| key.trim_matches(['"', '\'']).to_string()));
            }
            continue;
        }
        let python_array = flavor == Flavor::Python
            && ((table == "project" && key == "dependencies")
                || (table == "tool.uv" && key == "dev-dependencies")
                || (table.starts_with("tool.hatch.envs.") && key == "dependencies"));
        if python_array {
            let before_strings = before.map(|v| quoted_strings(v)).unwrap_or_default();
            if let Some(after) = after {
                for s in quoted_strings(after).difference(&before_strings) {
                    let one = quoted_names(&format!("\"{s}\""));
                    names.extend(one);
                }
            }
            continue;
        }
        if benign_toml(table, key, flavor) {
            continue;
        }
        let base = table.split('#').next().unwrap_or(table);
        let what = if flavor == Flavor::Cargo
            && (base == "patch" || base.starts_with("patch.") || base == "replace")
        {
            "a [patch] or [replace] section redirects dependencies".to_string()
        } else if table.is_empty() {
            format!("`{key}`")
        } else {
            format!("[{base}] `{key}`")
        };
        harness.insert(what);
    }
    out.dependencies = names.into_iter().collect();
    out.harness = harness.into_iter().collect();
    out
}

// --- package.json -----------------------------------------------------------

const NPM_DEPENDENCIES: [&str; 5] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
    "bundleDependencies",
];
const NPM_BENIGN: [&str; 11] = [
    "name",
    "version",
    "description",
    "keywords",
    "author",
    "contributors",
    "license",
    "repository",
    "bugs",
    "homepage",
    "funding",
];

fn package_json(old: &str, new: &str) -> ManifestChange {
    let mut out = ManifestChange::default();
    let (Ok(a), Ok(b)) = (Json::parse(old), Json::parse(new)) else {
        out.harness.push("package.json does not parse".into());
        return out;
    };
    let (Json::Obj(a), Json::Obj(b)) = (&a, &b) else {
        out.harness.push("package.json is not an object".into());
        return out;
    };
    let keys: BTreeSet<&str> = a.iter().chain(b).map(|(k, _)| k.as_str()).collect();
    let get = |fields: &'_ [(String, Json)], k: &str| {
        fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    };
    let mut names = BTreeSet::new();
    for key in keys {
        let (before, after) = (get(a, key), get(b, key));
        if before == after || NPM_BENIGN.contains(&key) {
            continue;
        }
        if NPM_DEPENDENCIES.contains(&key) {
            match (&before, &after) {
                (_, Some(Json::Obj(now))) => {
                    let was = match &before {
                        Some(Json::Obj(f)) => f.clone(),
                        _ => Vec::new(),
                    };
                    for (name, v) in now {
                        if get(&was, name).as_ref() != Some(v) {
                            names.insert(name.clone());
                        }
                    }
                }
                (_, Some(Json::Arr(items))) => {
                    names.extend(items.iter().filter_map(|i| i.as_str().map(str::to_string)));
                }
                _ => {}
            }
            continue;
        }
        match (&before, &after) {
            (Some(Json::Obj(x)), Some(Json::Obj(y))) => {
                let inner: BTreeSet<&str> = x.iter().chain(y).map(|(k, _)| k.as_str()).collect();
                for k in inner {
                    if get(x, k) != get(y, k) {
                        out.harness.push(format!("`{key}.{k}`"));
                    }
                }
            }
            _ => out.harness.push(format!("`{key}`")),
        }
    }
    out.dependencies = names.into_iter().collect();
    out
}

// --- pubspec.yaml -----------------------------------------------------------

/// Top-level keys of a pubspec that say nothing about running tests.
const PUB_BENIGN: [&str; 11] = [
    "description",
    "version",
    "homepage",
    "repository",
    "issue_tracker",
    "documentation",
    "publish_to",
    "flutter",
    "topics",
    "funding",
    "screenshots",
];

/// `(section, entry)` → text for a YAML file two levels deep: a top-level
/// key, and the keys indented below it.
fn yaml_entries(text: &str) -> Entries {
    let mut out = Entries::new();
    let mut section = String::new();
    let mut entry: Option<(String, String)> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        let t = trimmed.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - t.len();
        let (key, value) = t.split_once(':').unwrap_or((t, ""));
        if indent == 0 {
            section = key.trim().to_string();
            let k = (section.clone(), String::new());
            out.insert(k.clone(), value.trim().to_string());
            entry = Some(k);
        } else if indent > 2 && entry.is_some() {
            if let Some(k) = &entry {
                let e = out.entry(k.clone()).or_default();
                e.push(' ');
                e.push_str(t);
            }
        } else {
            let k = (section.clone(), key.trim().to_string());
            out.insert(k.clone(), value.trim().to_string());
            entry = Some(k);
        }
    }
    out
}

fn pubspec(old: &str, new: &str) -> ManifestChange {
    let (a, b) = (yaml_entries(old), yaml_entries(new));
    let mut out = ManifestChange::default();
    let mut names = BTreeSet::new();
    let mut harness = BTreeSet::new();
    let keys: BTreeSet<&(String, String)> = a.keys().chain(b.keys()).collect();
    for k in keys {
        if a.get(k) == b.get(k) {
            continue;
        }
        let (section, key) = (&k.0, &k.1);
        match section.as_str() {
            "dependencies" | "dev_dependencies" if !key.is_empty() => {
                if b.contains_key(k) {
                    names.insert(key.clone());
                }
            }
            "dependencies" | "dev_dependencies" => {}
            s if PUB_BENIGN.contains(&s) => {}
            "dependency_overrides" => {
                harness.insert("`dependency_overrides` redirects dependencies".to_string());
            }
            s => {
                harness.insert(format!("`{s}`"));
            }
        }
    }
    out.dependencies = names.into_iter().collect();
    out.harness = harness.into_iter().collect();
    out
}

// --- go.mod -----------------------------------------------------------------

/// `(kind, text)` of each directive of a `go.mod`; `require` and `exclude`
/// blocks are flattened.
fn go_lines(text: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut block: Option<String> = None;
    for line in text.lines() {
        let t = line.split("//").next().unwrap_or(line).trim();
        if t.is_empty() {
            continue;
        }
        if let Some(kind) = &block {
            if t == ")" {
                block = None;
            } else {
                out.insert((kind.clone(), t.to_string()));
            }
            continue;
        }
        let (word, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
        let rest = rest.trim();
        if rest == "(" {
            block = Some(word.to_string());
        } else {
            out.insert((word.to_string(), rest.to_string()));
        }
    }
    out
}

fn go_mod(old: &str, new: &str) -> ManifestChange {
    let (a, b) = (go_lines(old), go_lines(new));
    let mut out = ManifestChange::default();
    let mut names = BTreeSet::new();
    let mut harness = BTreeSet::new();
    for (kind, text) in a.symmetric_difference(&b) {
        let now = b.contains(&(kind.clone(), text.clone()));
        match kind.as_str() {
            "require" => {
                if now && let Some(name) = text.split_whitespace().next() {
                    names.insert(name.to_string());
                }
            }
            "replace" => {
                harness.insert("`replace` redirects a module".to_string());
            }
            other => {
                harness.insert(format!("`{other}`"));
            }
        }
    }
    out.dependencies = names.into_iter().collect();
    out.harness = harness.into_iter().collect();
    out
}

// --- lockfiles --------------------------------------------------------------

/// Reads a package name off one line of a lockfile.
type Extractor = Box<dyn Fn(&str) -> Option<String>>;

/// The packages `f` (a lockfile or requirement list) gained, by name;
/// `None` when `f` is no lockfile this knows.
pub fn lock_dependencies(f: &FileDiff) -> Option<Vec<String>> {
    static NAME_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^name\s*=\s*"([^"]+)""#).unwrap());
    static NPM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^\s*"(node_modules/[^"]+)"\s*:\s*\{"#).unwrap());
    static PNPM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^  /?(@?[A-Za-z0-9._\-]+(?:/[A-Za-z0-9._\-]+)?)@[^:\s]+:\s*$").unwrap()
    });
    static PIPFILE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^ {8}"([A-Za-z0-9._\-]+)"\s*:\s*\{"#).unwrap());
    static PUB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^  ([A-Za-z0-9_]+):\s*$").unwrap());
    static REQ: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._\-]*)\s*(?:\[[^\]]*\])?\s*(?:[<>=!~;@].*)?$").unwrap()
    });
    let name = file_name(&f.path);
    let extract: Extractor = match name {
        "Cargo.lock" | "poetry.lock" | "uv.lock" => {
            Box::new(|l| NAME_LINE.captures(l).map(|c| c[1].to_string()))
        }
        "package-lock.json" | "npm-shrinkwrap.json" => Box::new(|l| {
            NPM.captures(l).map(|c| {
                c[1].rsplit("node_modules/")
                    .next()
                    .unwrap_or(&c[1])
                    .to_string()
            })
        }),
        "pnpm-lock.yaml" => Box::new(|l| PNPM.captures(l).map(|c| c[1].to_string())),
        "yarn.lock" => Box::new(|l| {
            if l.starts_with([' ', '#', '\t']) || !l.ends_with(':') || l.starts_with("__metadata") {
                return None;
            }
            let first = l.trim_end_matches(':').split(',').next()?.trim();
            let first = first.trim_matches('"');
            let at = first[1.min(first.len())..].find('@')? + 1.min(first.len());
            Some(first[..at].to_string())
        }),
        "Pipfile.lock" => Box::new(|l| PIPFILE.captures(l).map(|c| c[1].to_string())),
        "go.sum" => Box::new(|l| {
            let mut w = l.split_whitespace();
            let module = w.next()?;
            (w.next().is_some() && l.contains("h1:")).then(|| module.to_string())
        }),
        "pubspec.lock" => Box::new(|l| PUB.captures(l).map(|c| c[1].to_string())),
        n if n.starts_with("requirements") && n.ends_with(".txt") => Box::new(|l| {
            let t = l.trim();
            if t.starts_with(['#', '-']) {
                return None;
            }
            REQ.captures(t).map(|c| c[1].to_ascii_lowercase())
        }),
        _ => return None,
    };
    let gone: BTreeSet<String> = f.removed.iter().filter_map(|l| extract(l)).collect();
    let gained: BTreeSet<String> = f
        .added
        .iter()
        .filter_map(|l| extract(l))
        .filter(|n| !gone.contains(n))
        .collect();
    Some(gained.into_iter().collect())
}

#[cfg(test)]
mod tests;
