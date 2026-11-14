//! Layered configuration: reads every tier, checks every line, merges by
//! each key's merge kind and keeps where every value came from.
//!
//! Tiers, lowest first: core defaults, the organisation's file (for keys it
//! did not lock), the user's `config.toml`, a workspace's file, the
//! repository's `.swarmux/config.toml`, packs, the mission, and the run
//! (command-line flags and `SWARMUX_<SECTION>__<KEY>`). Keys the managed
//! file locks win over all of them.
//!
//! Nothing here stops the app: a line that cannot be read is dropped, a
//! value that is not valid falls back (to the default, or for a restrictive
//! key to its restrictive reading) and each of those is a [`Finding`] to
//! show. What a repository grants counts only when the repository is
//! trusted; what it restricts counts at once.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use swarmux_application::Settings;
use swarmux_domain::layers::{Effect, MergeKind, Tier};
use swarmux_domain::ledger::{hex, sha256};
use toml::{Table, Value};

use crate::config::settings_from_table;
use crate::config_registry::{KeyDef, Registry, Ty};

/// The `schema_version` of the files this build reads.
pub const SCHEMA_VERSION: i64 = 1;

/// How loudly a finding is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Worth knowing, never a failure (a repository's key waiting for trust).
    Info,
    /// Something was ignored or replaced; `--strict` fails on it.
    Warn,
    /// A value or a line could not be used.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Syntax,
    Unknown,
    Invalid,
    NotAllowed,
    NeedsTrust,
    Removed,
    Locked,
    Schema,
    Env,
}

/// One thing the loader did not take as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub kind: Kind,
    pub tier: Tier,
    pub source: String,
    pub line: Option<usize>,
    pub key: Option<String>,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(n) => write!(f, "{} line {n}: {}", self.source, self.message),
            None => write!(f, "{}: {}", self.source, self.message),
        }
    }
}

/// A repository whose `.swarmux/config.toml` is a layer.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    /// The owner granted trust for its files: its granting keys count.
    pub trusted: bool,
}

/// What to read.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// The organisation's file.
    pub managed: Option<PathBuf>,
    pub user: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub project: Option<Project>,
    /// Practice packs: a label and the TOML text of its settings.
    pub packs: Vec<(String, String)>,
    /// A mission's settings: a label and TOML text.
    pub mission: Option<(String, String)>,
    /// Environment variables (all of them; only `SWARMUX_*__*` are read).
    pub env: Vec<(String, String)>,
    /// `key=value` pairs the user typed on the command line.
    pub flags: Vec<(String, String)>,
}

/// One tier's value for a key.
#[derive(Clone, Debug, PartialEq)]
pub struct Contribution {
    pub tier: Tier,
    pub source: String,
    pub line: Option<usize>,
    pub value: Value,
}

/// A value that was not used, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Ignored {
    pub contribution: Contribution,
    pub why: String,
}

/// A key's effective value and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// `None` for a key with no default that nobody set.
    pub value: Option<Value>,
    pub tier: Tier,
    pub source: String,
    pub line: Option<usize>,
    pub merge: MergeKind,
    pub effect: Effect,
    /// Every tier's value that took part, lowest tier first.
    pub chain: Vec<Contribution>,
    /// Those of the chain that did not win (or, for a list, were merged).
    pub overridden: Vec<Contribution>,
    pub ignored: Vec<Ignored>,
    /// The managed file that locked it.
    pub locked_by: Option<String>,
    pub is_default: bool,
}

/// A file that was looked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerInfo {
    pub tier: Tier,
    pub source: String,
    pub found: bool,
}

/// What loading found.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub settings: Settings,
    /// Every registered key (its default when nobody set it), and every
    /// key of an open section that is set.
    pub entries: BTreeMap<String, Entry>,
    pub findings: Vec<Finding>,
    pub layers: Vec<LayerInfo>,
    /// The merged values, as TOML.
    pub merged: Table,
    /// A hash of the merged values.
    pub sha256: String,
    /// What the managed file locked.
    pub locked: Vec<String>,
}

impl Resolved {
    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.get(path)
    }

    /// The configuration as TOML (without `schema_version`): everything
    /// merged, or only what `tier` sets.
    pub fn export(&self, tier: Option<Tier>) -> Result<String, String> {
        let table = match tier {
            None => self.merged.clone(),
            Some(want) => {
                let mut table = Table::new();
                for e in self.entries.values().filter(|e| !e.is_default) {
                    for c in e.chain.iter().filter(|c| c.tier == want) {
                        set_path(&mut table, &e.path, c.value.clone());
                    }
                }
                table
            }
        };
        toml::to_string(&table).map_err(|e| e.to_string())
    }

    /// The findings that make `lint` fail: errors, and with `strict` every
    /// warning.
    pub fn failing(&self, strict: bool) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Error || (strict && f.level == Level::Warn))
            .collect()
    }
}

// ---------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------

