//! Cross-repo contracts: what one repository of a workspace provides (a
//! table, a column, an RPC function, an HTTP endpoint, an environment key)
//! and which files of which repositories consume it.
//!
//! Everything here is derived from the sources, deterministically and
//! without a model: pluggable [`Extractor`]s read one file at a time and
//! return [`Fact`]s; an [`Index`] folds the facts of all files into the
//! current set of provided symbols and the places that use them. Asking
//! "who consumes X" or "what did this change break" is then a lookup
//! ([`Report`]).
//!
//! Adding a stack means writing one more [`Extractor`] and listing it in
//! [`EXTRACTORS`]; see `guide/contracts.md`.

pub mod code;
pub mod lex;
pub mod openapi;
pub mod sql;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::board::clean;
use crate::tokens::estimate_tokens;

/// What a pulled contract note may cost, in estimated tokens.
pub const PULL_TOKENS: usize = 300;
/// Consumers named per provider in a note before "+N more".
pub const NOTE_CONSUMERS: usize = 4;
/// Longest shape (columns, signature) shown.
const SHAPE_MAX: usize = 160;

/// What kind of thing a symbol is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymKind {
    Table,
    Column,
    Enum,
    EnumValue,
    Function,
    /// A row-level-security policy: `table.policy name`. Metadata only.
    Policy,
    /// `GET /orders/{}`.
    Endpoint,
    /// An API schema (OpenAPI components).
    Schema,
    /// An environment or config key.
    Env,
    /// A model field read from JSON (`order.total`): a use that is only
    /// matched to a column by name, so it is marked inferred.
    Field,
}

impl SymKind {
    pub const ALL: [SymKind; 10] = [
        SymKind::Table,
        SymKind::Column,
        SymKind::Enum,
        SymKind::EnumValue,
        SymKind::Function,
        SymKind::Policy,
        SymKind::Endpoint,
        SymKind::Schema,
        SymKind::Env,
        SymKind::Field,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SymKind::Table => "table",
            SymKind::Column => "column",
            SymKind::Enum => "enum",
            SymKind::EnumValue => "enum-value",
            SymKind::Function => "function",
            SymKind::Policy => "policy",
            SymKind::Endpoint => "endpoint",
            SymKind::Schema => "schema",
            SymKind::Env => "env",
            SymKind::Field => "field",
        }
    }

    pub fn parse(s: &str) -> Option<SymKind> {
        SymKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// A named thing a repository provides or another one uses.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol {
    pub kind: SymKind,
    /// Canonical: lower case, no `public.`; columns `table.column`,
    /// endpoints `METHOD /path/{}`.
    pub name: String,
}

impl Symbol {
    pub fn new(kind: SymKind, name: &str) -> Symbol {
        Symbol {
            kind,
            name: name.to_string(),
        }
    }

    pub fn table(name: &str) -> Symbol {
        Symbol::new(SymKind::Table, name)
    }

    pub fn column(table: &str, column: &str) -> Symbol {
        Symbol::new(SymKind::Column, &format!("{table}.{column}"))
    }

    pub fn function(name: &str) -> Symbol {
        Symbol::new(SymKind::Function, name)
    }

    /// `table orders`, `column orders.total`.
    pub fn label(&self) -> String {
        format!("{} {}", self.kind.as_str(), self.name)
    }

    /// The symbol this one is a part of: a column's table, a value's enum.
    fn child_of(&self, parent: &Symbol) -> Option<&str> {
        let owns = matches!(
            (parent.kind, self.kind),
            (SymKind::Table, SymKind::Column | SymKind::Policy)
                | (SymKind::Enum, SymKind::EnumValue)
        );
        if !owns {
            return None;
        }
        self.name
            .strip_prefix(parent.name.as_str())
            .and_then(|r| r.strip_prefix('.'))
    }

    /// The table (or model) part of a `table.column` name.
    fn owner(&self) -> Option<&str> {
        self.name.rsplit_once('.').map(|(t, _)| t)
    }

    fn leaf(&self) -> &str {
        self.name.rsplit_once('.').map_or(&self.name, |(_, c)| c)
    }

