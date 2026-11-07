//! Consumers found by reading application code: Dart/Flutter,
//! TypeScript/JavaScript and Python. A token scan, not a parser, and
//! honest about it: it finds the calls apps make to reach another
//! repository and ignores comments and strings that merely contain such a
//! call.
//!
//! Found: `.from('t')`, `.table('t')` and `.from_('t')` with a following
//! `.select('a, b, rel(c)')`; `.rpc('f')`; `fetch`/`axios`/`$fetch`/`dio`/
//! `requests`/`Uri.parse` calls with a URL (`/api/orders`, base URLs and
//! `${…}` parts dropped); `process.env.X`, `import.meta.env.X`,
//! `Platform.environment['X']`, `String.fromEnvironment('X')`,
//! `os.environ['X']`, `os.getenv('X')`; SQL inside strings; Supabase's
//! generated TypeScript types (`Tables`, `Functions`, `Enums`); and
//! `json['col']` / `@JsonKey(name: 'col')` in Dart model classes, matched
//! to a table by the class name (marked inferred).
//!
//! Known limits: a table name held in a variable or built at run time is
//! invisible; so is a URL in a constant used elsewhere; an ORM's model
//! mapping is not read; server route definitions (`app.get('/x')`) are
//! deliberately not counted as consumers.

use std::sync::LazyLock;

use regex::Regex;

use super::lex::{Syntax, Tok, Token, lex};
use super::openapi::canonical_path;
use super::sql::{norm, tables_read};
use super::{Extractor, Fact, SymKind, Symbol};

/// Reads `.dart`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` and `.py`.
pub struct CodeExtractor;

fn ext(path: &str) -> String {
    path.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

impl Extractor for CodeExtractor {
    fn id(&self) -> &'static str {
        "code"
    }

    fn applies(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        matches!(
            ext(path).as_str(),
            "dart" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py"
        ) && !lower.ends_with(".min.js")
    }

    fn extract(&self, path: &str, text: &str) -> Vec<Fact> {
        let e = ext(path);
        let syn = match e.as_str() {
            "dart" => Syntax::DART,
            "py" => Syntax::PYTHON,
            _ => Syntax::JS,
        };
        scan(&lex(text, &syn), e == "dart")
    }
}

/// Reads `.env.example` and its siblings: the keys a service expects.
/// Values are never read (a real `.env` is never opened).
pub struct EnvFileExtractor;

impl Extractor for EnvFileExtractor {
    fn id(&self) -> &'static str {
        "env"
    }

    fn applies(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".env.example" | ".env.sample" | ".env.template" | ".env.dist" | "example.env"
        )
    }

    fn extract(&self, _path: &str, text: &str) -> Vec<Fact> {
        text.lines()
            .enumerate()
            .filter_map(|(n, l)| {
                let l = l.trim().strip_prefix("export ").unwrap_or(l.trim());
                let (key, _) = l.split_once('=')?;
                let key = key.trim();
                is_env_key(key)
                    .then(|| Fact::provide(Symbol::new(SymKind::Env, key), n as u32 + 1, ""))
            })
            .collect()
    }
}

fn is_env_key(k: &str) -> bool {
    k.len() >= 2
        && k.starts_with(|c: char| c.is_ascii_uppercase())
        && k.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z_][A-Za-z0-9_$]*(\.[A-Za-z_][A-Za-z0-9_$]*)?$").expect("valid")
});

const VERBS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];
/// Receivers whose `.get('/x')` defines a route instead of calling one.
const SERVERS: [&str; 8] = [
    "app",
    "router",
    "server",
    "route",
    "routes",
    "fastify",
    "api_router",
    "blueprint",
];
/// Receivers whose `.from('x')` is not a table.
const NOT_TABLES: [&str; 9] = [
    "Array",
    "Buffer",
    "Observable",
    "Promise",
    "Object",
    "Uint8Array",
    "String",
    "List",
    "Map",
];

struct Cursor<'a>(&'a [Token]);