/// The 1-based line a statement's byte offset is on.
fn line_at(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text.as_bytes()[..at]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Parses `text`, dropping the lines it cannot read instead of refusing the
/// file. Each dropped line is said; a header that cannot be read takes its
/// block with it, so keys never land in the wrong table.
pub fn parse_tolerant(text: &str, tier: Tier, source: &str) -> (Table, Vec<Finding>) {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut findings = Vec::new();
    for _ in 0..=lines.len() {
        let joined = lines.join("\n");
        match toml::from_str::<Table>(&joined) {
            Ok(table) => return (table, findings),
            Err(e) => {
                let Some(span) = e.span() else { break };
                let n = line_at(&joined, span.start).min(lines.len());
                if n == 0 {
                    break;
                }
                let bad = lines[n - 1].trim().to_string();
                if bad.is_empty() {
                    break;
                }
                findings.push(Finding {
                    level: Level::Error,
                    kind: Kind::Syntax,
                    tier,
                    source: source.to_string(),
                    line: Some(n),
                    key: None,
                    message: format!("{}; this line is ignored", e.message().trim()),
                });
                let is_header = bad.starts_with('[');
                lines[n - 1].clear();
                if is_header {
                    for l in lines.iter_mut().skip(n) {
                        if l.trim_start().starts_with('[') {
                            break;
                        }
                        l.clear();
                    }
                }
            }
        }
    }
    findings.push(Finding {
        level: Level::Error,
        kind: Kind::Syntax,
        tier,
        source: source.to_string(),
        line: None,
        key: None,
        message: "the file could not be read; none of it is used".into(),
    });
    (Table::new(), findings)
}

/// The line of `path` (`section.key` or `key`) in `text`, when it is
/// written the usual way.
pub fn find_line(text: &str, path: &str) -> Option<usize> {
    let (section, key) = path.rsplit_once('.').unwrap_or(("", path));
    let mut current = String::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if let Some(header) = t.strip_prefix("[[").and_then(|h| h.strip_suffix("]]")) {
            current = header.trim().to_string();
            if current == path {
                return Some(i + 1);
            }
        } else if let Some(header) = t.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            current = header.trim().to_string();
            if current == path {
                return Some(i + 1);
            }
        } else if current == section
            && t.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        {
            return Some(i + 1);
        }
    }
    None
}

/// One value of one layer.
#[derive(Clone, Debug)]
struct Unit {
    path: String,
    value: Value,
    line: Option<usize>,
}

/// A layer, read and checked.
struct Layer {
    tier: Tier,
    source: String,
    text: String,
    units: Vec<Unit>,
    /// Values a repository set that wait for its owner's trust.
    held: Vec<Unit>,
    cli: bool,
}

fn set_path(table: &mut Table, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            table.insert(path.to_string(), value);
        }
        Some((section, key)) => {
            let entry = table
                .entry(section.to_string())
                .or_insert_with(|| Value::Table(Table::new()));
            if let Value::Table(t) = entry {
                t.insert(key.to_string(), value);
            }
        }
    }
}

struct Ctx<'a> {
    reg: &'a Registry,
    findings: Vec<Finding>,
}