    /// The method of an endpoint, `ANY` when the use did not say.
    fn method(&self) -> &str {
        self.name.split_once(' ').map_or("ANY", |(m, _)| m)
    }

    fn path(&self) -> &str {
        self.name.split_once(' ').map_or(&self.name, |(_, p)| p)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// What an extractor found in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fact {
    /// The file defines `sym`. `detail` is its shape (a column's type, a
    /// function's signature): when it changes, so has the contract.
    Provide {
        sym: Symbol,
        line: u32,
        detail: String,
    },
    /// The file adds to the shape of an existing symbol.
    Alter {
        sym: Symbol,
        line: u32,
        change: String,
    },
    /// The file removes `sym` (and its parts).
    Remove { sym: Symbol },
    /// The file renames `from` to `to` (and moves its parts).
    Rename { from: Symbol, to: Symbol },
    /// The file uses `sym`.
    Use {
        sym: Symbol,
        line: u32,
        /// Matched by a naming convention, not by an exact call.
        inferred: bool,
    },
}

impl Fact {
    pub fn provide(sym: Symbol, line: u32, detail: &str) -> Fact {
        Fact::Provide {
            sym,
            line,
            detail: detail.to_string(),
        }
    }

    pub fn using(sym: Symbol, line: u32) -> Fact {
        Fact::Use {
            sym,
            line,
            inferred: false,
        }
    }
}

/// Reads one file and says what it provides and uses. Pure: it is given
/// the path and the text, and touches nothing else.
pub trait Extractor: Sync {
    /// A short stable name: `sql`, `openapi`, `code`, `env`.
    fn id(&self) -> &'static str;
    /// Whether it reads the file at `path` (`/`-separated, relative to
    /// its repository).
    fn applies(&self, path: &str) -> bool;
    fn extract(&self, path: &str, text: &str) -> Vec<Fact>;
}

/// The extractors swarmux ships.
pub const EXTRACTORS: &[&dyn Extractor] = &[
    &sql::SqlExtractor,
    &openapi::OpenApiExtractor,
    &code::CodeExtractor,
    &code::EnvFileExtractor,
];

/// Whether any extractor reads `path`.
pub fn wanted(path: &str) -> bool {
    EXTRACTORS.iter().any(|e| e.applies(path))
}

/// What every extractor that reads `path` finds in `text`.
pub fn extract(path: &str, text: &str) -> Vec<Fact> {
    EXTRACTORS
        .iter()
        .filter(|e| e.applies(path))
        .flat_map(|e| e.extract(path, text))
        .collect()
}

/// A symbol as the sources define it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provided {
    pub sym: Symbol,
    pub repo: String,
    /// Relative to the repository, `/`.
    pub file: String,
    pub line: u32,
    /// The content hash of the file: the contract's anchor.
    pub hash: String,
    /// Its shape: a column's type, a function's signature, an endpoint's
    /// operation.
    pub detail: String,
}

impl Provided {
    /// `backend/supabase/migrations/001.sql:3`.
    pub fn place(&self) -> String {
        format!("{}/{}:{}", self.repo, self.file, self.line)
    }
}

/// A place that uses a symbol.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    pub repo: String,
    pub file: String,
    pub line: u32,
    /// The content hash of the file.
    pub hash: String,
    pub sym: Symbol,
    pub inferred: bool,
}

impl Site {
    /// `app/lib/orders.dart:42`.
    pub fn place(&self) -> String {
        format!("{}/{}:{}", self.repo, self.file, self.line)
    }
}

/// One file's facts, for [`Index::build`].
#[derive(Clone, Copy, Debug)]
pub struct FileFacts<'a> {
    pub repo: &'a str,
    pub path: &'a str,
    pub hash: &'a str,
    pub facts: &'a [Fact],
}

/// Everything the workspace provides and uses.
#[derive(Clone, Debug, Default)]
pub struct Index {
    provided: BTreeMap<(String, Symbol), Provided>,
    sites: Vec<Site>,
}