impl<'a> Cursor<'a> {
    fn ident(&self, i: usize) -> Option<&'a str> {
        match self.0.get(i).map(|t| &t.tok) {
            Some(Tok::Ident(s)) => Some(s),
            _ => None,
        }
    }

    fn is(&self, i: usize, c: char) -> bool {
        matches!(self.0.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c)
    }

    fn string(&self, i: usize) -> Option<(&'a str, bool)> {
        match self.0.get(i).map(|t| &t.tok) {
            Some(Tok::Str { text, interp }) => Some((text, *interp)),
            _ => None,
        }
    }

    /// Where the string argument at `i` is: Python's `f"…"`, `r"…"` put a
    /// short prefix before it.
    fn arg(&self, i: usize) -> usize {
        match self.ident(i) {
            Some(p) if p.len() <= 2 && self.string(i + 1).is_some() => i + 1,
            _ => i,
        }
    }

    fn line(&self, i: usize) -> u32 {
        self.0.get(i).map_or(1, |t| t.line)
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The endpoint path of a URL string: scheme and host, query and fragment
/// dropped, a leading `{}` (a base URL variable) dropped, parameters `{}`.
/// `None` when it is not a path.
pub fn url_path(url: &str) -> Option<String> {
    let mut rest = url.trim();
    if let Some(after) = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))
    {
        rest = after.find('/').map_or("/", |at| &after[at..]);
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let rest = match rest.strip_prefix('{').and_then(|r| r.find('}')) {
        Some(end) => &rest[end + 2..],
        None => rest,
    };
    if !rest.starts_with('/') || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(canonical_path(rest))
}

fn endpoint_use(method: &str, path: &str) -> Symbol {
    Symbol::new(
        SymKind::Endpoint,
        &format!("{} {path}", method.to_ascii_uppercase()),
    )
}

/// `col`, `alias:col`, `col::type`, `rel(a, b)`, `alias:rel!inner(a)`.
fn select_uses(table: &str, list: &str, line: u32, out: &mut Vec<Fact>) {
    let mut depth = 0;
    let mut from = 0;
    let mut parts = Vec::new();
    for (i, c) in list.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&list[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&list[from..]);
    for part in parts {
        let part = part.trim();
        if part.is_empty() || part == "*" {
            continue;
        }
        if let Some((head, inner)) = part.split_once('(') {
            let inner = inner.strip_suffix(')').unwrap_or(inner);
            let head = head.rsplit(':').next().unwrap_or(head);
            let head = head.split('!').next().unwrap_or(head).trim();
            if NAME.is_match(head) {
                let rel = norm(head);
                out.push(Fact::using(Symbol::table(&rel), line));
                select_uses(&rel, inner, line, out);
            }
            continue;
        }
        let uncast = part.split("::").next().unwrap_or(part);
        let col = uncast.rsplit(':').next().unwrap_or(uncast);
        let col = col.split(['-', '!']).next().unwrap_or(col).trim();
        if NAME.is_match(col) && !col.contains('.') {
            out.push(Fact::using(Symbol::column(table, &norm(col)), line));
        }
    }
}

fn upper_key(s: &str) -> bool {
    is_env_key(s)
}