impl Ctx<'_> {
    fn finding(
        &mut self,
        level: Level,
        kind: Kind,
        layer: (Tier, &str),
        line: Option<usize>,
        key: Option<&str>,
        message: String,
    ) {
        self.findings.push(Finding {
            level,
            kind,
            tier: layer.0,
            source: layer.1.to_string(),
            line,
            key: key.map(str::to_string),
            message,
        });
    }

    /// Splits a parsed table into units, saying what is unknown and what
    /// this tier may not set.
    fn flatten(
        &mut self,
        table: &Table,
        tier: Tier,
        source: &str,
        text: &str,
        trusted: bool,
        skip: &[&str],
    ) -> (Vec<Unit>, Vec<Unit>) {
        let mut units = Vec::new();
        let mut held = Vec::new();
        let mut candidates: Vec<(String, Value)> = Vec::new();
        for (k, v) in table {
            if skip.contains(&k.as_str()) {
                continue;
            }
            if k == "schema_version" {
                self.schema_version(v, tier, source, text);
                continue;
            }
            if self.reg.key(k).is_some() && !self.reg.is_section(k) {
                candidates.push((k.clone(), v.clone()));
            } else if self.reg.is_section(k) {
                match v {
                    Value::Table(inner) => {
                        for (k2, v2) in inner {
                            candidates.push((format!("{k}.{k2}"), v2.clone()));
                        }
                    }
                    _ => self.finding(
                        Level::Error,
                        Kind::Invalid,
                        (tier, source),
                        find_line(text, k),
                        Some(k),
                        format!("[{k}] must be a table; it is ignored"),
                    ),
                }
            } else {
                self.finding(
                    Level::Warn,
                    Kind::Unknown,
                    (tier, source),
                    find_line(text, k),
                    Some(k),
                    format!("{k} is not a setting swarmux knows; it is ignored"),
                );
            }
        }
        for (path, value) in candidates {
            let line = find_line(text, &path);
            let Some(def) = self.reg.key(&path) else {
                self.finding(
                    Level::Warn,
                    Kind::Unknown,
                    (tier, source),
                    line,
                    Some(&path),
                    format!("{path} is not a setting swarmux knows; it is ignored"),
                );
                continue;
            };
            if def.path == "jev" || path.starts_with("jev.") {
                // The reader of `jev` says it is removed; keep the unit so
                // it can.
                units.push(Unit { path, value, line });
                continue;
            }
            if !def.tiers.contains(tier) {
                let where_ = def
                    .tiers
                    .list()
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.finding(
                    Level::Warn,
                    Kind::NotAllowed,
                    (tier, source),
                    line,
                    Some(&path),
                    format!(
                        "{path} cannot be set in the {tier} tier{}; it is ignored (it can be set in: {where_})",
                        if def.effect == Effect::Grants && tier == Tier::Project {
                            ", it grants something and so needs the owner, not the repository"
                        } else {
                            ""
                        }
                    ),
                );
                continue;
            }
            if tier == Tier::Project && def.effect == Effect::Grants && !trusted {
                self.finding(
                    Level::Info,
                    Kind::NeedsTrust,
                    (tier, source),
                    line,
                    Some(&path),
                    format!(
                        "{path} grants something and the repository is not trusted: it waits for `swarmux trust grant`"
                    ),
                );
                held.push(Unit { path, value, line });
                continue;
            }
            units.push(Unit { path, value, line });
        }
        (units, held)
    }

    fn schema_version(&mut self, v: &Value, tier: Tier, source: &str, text: &str) {
        match v.as_integer() {
            Some(n) if n > SCHEMA_VERSION => self.finding(
                Level::Warn,
                Kind::Schema,
                (tier, source),
                find_line(text, "schema_version"),
                Some("schema_version"),
                format!(
                    "schema_version {n} is newer than this swarmux reads ({SCHEMA_VERSION}); keys it does not know are ignored"
                ),
            ),
            Some(n) if n >= 1 => {}
            _ => self.finding(
                Level::Warn,
                Kind::Schema,
                (tier, source),
                find_line(text, "schema_version"),
                Some("schema_version"),
                format!("schema_version is a whole number, {SCHEMA_VERSION} now; {v} is ignored"),
            ),
        }
    }

    /// Reads one file's text as a layer.
    fn layer(
        &mut self,
        tier: Tier,
        source: &str,
        text: String,
        trusted: bool,
        skip: &[&str],
    ) -> Layer {
        let (table, syntax) = parse_tolerant(&text, tier, source);
        self.findings.extend(syntax);
        let (units, held) = self.flatten(&table, tier, source, &text, trusted, skip);
        Layer {
            tier,
            source: source.to_string(),
            text,
            units,
            held,
            cli: false,
        }
    }

    /// Keeps the units of `layer` that the readers accept, one at a time
    /// (so one bad line costs only itself): a list that has a bad element
    /// keeps the good ones, a restrictive key with a bad value reads as its
    /// restrictive word, anything else falls back to the default.
    fn validate(&mut self, layer: &mut Layer) {
        let mut accepted = Table::new();
        let mut kept = Vec::new();
        let units = std::mem::take(&mut layer.units);
        let source_path = PathBuf::from(&layer.source);
        let probe = |trial: &Table| -> Result<Vec<String>, String> {
            settings_from_table(trial, &layer.text, &source_path)
                .map(|s| s.warnings)
                .map_err(|e| e.message)
        };
        let base_warnings = probe(&accepted).unwrap_or_default();
        let mut pending: Vec<(Unit, Option<String>)> = Vec::new();
        for unit in units {
            // An extension's key: checked by its type, not by the settings
            // reader, which does not know it.
            if let Some(def) = self.reg.key(&unit.path)
                && def.external
            {
                match def.check(&unit.value) {
                    Ok(()) => kept.push(unit),
                    Err(m) => pending.push((unit, Some(m))),
                }
                continue;
            }
            let mut trial = accepted.clone();
            set_path(&mut trial, &unit.path, unit.value.clone());
            let outcome = match probe(&trial) {
                Ok(w) => {
                    let fresh: Vec<String> = w
                        .into_iter()
                        .filter(|m| !base_warnings.contains(m))
                        .collect();
                    if unit.path == "jev" {
                        // Said by the reader; the section itself is not kept.
                        Err(fresh.join("; "))
                    } else if fresh.is_empty() {
                        Ok(())
                    } else {
                        Err(fresh.join("; "))
                    }
                }
                Err(e) => Err(e),
            };
            match outcome {
                Ok(()) => {
                    accepted = trial;
                    kept.push(unit);
                }
                Err(message) => pending.push((unit, Some(message))),
            }
        }
        for (unit, message) in pending {
            let message = message.unwrap_or_default();
            let def = self.reg.key(&unit.path);
            let tier_source = (layer.tier, layer.source.clone());
            if let Some(d) = def.as_ref().filter(|d| d.external) {
                let restrictive = d
                    .invalid
                    .as_deref()
                    .and_then(literal_value)
                    .filter(|v| d.check(v).is_ok());
                let used = match &restrictive {
                    Some(v) => format!("read as {v}, the restrictive answer"),
                    None => format!(
                        "the default{} is used",
                        if d.default.is_empty() {
                            String::new()
                        } else {
                            format!(", {}", d.default)
                        }
                    ),
                };
                self.finding(
                    Level::Error,
                    Kind::Invalid,
                    (tier_source.0, &tier_source.1),
                    unit.line,
                    Some(&unit.path),
                    format!(
                        "{}: {} is not valid ({message}); {used}",
                        unit.path, unit.value
                    ),
                );
                if let Some(v) = restrictive {
                    kept.push(Unit { value: v, ..unit });
                }
                continue;
            }
            if unit.path == "jev" {
                self.finding(
                    Level::Warn,
                    Kind::Removed,
                    (tier_source.0, &tier_source.1),
                    unit.line,
                    Some("jev"),
                    clean_removed(&message),
                );
                continue;
            }
            // A list with a bad element keeps the good ones, except an
            // allowlist of the organisation, which is enforced as empty.
            let org_allowlist = matches!(layer.tier, Tier::Org | Tier::Managed)
                && def
                    .as_ref()
                    .is_some_and(|d| d.merge == MergeKind::Allowlist);
            if let (Some(d), Value::Array(items)) = (&def, &unit.value)
                && d.ty == Ty::StrList
                && !org_allowlist
            {
                let mut good = Vec::new();
                for item in items {
                    let mut trial = accepted.clone();
                    set_path(&mut trial, &unit.path, Value::Array(vec![item.clone()]));
                    let ok =
                        probe(&trial).is_ok_and(|w| w.iter().all(|m| base_warnings.contains(m)));
                    if ok {
                        good.push(item.clone());
                    } else {
                        self.finding(
                            Level::Error,
                            Kind::Invalid,
                            (tier_source.0, &tier_source.1),
                            unit.line,
                            Some(&unit.path),
                            format!(
                                "{}: {item} is not valid ({message}); that entry is ignored",
                                unit.path
                            ),
                        );
                    }
                }
                if !good.is_empty() {
                    let mut trial = accepted.clone();
                    set_path(&mut trial, &unit.path, Value::Array(good.clone()));
                    if probe(&trial).is_ok() {
                        accepted = trial;
                        kept.push(Unit {
                            value: Value::Array(good),
                            ..unit
                        });
                    }
                }
                continue;
            }
            // A restrictive key reads as its restrictive word.
            if let Some(d) = &def
                && let Some(literal) = &d.invalid
                && let Some(v) = literal_value(literal)
            {
                let mut trial = accepted.clone();
                set_path(&mut trial, &unit.path, v.clone());
                if probe(&trial).is_ok() {
                    accepted = trial;
                    self.finding(
                        Level::Error,
                        Kind::Invalid,
                        (tier_source.0, &tier_source.1),
                        unit.line,
                        Some(&unit.path),
                        format!(
                            "{}: {} is not valid ({message}); read as {literal}, the restrictive answer",
                            unit.path, unit.value
                        ),
                    );
                    kept.push(Unit { value: v, ..unit });
                    continue;
                }
            }
            // An allowlist from the organisation that cannot be read is
            // enforced as empty.
            let managed = matches!(layer.tier, Tier::Org | Tier::Managed);
            if managed
                && let Some(d) = &def
                && d.merge == MergeKind::Allowlist
                && d.ty == Ty::StrList
            {
                let mut trial = accepted.clone();
                set_path(&mut trial, &unit.path, Value::Array(Vec::new()));
                if probe(&trial).is_ok() {
                    accepted = trial;
                    self.finding(
                        Level::Error,
                        Kind::Invalid,
                        (tier_source.0, &tier_source.1),
                        unit.line,
                        Some(&unit.path),
                        format!("{}: {} is not valid ({message}); an allowlist that cannot be read is enforced as empty", unit.path, unit.value),
                    );
                    kept.push(Unit {
                        value: Value::Array(Vec::new()),
                        ..unit
                    });
                    continue;
                }
            }
            self.finding(
                Level::Error,
                Kind::Invalid,
                (tier_source.0, &tier_source.1),
                unit.line,
                Some(&unit.path),
                format!(
                    "{}: {} is not valid ({message}); {} is used",
                    unit.path,
                    unit.value,
                    def.as_ref().filter(|d| !d.default.is_empty()).map_or(
                        "the default".to_string(),
                        |d| format!("the default, {}", d.default)
                    )
                ),
            );
        }
        layer.units = kept;
    }
}