fn names_table(table: &str, model: &str) -> bool {
    let table = table.rsplit_once('.').map_or(table, |(_, t)| t);
    let plural_y = model
        .strip_suffix('y')
        .map(|m| format!("{m}ies"))
        .unwrap_or_default();
    table == model
        || table == format!("{model}s")
        || table == format!("{model}es")
        || (!plural_y.is_empty() && table == plural_y)
}

/// Whether a use at `site` depends on `p` directly.
fn consumes(site: &Symbol, p: &Symbol) -> bool {
    match (site.kind, p.kind) {
        (SymKind::Table, SymKind::Table)
        | (SymKind::Column, SymKind::Column)
        | (SymKind::Function, SymKind::Function)
        | (SymKind::Env, SymKind::Env)
        | (SymKind::Schema, SymKind::Schema)
        | (SymKind::Enum, SymKind::Enum) => site.name == p.name,
        (SymKind::Endpoint, SymKind::Endpoint) => {
            site.path() == p.path() && (site.method() == "ANY" || site.method() == p.method())
        }
        (SymKind::Field, SymKind::Column) => {
            site.leaf() == p.leaf()
                && site
                    .owner()
                    .zip(p.owner())
                    .is_some_and(|(model, table)| names_table(table, model))
        }
        _ => false,
    }
}

/// Whether a use at `site` depends on the table `p` by using a part of it.
fn consumes_part(site: &Symbol, p: &Symbol) -> bool {
    if p.kind != SymKind::Table {
        return false;
    }
    match site.kind {
        SymKind::Column => site.owner() == Some(p.name.as_str()),
        SymKind::Field => site.owner().is_some_and(|m| names_table(&p.name, m)),
        _ => false,
    }
}