fn scan(tokens: &[Token], dart: bool) -> Vec<Fact> {
    let c = Cursor(tokens);
    let mut out = Vec::new();
    let mut owner: Option<String> = None;
    let mut fields: Vec<Fact> = Vec::new();
    let mut from_json = false;
    let mut i = 0;
    while i < tokens.len() {
        // `.name('…')` calls.
        if c.is(i, '.')
            && let Some(name) = c.ident(i + 1)
            && c.is(i + 2, '(')
        {
            let recv = i.checked_sub(1).and_then(|p| c.ident(p));
            let at = c.arg(i + 3);
            if let Some((arg, interp)) = c.string(at) {
                let line = c.line(at);
                match name {
                    "from" | "table" | "from_"
                        if !interp
                            && NAME.is_match(arg)
                            && !recv.is_some_and(|r| NOT_TABLES.contains(&r)) =>
                    {
                        let table = norm(arg);
                        out.push(Fact::using(Symbol::table(&table), line));
                        chain_select(&c, at + 1, &table, &mut out);
                    }
                    "rpc" if !interp && NAME.is_match(arg) && !arg.contains('.') => {
                        out.push(Fact::using(Symbol::function(&norm(arg)), line));
                    }
                    "get" if recv.is_some_and(|r| matches!(r, "env" | "environ")) => {
                        if upper_key(arg) {
                            out.push(Fact::using(Symbol::new(SymKind::Env, arg), line));
                        }
                    }
                    "parse" if recv == Some("Uri") => {
                        if let Some(path) = url_path(arg) {
                            let method = dart_verb(&c, i);
                            out.push(Fact::using(endpoint_use(method, &path), line));
                        }
                    }
                    "http" | "https" if recv == Some("Uri") => {
                        // Uri.https('host', '/path')
                        if c.is(at + 1, ',')
                            && let Some((p, _)) = c.string(at + 2)
                            && let Some(path) = url_path(p)
                        {
                            let method = dart_verb(&c, i);
                            out.push(Fact::using(endpoint_use(method, &path), line));
                        }
                    }
                    v if VERBS.contains(&v)
                        && !recv.is_some_and(|r| SERVERS.contains(&r.to_lowercase().as_str())) =>
                    {
                        if let Some(path) = url_path(arg) {
                            out.push(Fact::using(endpoint_use(v, &path), line));
                        }
                    }
                    _ => {}
                }
            }
        }
        // Calls without a receiver: fetch('/x'), getenv('X'), fromEnvironment('X').
        if let Some(name) = c.ident(i)
            && c.is(i + 1, '(')
            && let Some((arg, _)) = c.string(c.arg(i + 2))
            && (!(i > 0 && c.is(i - 1, '.')) || matches!(name, "fromEnvironment" | "getenv"))
        {
            let line = c.line(c.arg(i + 2));
            match name {
                "fetch" | "$fetch" | "ofetch" | "axios" | "ky" | "got" => {
                    if let Some(path) = url_path(arg) {
                        let method =
                            option_method(&c, c.arg(i + 2) + 1).unwrap_or_else(|| "ANY".into());
                        out.push(Fact::using(endpoint_use(&method, &path), line));
                    }
                }
                "getenv" | "fromEnvironment" if upper_key(arg) => {
                    out.push(Fact::using(Symbol::new(SymKind::Env, arg), line));
                }
                _ => {}
            }
        }
        // process.env.X / import.meta.env.X / env['X'].
        if c.ident(i) == Some("env")
            && i >= 2
            && c.is(i - 1, '.')
            && matches!(c.ident(i - 2), Some("process" | "meta"))
        {
            if c.is(i + 1, '.')
                && let Some(key) = c.ident(i + 2)
                && upper_key(key)
            {
                out.push(Fact::using(Symbol::new(SymKind::Env, key), c.line(i + 2)));
            } else if c.is(i + 1, '[')
                && let Some((key, _)) = c.string(i + 2)
                && upper_key(key)
            {
                out.push(Fact::using(Symbol::new(SymKind::Env, key), c.line(i + 2)));
            }
        }
        // os.environ['X'], Platform.environment['X'].
        if matches!(c.ident(i), Some("environ" | "environment"))
            && c.is(i + 1, '[')
            && let Some((key, _)) = c.string(i + 2)
            && upper_key(key)
        {
            out.push(Fact::using(Symbol::new(SymKind::Env, key), c.line(i + 2)));
        }
        // SQL in a string.
        if let Some((text, _)) = c.string(i)
            && text.len() >= 12
        {
            for t in tables_read(text) {
                out.push(Fact::using(Symbol::table(&t), c.line(i)));
            }
        }
        // Supabase's generated types: Tables / Functions / Enums.
        if let Some(section @ ("Tables" | "Functions" | "Enums")) = c.ident(i)
            && c.is(i + 1, ':')
            && c.is(i + 2, '{')
        {
            generated_section(&c, i + 3, section, &mut out);
        }
        // Supabase's generated type lookups: ['Tables']['orders'].
        if c.is(i, '[')
            && let Some((section @ ("Tables" | "Functions" | "Enums"), _)) = c.string(i + 1)
            && c.is(i + 2, ']')
            && c.is(i + 3, '[')
            && let Some((name, _)) = c.string(i + 4)
            && NAME.is_match(name)
        {
            out.push(Fact::using(generated_symbol(section, name), c.line(i + 4)));
        }
        // Dart models.
        if dart {
            if c.ident(i) == Some("class")
                && let Some(n) = c.ident(i + 1)
            {
                owner = Some(snake(n));
            }
            if let Some(n) = c.ident(i)
                && let Some(mid) = n.strip_prefix("_$")
                && let Some(model) = mid
                    .strip_suffix("FromJson")
                    .or_else(|| mid.strip_suffix("ToJson"))
            {
                owner = Some(snake(model));
                from_json = true;
            }
            if c.ident(i) == Some("fromJson") {
                from_json = true;
            }
            if let Some(o) = &owner {
                let key = if c.ident(i) == Some("json") && c.is(i + 1, '[') {
                    c.string(i + 2).map(|(k, _)| k)
                } else if c.ident(i) == Some("JsonKey")
                    && c.is(i + 1, '(')
                    && c.ident(i + 2) == Some("name")
                    && c.is(i + 3, ':')
                {
                    c.string(i + 4).map(|(k, _)| k)
                } else {
                    None
                };
                if let Some(k) = key.filter(|k| NAME.is_match(k) && !k.contains('.')) {
                    fields.push(Fact::Use {
                        sym: Symbol::new(SymKind::Field, &format!("{o}.{}", norm(k))),
                        line: c.line(i),
                        inferred: true,
                    });
                }
            }
        }
        i += 1;
    }
    if from_json {
        out.extend(fields);
    }
    out
}