/// The reader's text about a removed section, without its file name prefix.
fn clean_removed(message: &str) -> String {
    message
        .find("JEV was removed")
        .map_or(message, |i| &message[i..])
        .to_string()
}

/// A TOML literal as a value.
pub fn literal_value(literal: &str) -> Option<Value> {
    let table: Table = toml::from_str(&format!("v = {literal}")).ok()?;
    table.get("v").cloned()
}

/// A value typed as `def` says, from text a person or an environment
/// variable wrote.
pub fn typed(def: &KeyDef, text: &str) -> Result<Value, String> {
    let t = text.trim();
    match &def.ty {
        Ty::Bool => match t.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(Value::Boolean(true)),
            "false" | "0" | "no" | "off" => Ok(Value::Boolean(false)),
            _ => Err(format!("{t:?} is not true or false")),
        },
        Ty::Int => t
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| format!("{t:?} is not a whole number")),
        Ty::Float => t
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("{t:?} is not a number")),
        Ty::Str | Ty::Enum(_) => Ok(Value::String(t.to_string())),
        Ty::StrList => {
            if t.starts_with('[') {
                literal_value(t).ok_or_else(|| format!("{t:?} is not a TOML list"))
            } else {
                Ok(Value::Array(
                    t.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| Value::String(s.to_string()))
                        .collect(),
                ))
            }
        }
        Ty::Table | Ty::Any => Ok(literal_value(t).unwrap_or_else(|| Value::String(t.to_string()))),
    }
}