impl Index {
    /// Folds the files' facts. Within a repository files are applied in
    /// path order, so migrations (`001_…`, `20240101…_…`) build up the
    /// schema the way a database would: later `ALTER` and `DROP`
    /// statements change what earlier files created.
    pub fn build(files: &[FileFacts<'_>]) -> Index {
        let mut order: Vec<&FileFacts<'_>> = files.iter().collect();
        order.sort_by(|a, b| (a.repo, a.path).cmp(&(b.repo, b.path)));
        let mut index = Index::default();
        for f in order {
            for fact in f.facts {
                index.apply(f, fact);
            }
        }
        index.sites.sort();
        index.sites.dedup();
        index
    }

    fn apply(&mut self, f: &FileFacts<'_>, fact: &Fact) {
        let repo = f.repo.to_string();
        match fact {
            Fact::Provide { sym, line, detail } => {
                self.provided.insert(
                    (repo.clone(), sym.clone()),
                    Provided {
                        sym: sym.clone(),
                        repo,
                        file: f.path.to_string(),
                        line: *line,
                        hash: f.hash.to_string(),
                        detail: detail.clone(),
                    },
                );
            }
            Fact::Alter { sym, line, change } => {
                if let Some(p) = self.provided.get_mut(&(repo, sym.clone())) {
                    p.detail = cap(&format!("{}; {change}", p.detail), SHAPE_MAX);
                    p.file = f.path.to_string();
                    p.line = *line;
                    p.hash = f.hash.to_string();
                }
            }
            Fact::Remove { sym } => {
                self.provided
                    .retain(|(r, s), _| !(*r == repo && (s == sym || s.child_of(sym).is_some())));
            }
            Fact::Rename { from, to } => {
                let moved: Vec<(String, Symbol)> = self
                    .provided
                    .keys()
                    .filter(|(r, s)| *r == repo && (s == from || s.child_of(from).is_some()))
                    .cloned()
                    .collect();
                for key in moved {
                    let Some(mut p) = self.provided.remove(&key) else {
                        continue;
                    };
                    p.sym.name = match p.sym.child_of(from) {
                        Some(rest) => format!("{}.{rest}", to.name),
                        None => to.name.clone(),
                    };
                    p.file = f.path.to_string();
                    p.hash = f.hash.to_string();
                    self.provided.insert((repo.clone(), p.sym.clone()), p);
                }
            }
            Fact::Use {
                sym,
                line,
                inferred,
            } => self.sites.push(Site {
                repo,
                file: f.path.to_string(),
                line: *line,
                hash: f.hash.to_string(),
                sym: sym.clone(),
                inferred: *inferred,
            }),
        }
    }

    /// Every provided symbol, by repository and symbol.
    pub fn provided(&self) -> impl Iterator<Item = &Provided> {
        self.provided.values()
    }

    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    pub fn is_empty(&self) -> bool {
        self.provided.is_empty() && self.sites.is_empty()
    }

    /// Whether some repository provides `sym`.
    pub fn provides(&self, sym: &Symbol) -> bool {
        self.provided.keys().any(|(_, s)| s == sym)
    }

    /// The provided symbols `query` names: `orders`, `table orders`,
    /// `orders.total`, `get_orders`, `GET /orders`. Case does not matter.
    pub fn find(&self, query: &str) -> Vec<&Provided> {
        let q = query.trim().to_lowercase();
        let (kind, name) = match q.split_once(' ') {
            Some((k, rest)) if SymKind::parse(k).is_some() => (SymKind::parse(k), rest.trim()),
            _ => (None, q.as_str()),
        };
        let name = name.strip_prefix("public.").unwrap_or(name);
        self.provided
            .values()
            .filter(|p| kind.is_none_or(|k| p.sym.kind == k))
            .filter(|p| p.sym.name.to_lowercase() == name)
            .collect()
    }

    /// The places that depend on `p`, by place: a use of the symbol itself
    /// or, for a table, of one of its columns. Its own file is left out.
    pub fn consumers_of(&self, p: &Provided) -> Vec<&Site> {
        let mut out: Vec<&Site> = self
            .sites
            .iter()
            .filter(|s| !(s.repo == p.repo && s.file == p.file))
            .filter(|s| consumes(&s.sym, &p.sym) || consumes_part(&s.sym, &p.sym))
            .collect();
        out.dedup_by(|a, b| (&a.repo, &a.file, a.line) == (&b.repo, &b.file, b.line));
        out
    }

    /// The provided symbols a use at `site` depends on.
    pub fn providers_of(&self, site: &Site) -> Vec<&Provided> {
        self.provided
            .values()
            .filter(|p| consumes(&site.sym, &p.sym))
            .collect()
    }

    /// Provided symbols defined in `file` of `repo`.
    pub fn provided_in(&self, repo: &str, file: &str) -> Vec<&Provided> {
        self.provided
            .values()
            .filter(|p| p.repo == repo && p.file == file)
            .collect()
    }

    /// Uses found in `file` of `repo`.
    pub fn sites_in(&self, repo: &str, file: &str) -> Vec<&Site> {
        self.sites
            .iter()
            .filter(|s| s.repo == repo && s.file == file)
            .collect()
    }

    /// What `p` looks like now, in a line: a table's columns, a
    /// function's signature.
    pub fn shape(&self, p: &Provided) -> String {
        let text = match p.sym.kind {
            SymKind::Table => {
                let cols: Vec<String> = self
                    .provided
                    .values()
                    .filter(|c| c.repo == p.repo && c.sym.kind == SymKind::Column)
                    .filter(|c| c.sym.child_of(&p.sym).is_some())
                    .map(|c| {
                        let ty = c.detail.split_whitespace().next().unwrap_or("");
                        format!("{} {ty}", c.sym.leaf()).trim().to_string()
                    })
                    .collect();
                if cols.is_empty() {
                    p.detail.clone()
                } else {
                    format!("columns {}", cols.join(", "))
                }
            }
            SymKind::Enum => {
                let vals: Vec<&str> = self
                    .provided
                    .values()
                    .filter(|v| v.repo == p.repo && v.sym.child_of(&p.sym).is_some())
                    .map(|v| v.sym.leaf())
                    .collect();
                format!("values {}", vals.join(", "))
            }
            SymKind::Function => p.detail.clone(),
            _ => p.detail.clone(),
        };
        cap(&text, SHAPE_MAX)
    }
}

fn cap(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// How a provided symbol differs from before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delta {
    /// No repository provides it any more (removed or renamed).
    Gone,
    /// Its shape changed: a signature, a column's type.
    Changed(String),
}

/// A symbol that was provided before and is not as it was, and who uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Impact {
    pub before: Provided,
    pub delta: Delta,
    /// Places that still use it, as of the newer index.
    pub consumers: Vec<Site>,
}

