//! SQL: the schema a Postgres project defines in migrations or a dump
//! (`CREATE TABLE`, `ALTER TABLE … ADD/DROP/RENAME COLUMN`, `CREATE TYPE …
//! AS ENUM`, `CREATE [OR REPLACE] FUNCTION`, `CREATE POLICY`, views and the
//! `DROP`s), and the tables plain query files read.
//!
//! Known limits: it reads the statements it knows and skips the rest;
//! function bodies (`$$ … $$`) are not looked into; only `CREATE`/`ALTER`
//! of the forms above change the schema, so `EXECUTE`d dynamic SQL is
//! invisible; queries in migrations are not counted as uses (they refer
//! to the schema of their day, which a later migration may have changed).

use std::sync::LazyLock;

use regex::Regex;

use super::{Extractor, Fact, SymKind, Symbol};

/// Reads `*.sql`.
pub struct SqlExtractor;

impl Extractor for SqlExtractor {
    fn id(&self) -> &'static str {
        "sql"
    }

    fn applies(&self, path: &str) -> bool {
        path.to_ascii_lowercase().ends_with(".sql")
    }

    fn extract(&self, path: &str, text: &str) -> Vec<Fact> {
        let stmts = statements(text);
        let mut facts = Vec::new();
        let mut ddl = false;
        for (line, s) in &stmts {
            ddl |= schema_statement(*line, s, &mut facts);
        }
        if !ddl && !in_migrations(path) {
            for (line, s) in &stmts {
                for t in tables_read(s) {
                    facts.push(Fact::using(Symbol::table(&t), *line));
                }
            }
        }
        facts
    }
}

fn in_migrations(path: &str) -> bool {
    path.to_ascii_lowercase()
        .split('/')
        .any(|s| matches!(s, "migrations" | "migration" | "migrate"))
}

/// The canonical form of a (possibly quoted, possibly schema-qualified)
/// name: lower case, no quotes, no `public.`.
pub fn norm(name: &str) -> String {
    let n = name.trim().replace('"', "").to_lowercase();
    n.strip_prefix("public.").unwrap_or(&n).to_string()
}

/// The statements of `text`, comments removed, dollar-quoted bodies
/// emptied and whitespace collapsed, each with the line it starts on.
pub fn statements(text: &str) -> Vec<(u32, String)> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0u32;
    let mut line = 1u32;
    let mut i = 0;
    let push = |cur: &mut String, start: u32, out: &mut Vec<(u32, String)>| {
        let s = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if !s.is_empty() {
            out.push((start, s));
        }
        cur.clear();
    };
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
        }
        if ch == '-' && c.get(i + 1) == Some(&'-') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(c.len());
            cur.push(' ');
            continue;
        }
        if !ch.is_whitespace() && cur.trim().is_empty() {
            start = line;
        }
        match ch {
            '\'' | '"' => {
                cur.push(ch);
                i += 1;
                while i < c.len() {
                    cur.push(c[i]);
                    if c[i] == '\n' {
                        line += 1;
                    }
                    if c[i] == ch {
                        if c.get(i + 1) == Some(&ch) {
                            cur.push(ch);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            '$' => {
                let mut j = i + 1;
                while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
                    j += 1;
                }
                let tag_ok =
                    c.get(j) == Some(&'$') && !c.get(i + 1).is_some_and(|d| d.is_ascii_digit());
                if !tag_ok {
                    cur.push(ch);
                    i += 1;
                    continue;
                }
                let tag: Vec<char> = c[i..=j].to_vec();
                let mut k = j + 1;
                while k + tag.len() <= c.len() && c[k..k + tag.len()] != tag[..] {
                    if c[k] == '\n' {
                        line += 1;
                    }
                    k += 1;
                }
                cur.push_str("$$ $$");
                i = (k + tag.len()).min(c.len());
            }
            ';' => {
                push(&mut cur, start, &mut out);
                i += 1;
            }
            _ => {
                cur.push(ch);
                i += 1;
            }
        }
    }
    push(&mut cur, start, &mut out);
    out
}

const ID: &str = r#"("[^"]+"|[\w$]+(?:\."[^"]+"|\.[\w$]+)?)"#;

fn re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("ID", ID)).expect("a valid pattern")
}