/// `SWARMUX_GUARD__ON_UNREACHABLE` is `guard.on_unreachable`;
/// `SWARMUX__THEME` is the top-level `theme`. Other `SWARMUX_*` variables
/// (`SWARMUX_CONFIG_DIR`, `SWARMUX_CLAUDE_BIN`) are not settings.
pub fn env_key(name: &str) -> Option<String> {
    let rest = name.strip_prefix("SWARMUX_")?;
    if let Some(key) = rest.strip_prefix('_') {
        return (!key.is_empty()).then(|| key.to_ascii_lowercase());
    }
    let (section, key) = rest.split_once("__")?;
    (!section.is_empty() && !key.is_empty()).then(|| {
        format!(
            "{}.{}",
            section.to_ascii_lowercase(),
            key.to_ascii_lowercase()
        )
    })
}

// ---------------------------------------------------------------------
// Merging
// ---------------------------------------------------------------------

struct Cand {
    contribution: Contribution,
    cli: bool,
}

fn merge_list(cands: &[Cand]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for c in cands {
        if let Value::Array(items) = &c.contribution.value {
            for i in items {
                if !out.contains(i) {
                    out.push(i.clone());
                }
            }
        }
    }
    Value::Array(out)
}

/// Which candidate wins for `def`, and the merged value for a list.
fn pick(def: &KeyDef, cands: &[Cand]) -> (usize, Option<Value>) {
    let last = cands.len() - 1;
    match def.merge {
        MergeKind::Scalar | MergeKind::Allowlist => (last, None),
        MergeKind::List => (last, Some(merge_list(cands))),
        MergeKind::Lock => {
            // What the user typed on the command line wins outright; the
            // strictest of the rest does.
            if let Some(i) = cands.iter().rposition(|c| c.cli) {
                return (i, None);
            }
            let mut best = 0;
            for (i, c) in cands.iter().enumerate().skip(1) {
                if def
                    .strict
                    .compare(&c.contribution.value, &cands[best].contribution.value)
                    != std::cmp::Ordering::Greater
                {
                    best = i;
                }
            }
            (best, None)
        }
    }
}