/// What a change did to the contracts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub impacts: Vec<Impact>,
}

impl Report {
    /// Compares the provided symbols of `before` with those of `after`;
    /// consumers are looked for in `after`, so a consumer that was
    /// updated is not counted.
    pub fn of(before: &Index, after: &Index) -> Report {
        let mut impacts = Vec::new();
        for p in before.provided() {
            if matches!(p.sym.kind, SymKind::Policy) {
                continue;
            }
            let now = after.provided.get(&(p.repo.clone(), p.sym.clone()));
            let delta = match now {
                None if after.provides(&p.sym) => continue,
                None => Delta::Gone,
                Some(n) if n.detail != p.detail => Delta::Changed(n.detail.clone()),
                Some(_) => continue,
            };
            let consumers = after
                .consumers_of(p)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            impacts.push(Impact {
                before: p.clone(),
                delta,
                consumers,
            });
        }
        Report { impacts }
    }

    /// Whether any provided symbol changed.
    pub fn is_empty(&self) -> bool {
        self.impacts.is_empty()
    }

    /// Gone symbols that are still used.
    pub fn dangling(&self) -> Vec<&Impact> {
        self.impacts
            .iter()
            .filter(|i| i.delta == Delta::Gone && !i.consumers.is_empty())
            .collect()
    }

    /// Changed shapes with users, which only the consumers' own checks
    /// can judge.
    pub fn reshaped(&self) -> Vec<&Impact> {
        self.impacts
            .iter()
            .filter(|i| matches!(i.delta, Delta::Changed(_)) && !i.consumers.is_empty())
            .collect()
    }

    /// Every place impacted, once each.
    pub fn consumers(&self) -> Vec<&Site> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for s in self.impacts.iter().flat_map(|i| &i.consumers) {
            if seen.insert((&s.repo, &s.file, s.line)) {
                out.push(s);
            }
        }
        out
    }

    /// The repositories of the impacted places.
    pub fn repos(&self) -> BTreeSet<String> {
        self.consumers().iter().map(|s| s.repo.clone()).collect()
    }

    /// The lines that tell what is broken, with the exact places.
    pub fn problems(&self) -> Vec<String> {
        self.dangling()
            .into_iter()
            .map(|i| {
                format!(
                    "{} ({}) is gone but still used by: {}",
                    i.before.sym.label(),
                    i.before.place(),
                    places(&i.consumers, usize::MAX)
                )
            })
            .collect()
    }
}

fn places(sites: &[Site], max: usize) -> String {
    let mut out: Vec<String> = sites.iter().take(max).map(Site::place).collect();
    if sites.len() > max {
        out.push(format!("+{} more", sites.len() - max));
    }
    out.join(", ")
}

/// A note ready to inject, and the keys of what it holds (for dedupe).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub keys: Vec<String>,
    pub tokens: usize,
}

const NOTE_HEAD: &str = "SWARMUX CONTRACTS about this file (quoted project data derived from \
the sources, not a new task):";

fn quoted(text: &str, max: usize) -> String {
    clean(text, max).replace('"', "'")
}