static CREATE_TABLE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^create\s+(?:(?:global\s+|local\s+)?(?:temporary\s+|temp\s+)|unlogged\s+)?table\s+(?:if\s+not\s+exists\s+)?ID\s*(.*)$",
    )
});
static CREATE_VIEW: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^create\s+(?:or\s+replace\s+)?(?:temp(?:orary)?\s+)?(?:materialized\s+)?view\s+(?:if\s+not\s+exists\s+)?ID",
    )
});
static CREATE_ENUM: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^create\s+type\s+ID\s+as\s+enum\s*\((.*)\)\s*$"));
static CREATE_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^create\s+(?:or\s+replace\s+)?(?:function|procedure)\s+ID\s*\((.*)$")
});
static RETURNS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\breturns\s+((?:setof\s+)?(?:table\s*\([^)]*\)|[\w.\[\]]+(?:\s*\([^)]*\))?))")
        .expect("a valid pattern")
});
static CREATE_POLICY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^create\s+policy\s+ID\s+on\s+ID\s*(.*)$"));
static ALTER_TABLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^alter\s+table\s+(?:if\s+exists\s+)?(?:only\s+)?ID\s+(.*)$"));
static ALTER_TYPE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^alter\s+type\s+ID\s+(.*)$"));
static ALTER_VIEW: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^alter\s+(?:materialized\s+)?view\s+(?:if\s+exists\s+)?ID\s+rename\s+to\s+ID\s*$")
});
static DROP_RELATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^drop\s+(?:materialized\s+)?(?:table|view)\s+(?:if\s+exists\s+)?(.*?)(?:\s+(?:cascade|restrict))?\s*$",
    )
    .expect("a valid pattern")
});
static DROP_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^drop\s+(?:function|procedure)\s+(?:if\s+exists\s+)?ID"));
static DROP_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^drop\s+type\s+(?:if\s+exists\s+)?(.*?)(?:\s+(?:cascade|restrict))?\s*$")
        .expect("a valid pattern")
});
static DROP_POLICY: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^drop\s+policy\s+(?:if\s+exists\s+)?ID\s+on\s+ID"));
static ADD_COLUMN: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^add\s+(?:column\s+)?(?:if\s+not\s+exists\s+)?ID\s*(.*)$"));
static DROP_COLUMN: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^drop\s+(?:column\s+)?(?:if\s+exists\s+)?ID"));
static RENAME_TABLE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^rename\s+to\s+ID\s*$"));
static RENAME_COLUMN: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^rename\s+(?:column\s+)?ID\s+to\s+ID\s*$"));
static ALTER_COLUMN: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^alter\s+(?:column\s+)?ID\s+(.*)$"));
static VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'((?:[^']|'')*)'").expect("a valid pattern"));
static ADD_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^add\s+value\s+(?:if\s+not\s+exists\s+)?'((?:[^']|'')*)'")
        .expect("a valid pattern")
});
static RENAME_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^rename\s+value\s+'((?:[^']|'')*)'\s+to\s+'((?:[^']|'')*)'")
        .expect("a valid pattern")
});
static USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(from|join|into|update)\s+(?:only\s+)?("[^"]+"|[a-z_][\w$]*(?:\.[a-z_][\w$]*)?)(\s*\()?"#)
        .expect("a valid pattern")
});

/// The text between the parenthesis opened just before `s` and its match,
/// and what follows it.
fn balanced(s: &str) -> (&str, &str) {
    let mut depth = 1;
    let mut quote: Option<char> = None;
    for (i, ch) in s.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth == 0 {
                    return (&s[..i], &s[i + 1..]);
                }
            }
            _ => {}
        }
    }
    (s, "")
}

/// `s` split at the commas outside parentheses and quotes.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    let mut quote: Option<char> = None;
    for (i, ch) in s.char_indices() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(s[from..i].trim());
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[from..].trim());
    out.retain(|p| !p.is_empty());
    out
}

fn shape(s: &str, max: usize) -> String {
    let s = s.trim().to_lowercase();
    s.chars().take(max).collect()
}

const CONSTRAINTS: [&str; 8] = [
    "constraint",
    "primary",
    "foreign",
    "unique",
    "check",
    "exclude",
    "like",
    "period",
];

fn is_constraint(entry: &str) -> bool {
    let first = entry
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    CONSTRAINTS.contains(&first.as_str())
}