/// Reads every tier of `inputs` and merges them.
pub fn resolve(reg: &Registry, inputs: &Inputs) -> Resolved {
    let mut ctx = Ctx {
        reg,
        findings: Vec::new(),
    };
    let mut infos = Vec::new();
    let mut layers: Vec<Layer> = Vec::new();
    let mut locked: Vec<String> = Vec::new();
    let mut managed_layer: Option<Layer> = None;

    let read = |path: &Path| std::fs::read_to_string(path).ok();

    // The organisation's file: what it locks is a layer above everything,
    // the rest a default below the user's.
    if let Some(path) = &inputs.managed {
        let source = path.display().to_string();
        match read(path) {
            Some(text) => {
                infos.push(LayerInfo {
                    tier: Tier::Managed,
                    source: source.clone(),
                    found: true,
                });
                let mut all = ctx.layer(Tier::Org, &source, text, true, &["locked"]);
                ctx.validate(&mut all);
                let names: Vec<String> = match toml::from_str::<Table>(&all.text)
                    .ok()
                    .and_then(|t| t.get("locked").cloned())
                {
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                        .collect(),
                    _ => Vec::new(),
                };
                for n in &names {
                    if reg.key(n).is_none() {
                        ctx.finding(
                            Level::Warn,
                            Kind::Unknown,
                            (Tier::Managed, &source),
                            find_line(&all.text, "locked"),
                            Some(n),
                            format!("locked names {n}, which is not a setting swarmux knows"),
                        );
                    }
                }
                let (lock_units, rest): (Vec<Unit>, Vec<Unit>) =
                    all.units.into_iter().partition(|u| names.contains(&u.path));
                for n in &names {
                    if reg.key(n).is_some() && !lock_units.iter().any(|u| &u.path == n) {
                        ctx.finding(
                            Level::Warn,
                            Kind::Locked,
                            (Tier::Managed, &source),
                            find_line(&all.text, "locked"),
                            Some(n),
                            format!(
                                "locked names {n} but the file does not set it; nothing is locked"
                            ),
                        );
                    }
                }
                locked = lock_units.iter().map(|u| u.path.clone()).collect();
                managed_layer = Some(Layer {
                    tier: Tier::Managed,
                    source: source.clone(),
                    text: all.text.clone(),
                    units: lock_units,
                    held: Vec::new(),
                    cli: false,
                });
                layers.push(Layer {
                    tier: Tier::Org,
                    source,
                    text: all.text,
                    units: rest,
                    held: Vec::new(),
                    cli: false,
                });
            }
            None => infos.push(LayerInfo {
                tier: Tier::Managed,
                source,
                found: false,
            }),
        }
    }

    let file_layer = |ctx: &mut Ctx<'_>,
                      layers: &mut Vec<Layer>,
                      infos: &mut Vec<LayerInfo>,
                      tier: Tier,
                      path: &Path,
                      trusted: bool| {
        let source = path.display().to_string();
        match read(path) {
            Some(text) => {
                infos.push(LayerInfo {
                    tier,
                    source: source.clone(),
                    found: true,
                });
                let mut l = ctx.layer(tier, &source, text, trusted, &[]);
                ctx.validate(&mut l);
                layers.push(l);
            }
            None => infos.push(LayerInfo {
                tier,
                source,
                found: false,
            }),
        }
    };
    if let Some(p) = &inputs.user {
        file_layer(&mut ctx, &mut layers, &mut infos, Tier::User, p, true);
    }
    if let Some(p) = &inputs.workspace {
        file_layer(&mut ctx, &mut layers, &mut infos, Tier::Workspace, p, true);
    }
    if let Some(project) = &inputs.project {
        let path = project.root.join(".swarmux").join("config.toml");
        file_layer(
            &mut ctx,
            &mut layers,
            &mut infos,
            Tier::Project,
            &path,
            project.trusted,
        );
    }
    for (label, text) in &inputs.packs {
        let mut l = ctx.layer(Tier::Pack, label, text.clone(), true, &[]);
        ctx.validate(&mut l);
        layers.push(l);
    }
    if let Some((label, text)) = &inputs.mission {
        let mut l = ctx.layer(Tier::Mission, label, text.clone(), true, &[]);
        ctx.validate(&mut l);
        layers.push(l);
    }

    // The run tier: variables, then flags typed on the command line.
    let mut env_units = Vec::new();
    let mut env_names: Vec<(String, String)> = inputs.env.clone();
    env_names.sort();
    for (name, value) in &env_names {
        let Some(path) = env_key(name) else { continue };
        let Some(def) = reg.key(&path) else {
            ctx.finding(
                Level::Warn,
                Kind::Unknown,
                (Tier::Run, name),
                None,
                Some(&path),
                format!("{name} names {path}, which is not a setting swarmux knows; it is ignored"),
            );
            continue;
        };
        if !def.env {
            ctx.finding(
                Level::Warn,
                Kind::Env,
                (Tier::Run, name),
                None,
                Some(&path),
                format!("{path} grants something or is the file's own; it cannot be set from the environment, only in a file"),
            );
            continue;
        }
        if !def.tiers.contains(Tier::Run) {
            ctx.finding(
                Level::Warn,
                Kind::NotAllowed,
                (Tier::Run, name),
                None,
                Some(&path),
                format!("{path} cannot be set at run time; it is ignored"),
            );
            continue;
        }
        match typed(&def, value) {
            Ok(v) => env_units.push(Unit {
                path,
                value: v,
                line: None,
            }),
            Err(e) => ctx.finding(
                Level::Error,
                Kind::Env,
                (Tier::Run, name),
                None,
                Some(&path),
                format!("{name}: {e}; it is ignored"),
            ),
        }
    }
    if !env_units.is_empty() {
        let mut l = Layer {
            tier: Tier::Run,
            source: "environment".into(),
            text: String::new(),
            units: env_units,
            held: Vec::new(),
            cli: false,
        };
        ctx.validate(&mut l);
        layers.push(l);
    }
    let mut flag_units = Vec::new();
    for (key, value) in &inputs.flags {
        let Some(def) = reg.key(key) else {
            ctx.finding(
                Level::Warn,
                Kind::Unknown,
                (Tier::Run, "--set"),
                None,
                Some(key),
                format!("--set {key}: not a setting swarmux knows; it is ignored"),
            );
            continue;
        };
        if !def.tiers.contains(Tier::Run) {
            ctx.finding(
                Level::Warn,
                Kind::NotAllowed,
                (Tier::Run, "--set"),
                None,
                Some(key),
                format!("--set {key}: cannot be set at run time; it is ignored"),
            );
            continue;
        }
        match typed(&def, value) {
            Ok(v) => flag_units.push(Unit {
                path: key.clone(),
                value: v,
                line: None,
            }),
            Err(e) => ctx.finding(
                Level::Error,
                Kind::Invalid,
                (Tier::Run, "--set"),
                None,
                Some(key),
                format!("--set {key}: {e}; it is ignored"),
            ),
        }
    }
    if !flag_units.is_empty() {
        let mut l = Layer {
            tier: Tier::Run,
            source: "--set".into(),
            text: String::new(),
            units: flag_units,
            held: Vec::new(),
            cli: true,
        };
        ctx.validate(&mut l);
        layers.push(l);
    }

    // Order: by tier, packs and the like in the order given.
    layers.sort_by_key(|l| l.tier);

    // Gather every key's candidates.
    let mut by_key: BTreeMap<String, Vec<Cand>> = BTreeMap::new();
    let mut ignored: BTreeMap<String, Vec<Ignored>> = BTreeMap::new();
    for l in &layers {
        for u in &l.units {
            by_key.entry(u.path.clone()).or_default().push(Cand {
                contribution: Contribution {
                    tier: l.tier,
                    source: l.source.clone(),
                    line: u.line,
                    value: u.value.clone(),
                },
                cli: l.cli,
            });
        }
        for u in &l.held {
            ignored.entry(u.path.clone()).or_default().push(Ignored {
                contribution: Contribution {
                    tier: l.tier,
                    source: l.source.clone(),
                    line: u.line,
                    value: u.value.clone(),
                },
                why: "the repository is not trusted yet (`swarmux trust grant`)".into(),
            });
        }
    }
    let mut managed_values: BTreeMap<String, Contribution> = BTreeMap::new();
    if let Some(m) = &managed_layer {
        for u in &m.units {
            managed_values.insert(
                u.path.clone(),
                Contribution {
                    tier: Tier::Managed,
                    source: m.source.clone(),
                    line: u.line,
                    value: u.value.clone(),
                },
            );
        }
    }

    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut merged = Table::new();
    // What swarmux's own settings read: without the extensions' keys.
    let mut core_merged = Table::new();
    let mut paths: BTreeSet<String> = by_key.keys().cloned().collect();
    paths.extend(managed_values.keys().cloned());
    for path in &paths {
        let Some(def) = reg.key(path) else { continue };
        let cands = by_key.remove(path).unwrap_or_default();
        let lock = managed_values.get(path);
        // The default is the bottom of every chain: what the others overrode.
        let core: Option<Contribution> = default_entry(&def, Vec::new()).chain.into_iter().next();
        let mut chain: Vec<Contribution> = core
            .iter()
            .cloned()
            .chain(cands.iter().map(|c| c.contribution.clone()))
            .collect();
        if let Some(l) = lock {
            chain.push(l.clone());
        }
        let (value, winner, overridden) = match (lock, cands.is_empty()) {
            (Some(l), _) => (
                l.value.clone(),
                l.clone(),
                chain[..chain.len() - 1].to_vec(),
            ),
            (None, false) => {
                let (i, listed) = pick(&def, &cands);
                let winner = cands[i].contribution.clone();
                // A list is merged, not overridden.
                let overridden = if def.merge == MergeKind::List {
                    Vec::new()
                } else {
                    core.iter()
                        .cloned()
                        .chain(
                            cands
                                .iter()
                                .enumerate()
                                .filter(|(j, _)| *j != i)
                                .map(|(_, c)| c.contribution.clone()),
                        )
                        .collect()
                };
                (
                    listed.unwrap_or_else(|| winner.value.clone()),
                    winner,
                    overridden,
                )
            }
            (None, true) => continue,
        };
        set_path(&mut merged, path, value.clone());
        if !def.external {
            set_path(&mut core_merged, path, value.clone());
        }
        entries.insert(
            path.clone(),
            Entry {
                path: path.clone(),
                value: Some(value),
                tier: winner.tier,
                source: winner.source.clone(),
                line: winner.line,
                merge: def.merge,
                effect: def.effect,
                chain,
                overridden,
                ignored: ignored.remove(path).unwrap_or_default(),
                locked_by: lock.map(|l| l.source.clone()),
                is_default: false,
            },
        );
    }
    // Keys a repository set that wait for trust, and nobody else set.
    for (path, ign) in ignored {
        if entries.contains_key(&path) {
            continue;
        }
        if let Some(def) = reg.key(&path) {
            entries.insert(path.clone(), default_entry(&def, ign));
        }
    }
    for def in reg.keys() {
        entries
            .entry(def.path.clone())
            .or_insert_with(|| default_entry(def, Vec::new()));
    }

    // The settings the app reads.
    let text_for_lines = layers
        .iter()
        .find(|l| l.tier == Tier::User)
        .map(|l| l.text.clone())
        .unwrap_or_default();
    let (mut settings, mut final_findings) = finalize(&core_merged, &text_for_lines);
    ctx.findings.append(&mut final_findings);
    // Findings that are worth saying on start.
    let mut warnings: Vec<String> = ctx
        .findings
        .iter()
        .filter(|f| f.level >= Level::Warn)
        .map(|f| f.to_string())
        .collect();
    warnings.dedup();
    settings.warnings = warnings;
    let sha = hex(&sha256(
        toml::to_string(&merged).unwrap_or_default().as_bytes(),
    ));
    let mut infos_out = infos;
    infos_out.sort_by_key(|i| i.tier);
    // Where each Guard rule came from, highest tier last so it wins.
    let mut rule_origins = std::collections::BTreeMap::new();
    for path in ["guard.allow", "guard.deny", "guard.protected_paths"] {
        if let Some(e) = entries.get(path) {
            for c in e.chain.iter().filter(|c| c.tier != Tier::Core) {
                if let Value::Array(items) = &c.value {
                    for item in items.iter().filter_map(Value::as_str) {
                        rule_origins.insert(
                            item.to_string(),
                            swarmux_domain::layers::Origin {
                                tier: c.tier,
                                source: c.source.clone(),
                            },
                        );
                    }
                }
            }
        }
    }
    settings.provenance = swarmux_application::Provenance {
        config_sha256: sha.clone(),
        layers: infos_out
            .iter()
            .map(|l| (l.tier, l.source.clone(), l.found))
            .collect(),
        rule_origins,
    };
    Resolved {
        settings,
        entries,
        findings: ctx.findings,
        layers: infos_out,
        merged,
        sha256: sha,
        locked,
    }
}