fn generated_symbol(section: &str, name: &str) -> Symbol {
    match section {
        "Tables" => Symbol::table(&norm(name)),
        "Functions" => Symbol::function(&norm(name)),
        _ => Symbol::new(SymKind::Enum, &norm(name)),
    }
}

/// The keys of `Tables: { orders: { Row: { id: …, total: … }, … } }`:
/// each key of the section is used, and the columns of each `Row`.
fn generated_section(c: &Cursor<'_>, mut i: usize, section: &str, out: &mut Vec<Fact>) {
    let mut depth = 1;
    let mut table: Option<String> = None;
    let mut in_row_at: Option<i32> = None;
    while i < c.0.len() && depth > 0 {
        let key = match c.0.get(i).map(|t| &t.tok) {
            Some(Tok::Ident(s)) => Some(s.as_str()),
            Some(Tok::Str { text, .. }) => Some(text.as_str()),
            _ => None,
        };
        if c.is(i, '{') {
            depth += 1;
        } else if c.is(i, '}') {
            depth -= 1;
            if in_row_at == Some(depth + 1) {
                in_row_at = None;
            }
        } else if let Some(k) = key
            && c.is(i + 1, ':')
            && NAME.is_match(k)
        {
            if depth == 1 && c.is(i + 2, '{') {
                if section == "Tables" {
                    table = Some(norm(k));
                }
                out.push(Fact::using(generated_symbol(section, k), c.line(i)));
            } else if depth == 1 {
                out.push(Fact::using(generated_symbol(section, k), c.line(i)));
            } else if section == "Tables" && depth == 2 && k == "Row" && c.is(i + 2, '{') {
                in_row_at = Some(depth + 1);
            } else if let (Some(t), Some(at)) = (&table, in_row_at)
                && depth == at
                && !k.contains('.')
            {
                out.push(Fact::using(Symbol::column(t, &norm(k)), c.line(i)));
            }
        }
        i += 1;
    }
}

/// Looks along `.select('…')` in the chain that follows `.from('t')`.
fn chain_select(c: &Cursor<'_>, mut i: usize, table: &str, out: &mut Vec<Fact>) {
    // `i` is just after the table string: expect `)`.
    if !c.is(i, ')') {
        return;
    }
    i += 1;
    for _ in 0..8 {
        if !(c.is(i, '.') && c.ident(i + 1).is_some() && c.is(i + 2, '(')) {
            return;
        }
        if c.ident(i + 1) == Some("select")
            && let Some((list, _)) = c.string(i + 3)
        {
            select_uses(table, list, c.line(i + 3), out);
        }
        // Skip this call's arguments.
        let mut depth = 0;
        let mut j = i + 2;
        while j < c.0.len() {
            if c.is(j, '(') {
                depth += 1;
            } else if c.is(j, ')') {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        i = j + 1;
    }
}

/// The verb of `http.get(Uri.parse(…))`: the call that encloses `Uri`.
fn dart_verb<'a>(c: &Cursor<'a>, dot: usize) -> &'a str {
    // tokens: verb ( Uri . parse (
    if dot >= 4
        && c.is(dot - 2, '(')
        && c.is(dot - 4, '.')
        && let Some(v) = c.ident(dot - 3)
        && VERBS.contains(&v)
    {
        return v;
    }
    "ANY"
}

/// `{ method: 'POST' }` within the arguments that follow the URL.
fn option_method(c: &Cursor<'_>, mut i: usize) -> Option<String> {
    let mut depth = 1;
    let end = (i + 60).min(c.0.len());
    while i < end {
        if c.is(i, '(') {
            depth += 1;
        } else if c.is(i, ')') {
            depth -= 1;
            if depth == 0 {
                return None;
            }
        } else if c.ident(i) == Some("method")
            && c.is(i + 1, ':')
            && let Some((m, _)) = c.string(i + 2)
        {
            return Some(m.to_ascii_uppercase());
        }
        i += 1;
    }
    None
}