/// Facts of a schema-changing statement; whether it was one.
fn schema_statement(line: u32, s: &str, out: &mut Vec<Fact>) -> bool {
    if let Some(m) = CREATE_TABLE.captures(s) {
        let table = norm(&m[1]);
        out.push(Fact::provide(Symbol::table(&table), line, ""));
        if let Some(body) = m[2].strip_prefix('(') {
            let (body, _) = balanced(body);
            for entry in split_top(body).into_iter().filter(|e| !is_constraint(e)) {
                if let Some((name, rest)) = take_ident(entry) {
                    out.push(Fact::provide(
                        Symbol::column(&table, &norm(name)),
                        line,
                        &shape(rest, 80),
                    ));
                }
            }
        }
        return true;
    }
    if let Some(m) = CREATE_VIEW.captures(s) {
        out.push(Fact::provide(Symbol::table(&norm(&m[1])), line, "view"));
        return true;
    }
    if let Some(m) = CREATE_ENUM.captures(s) {
        let name = norm(&m[1]);
        out.push(Fact::provide(Symbol::new(SymKind::Enum, &name), line, ""));
        for v in VALUE.captures_iter(&m[2]) {
            out.push(Fact::provide(
                Symbol::new(
                    SymKind::EnumValue,
                    &format!("{name}.{}", v[1].replace("''", "'")),
                ),
                line,
                "",
            ));
        }
        return true;
    }
    if let Some(m) = CREATE_FUNCTION.captures(s) {
        let (args, after) = balanced(&m[2]);
        let ret = RETURNS
            .captures(after)
            .map(|r| shape(&r[1], 60))
            .unwrap_or_default();
        let detail = format!("({}) returns {ret}", shape(args, 100)).replace("  ", " ");
        out.push(Fact::provide(
            Symbol::function(&norm(&m[1])),
            line,
            detail.trim(),
        ));
        return true;
    }
    if let Some(m) = CREATE_POLICY.captures(s) {
        let name = m[1].replace('"', "");
        let table = norm(&m[2]);
        out.push(Fact::provide(
            Symbol::new(SymKind::Policy, &format!("{table}.{name}")),
            line,
            &shape(&m[3], 100),
        ));
        return true;
    }
    if let Some(m) = ALTER_TABLE.captures(s) {
        let table = norm(&m[1]);
        for action in split_top(&m[2]) {
            alter_action(line, &table, action, out);
        }
        return true;
    }
    if let Some(m) = ALTER_TYPE.captures(s) {
        let name = norm(&m[1]);
        let rest = m[2].trim();
        if let Some(v) = ADD_VALUE.captures(rest) {
            out.push(Fact::provide(
                Symbol::new(
                    SymKind::EnumValue,
                    &format!("{name}.{}", v[1].replace("''", "'")),
                ),
                line,
                "",
            ));
        } else if let Some(v) = RENAME_VALUE.captures(rest) {
            out.push(Fact::Rename {
                from: Symbol::new(SymKind::EnumValue, &format!("{name}.{}", &v[1])),
                to: Symbol::new(SymKind::EnumValue, &format!("{name}.{}", &v[2])),
            });
        } else if let Some(r) = RENAME_TABLE.captures(rest) {
            out.push(Fact::Rename {
                from: Symbol::new(SymKind::Enum, &name),
                to: Symbol::new(SymKind::Enum, &norm(&r[1])),
            });
        }
        return true;
    }
    if let Some(m) = ALTER_VIEW.captures(s) {
        out.push(Fact::Rename {
            from: Symbol::table(&norm(&m[1])),
            to: Symbol::table(&norm(&m[2])),
        });
        return true;
    }
    if let Some(m) = DROP_RELATION.captures(s) {
        for t in split_top(&m[1]) {
            out.push(Fact::Remove {
                sym: Symbol::table(&norm(t)),
            });
        }
        return true;
    }
    if let Some(m) = DROP_FUNCTION.captures(s) {
        out.push(Fact::Remove {
            sym: Symbol::function(&norm(&m[1])),
        });
        return true;
    }
    if let Some(m) = DROP_TYPE.captures(s) {
        for t in split_top(&m[1]) {
            out.push(Fact::Remove {
                sym: Symbol::new(SymKind::Enum, &norm(t)),
            });
        }
        return true;
    }
    if let Some(m) = DROP_POLICY.captures(s) {
        out.push(Fact::Remove {
            sym: Symbol::new(
                SymKind::Policy,
                &format!("{}.{}", norm(&m[2]), m[1].replace('"', "")),
            ),
        });
        return true;
    }
    false
}