fn default_entry(def: &KeyDef, ignored: Vec<Ignored>) -> Entry {
    let value = (!def.default.is_empty())
        .then(|| literal_value(&def.default))
        .flatten();
    Entry {
        path: def.path.clone(),
        value: value.clone(),
        tier: Tier::Core,
        source: "built-in default".into(),
        line: None,
        merge: def.merge,
        effect: def.effect,
        chain: value
            .map(|v| {
                vec![Contribution {
                    tier: Tier::Core,
                    source: "built-in default".into(),
                    line: None,
                    value: v,
                }]
            })
            .unwrap_or_default(),
        overridden: Vec::new(),
        ignored,
        locked_by: None,
        is_default: true,
    }
}

/// The settings of the merged table. Every value passed its own tier's
/// check, so this normally succeeds; if values from different tiers do not
/// go together, the later ones are dropped one at a time and said.
fn finalize(merged: &Table, text: &str) -> (Settings, Vec<Finding>) {
    let path = Path::new("config.toml");
    match settings_from_table(merged, text, path) {
        Ok(s) => (s, Vec::new()),
        Err(first) => {
            let mut findings = vec![Finding {
                level: Level::Error,
                kind: Kind::Invalid,
                tier: Tier::Core,
                source: "merged configuration".into(),
                line: None,
                key: None,
                message: format!(
                    "values from different tiers do not go together ({}); the ones that fail are dropped",
                    first.message
                ),
            }];
            let mut accepted = Table::new();
            let mut flat: Vec<(String, Value)> = Vec::new();
            for (k, v) in merged {
                match v {
                    Value::Table(inner) if k != "keys" && k != "deck" => {
                        for (k2, v2) in inner {
                            flat.push((format!("{k}.{k2}"), v2.clone()));
                        }
                    }
                    _ => flat.push((k.clone(), v.clone())),
                }
            }
            for (p, v) in flat {
                let mut trial = accepted.clone();
                set_path(&mut trial, &p, v);
                if settings_from_table(&trial, text, path).is_ok() {
                    accepted = trial;
                } else {
                    findings.push(Finding {
                        level: Level::Error,
                        kind: Kind::Invalid,
                        tier: Tier::Core,
                        source: "merged configuration".into(),
                        line: None,
                        key: Some(p.clone()),
                        message: format!("{p} is dropped: it does not go with the others"),
                    });
                }
            }
            let settings = settings_from_table(&accepted, text, path).unwrap_or_default();
            (settings, findings)
        }
    }
}