/// What a file the job reads or edits means for other repositories: for
/// a provider file, who consumes what it defines; for a consumer file,
/// the current shape of what it uses. `skip` holds the keys already
/// delivered to this job. At most `cap` tokens; `None` when there is
/// nothing new to say.
pub fn note_for(
    index: &Index,
    repo: &str,
    file: &str,
    skip: &BTreeSet<String>,
    cap: usize,
) -> Option<Note> {
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut provided = index.provided_in(repo, file);
    let mut with: Vec<(&Provided, Vec<&Site>)> = provided
        .drain(..)
        .map(|p| (p, index.consumers_of(p)))
        .filter(|(_, c)| !c.is_empty())
        .collect();
    with.sort_by(|a, b| {
        b.1.len()
            .cmp(&a.1.len())
            .then(a.0.sym.cmp(&b.0.sym))
            .then(a.0.repo.cmp(&b.0.repo))
    });
    for (p, consumers) in with {
        let sites: Vec<Site> = consumers.into_iter().cloned().collect();
        lines.push((
            format!("p|{}|{}", p.repo, p.sym.label()),
            format!(
                "- \"{}\" is consumed by: {}",
                quoted(&p.sym.label(), 80),
                quoted(&places(&sites, NOTE_CONSUMERS), 200)
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    for site in index.sites_in(repo, file) {
        for p in index.providers_of(site) {
            if !seen.insert((p.repo.clone(), p.sym.clone())) {
                continue;
            }
            lines.push((
                format!("c|{}|{}", p.repo, p.sym.label()),
                format!(
                    "- this file uses \"{}\" provided by \"{}\": \"{}\"",
                    quoted(&p.sym.label(), 80),
                    quoted(&p.place(), 120),
                    quoted(&index.shape(p), SHAPE_MAX)
                ),
            ));
        }
    }
    lines.retain(|(k, _)| !skip.contains(k));
    if lines.is_empty() {
        return None;
    }
    let head = estimate_tokens(NOTE_HEAD) + 1;
    let mut text = NOTE_HEAD.to_string();
    let mut keys = Vec::new();
    let mut used = head;
    for (key, line) in lines {
        let cost = estimate_tokens(&line) + 1;
        if used + cost > cap {
            break;
        }
        used += cost;
        text.push('\n');
        text.push_str(&line);
        keys.push(key);
    }
    if keys.is_empty() {
        return None;
    }
    Some(Note {
        tokens: estimate_tokens(&text),
        text,
        keys,
    })
}

/// One file's facts as text, one fact a line, for storing.
pub fn encode(facts: &[Fact]) -> String {
    fn s(t: &str) -> String {
        t.replace(['\t', '\n', '\r'], " ")
    }
    let mut out = String::new();
    for f in facts {
        let line = match f {
            Fact::Provide { sym, line, detail } => format!(
                "P\t{}\t{}\t{line}\t{}",
                sym.kind.as_str(),
                s(&sym.name),
                s(detail)
            ),
            Fact::Alter { sym, line, change } => format!(
                "A\t{}\t{}\t{line}\t{}",
                sym.kind.as_str(),
                s(&sym.name),
                s(change)
            ),
            Fact::Remove { sym } => format!("R\t{}\t{}", sym.kind.as_str(), s(&sym.name)),
            Fact::Rename { from, to } => format!(
                "N\t{}\t{}\t{}\t{}",
                from.kind.as_str(),
                s(&from.name),
                to.kind.as_str(),
                s(&to.name)
            ),
            Fact::Use {
                sym,
                line,
                inferred,
            } => format!(
                "U\t{}\t{}\t{line}\t{}",
                sym.kind.as_str(),
                s(&sym.name),
                u8::from(*inferred)
            ),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The facts [`encode`] wrote; lines it cannot read are skipped.
pub fn decode(text: &str) -> Vec<Fact> {
    let sym = |k: &str, n: &str| SymKind::parse(k).map(|k| Symbol::new(k, n));
    let mut out = Vec::new();
    for l in text.lines() {
        let p: Vec<&str> = l.split('\t').collect();
        let fact = match p.as_slice() {
            ["P", k, n, line, d] => sym(k, n)
                .zip(line.parse().ok())
                .map(|(sym, line)| Fact::provide(sym, line, d)),
            ["A", k, n, line, c] => {
                sym(k, n)
                    .zip(line.parse().ok())
                    .map(|(sym, line)| Fact::Alter {
                        sym,
                        line,
                        change: (*c).to_string(),
                    })
            }
            ["R", k, n] => sym(k, n).map(|sym| Fact::Remove { sym }),
            ["N", k1, n1, k2, n2] => sym(k1, n1)
                .zip(sym(k2, n2))
                .map(|(from, to)| Fact::Rename { from, to }),
            ["U", k, n, line, inf] => {
                sym(k, n)
                    .zip(line.parse().ok())
                    .map(|(sym, line)| Fact::Use {
                        sym,
                        line,
                        inferred: *inf == "1",
                    })
            }
            _ => None,
        };
        out.extend(fact);
    }
    out
}

#[cfg(test)]
mod tests;