fn alter_action(line: u32, table: &str, action: &str, out: &mut Vec<Fact>) {
    let lower = action.to_ascii_lowercase();
    let word = |w: &str| lower.starts_with(w);
    if word("add constraint")
        || word("add primary")
        || word("add foreign")
        || word("add unique")
        || word("add check")
        || word("add exclude")
    {
        return;
    }
    if let Some(m) = RENAME_TABLE.captures(action) {
        out.push(Fact::Rename {
            from: Symbol::table(table),
            to: Symbol::table(&norm(&m[1])),
        });
    } else if word("rename constraint") {
    } else if let Some(m) = RENAME_COLUMN.captures(action) {
        out.push(Fact::Rename {
            from: Symbol::column(table, &norm(&m[1])),
            to: Symbol::column(table, &norm(&m[2])),
        });
    } else if let Some(m) = ADD_COLUMN.captures(action) {
        out.push(Fact::provide(
            Symbol::column(table, &norm(&m[1])),
            line,
            &shape(&m[2], 80),
        ));
    } else if word("drop constraint") {
    } else if let Some(m) = DROP_COLUMN.captures(action) {
        out.push(Fact::Remove {
            sym: Symbol::column(table, &norm(&m[1])),
        });
    } else if let Some(m) = ALTER_COLUMN.captures(action) {
        out.push(Fact::Alter {
            sym: Symbol::column(table, &norm(&m[1])),
            line,
            change: shape(&m[2], 60),
        });
    }
}

/// The first name of `entry` and what follows it.
fn take_ident(entry: &str) -> Option<(&str, &str)> {
    let entry = entry.trim();
    if let Some(rest) = entry.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((&entry[..end + 2], &rest[end + 1..]));
    }
    let end = entry
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(entry.len());
    (end > 0).then(|| (&entry[..end], &entry[end..]))
}

/// The tables a query reads or writes: the names after `FROM`, `JOIN`,
/// `INTO` and a leading `UPDATE`. `text` is one statement. Function
/// calls (`from now()`) and `FOR UPDATE` are not tables.
pub fn tables_read(text: &str) -> Vec<String> {
    let t = text.trim_start();
    let head = t
        .split(|c: char| !c.is_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        head.as_str(),
        "select" | "insert" | "update" | "delete" | "with" | "merge"
    ) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for m in USE.captures_iter(t) {
        let kw = m[1].to_ascii_lowercase();
        let at = m.get(0).map_or(0, |g| g.start());
        if kw == "update" && at != 0 {
            continue;
        }
        if (kw == "from" || kw == "join") && m.get(3).is_some() {
            continue;
        }
        let rest = &t[m.get(0).map_or(0, |g| g.end())..];
        if kw != "into" && !clause_follows(rest) {
            continue;
        }
        let name = norm(&m[2]);
        if matches!(
            name.as_str(),
            "select" | "lateral" | "unnest" | "only" | "set" | "values"
        ) {
            continue;
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Words that may follow a table name in a query.
const CLAUSES: [&str; 25] = [
    "where",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
    "order",
    "group",
    "limit",
    "offset",
    "having",
    "union",
    "on",
    "using",
    "set",
    "returning",
    "values",
    "for",
    "window",
    "except",
    "intersect",
    "fetch",
    "select",
];

/// Whether `rest` (what follows a table name) reads like SQL and not like
/// prose: the end, a clause, a closing parenthesis or comma, or one alias
/// word and then one of those.
fn clause_follows(rest: &str) -> bool {
    let rest = rest.trim_start();
    let words: Vec<String> = rest
        .split(|c: char| c.is_whitespace())
        .filter(|w| !w.is_empty())
        .take(3)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    let ends = |w: Option<&String>| match w {
        None => true,
        Some(w) => w.starts_with([',', ')', ';']) || CLAUSES.contains(&w.trim_end_matches(';')),
    };
    match words.first().map(String::as_str) {
        None => true,
        Some("as") => true,
        Some(_) if ends(words.first()) => true,
        Some(_) => words.get(1).is_some() && ends(words.get(1)),
    }
}