// ---------------------------------------------------------------------
// The process's own layers
// ---------------------------------------------------------------------

static RUN_FLAGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The `--set key=value` pairs of this run, read by every later load.
pub fn set_run_flags(flags: Vec<(String, String)>) {
    *RUN_FLAGS.lock().unwrap_or_else(PoisonError::into_inner) = flags;
}

fn run_flags() -> Vec<(String, String)> {
    RUN_FLAGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The environment as text pairs.
pub fn process_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(k, v): (OsString, OsString)| {
            Some((k.into_string().ok()?, v.into_string().ok()?))
        })
        .filter(|(k, _)| k.starts_with("SWARMUX_"))
        .collect()
}

/// What to read for this process: the organisation's file, the user's
/// `config.toml`, the workspace named by `SWARMUX_WORKSPACE` (or `workspace`),
/// the environment and this run's flags.
pub fn process_inputs(paths: &crate::Paths, workspace: Option<&str>) -> Inputs {
    let env = process_env();
    let workspace = workspace.map(str::to_string).or_else(|| {
        env.iter()
            .find(|(k, _)| k == "SWARMUX_WORKSPACE")
            .map(|(_, v)| v.clone())
    });
    Inputs {
        managed: Some(paths.managed.clone()),
        user: Some(paths.config.clone()),
        workspace: workspace.map(|w| workspace_file(paths, &w)),
        env,
        flags: run_flags(),
        ..Inputs::default()
    }
}

/// Where a workspace's own settings live.
pub fn workspace_file(paths: &crate::Paths, name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    paths
        .config_dir
        .join("workspaces")
        .join(format!("{safe}.toml"))
}

/// The effective settings for this process. Never fails: what cannot be
/// read is in `settings.warnings` and `Resolved::findings`.
pub fn load_effective(paths: &crate::Paths) -> Resolved {
    resolve(&Registry::core(), &process_inputs(paths, None))
}

/// The effective settings of this process, for the commands that only need
/// them: never fails, and says what it replaced in `warnings`.
pub fn load_effective_settings(paths: &crate::Paths) -> Settings {
    load_effective(paths).settings
}

#[cfg(test)]
mod tests;

/// What a repository's `.swarmux/config.toml` restricts, read for the gate:
/// these keys count whether or not the repository is trusted.
#[derive(Clone, Debug, Default)]
pub struct ProjectRestrictions {
    /// Globs agents may not write (`guard.protected_paths`).
    pub protected: Vec<String>,
    /// Problems with the file, for the gate's warnings.
    pub warnings: Vec<String>,
}

/// Reads `root/.swarmux/config.toml` as the project tier and returns the
/// restrictions in it. A missing file restricts nothing. Globs that climb out
/// of the repository are dropped and said.
pub fn project_restrictions(root: &Path) -> ProjectRestrictions {
    if !root.join(".swarmux").join("config.toml").is_file() {
        return ProjectRestrictions::default();
    }
    let r = resolve(
        &Registry::core(),
        &Inputs {
            project: Some(Project {
                root: root.to_path_buf(),
                trusted: false,
            }),
            ..Inputs::default()
        },
    );
    let mut out = ProjectRestrictions::default();
    for glob in r.settings.guard.protected_paths {
        if glob.split(['/', '\\']).any(|s| s == "..") {
            out.warnings.push(format!(
                ".swarmux/config.toml: guard.protected_paths {glob:?} may not contain ..; it is left out"
            ));
        } else {
            out.protected.push(glob);
        }
    }
    out.warnings.extend(
        r.findings
            .iter()
            .filter(|f| f.level >= Level::Warn)
            .map(|f| f.to_string()),
    );
    out
}
