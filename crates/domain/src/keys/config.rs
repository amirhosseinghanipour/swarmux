//! What a user's `[keys]` asks for, and how it is checked against the
//! default keymap.
//!
//! The config is read into a [`KeyConfig`] without judging it: entries are
//! text as written. [`apply`] then checks every entry and builds the
//! keymap that is used. A bad entry is reported and does not take effect;
//! the defaults keep working for it. Nothing here fails, so a mistake in
//! the config can never leave the app without keys.

use std::fmt;

use super::keymap::{Action, BindError, Catalog, Context, Keymap, Shadow};
use super::spec::{Chord, KeyError, KeySpec, is_reserved};

/// One line of `[keys]`: `name = "key"` or `name = ["key", "key"]` inside
/// the table `section`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEntry {
    /// The tables the line is in, as written, below `[keys]`
    /// (`mission.wall`). Empty when it is directly in `[keys]`.
    pub section: String,
    /// The key of the line, as written (`list.prev`).
    pub name: String,
    /// The keys, as written. Empty unbinds the action.
    pub keys: Vec<String>,
}

/// A part of `[keys]` that is not an entry at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed {
    /// Where, as a dotted path below `[keys]`.
    pub at: String,
    /// What was there instead of a key or a list of keys.
    pub found: String,
}

/// The `[keys]` section as written, not yet checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyConfig {
    pub entries: Vec<KeyEntry>,
    pub malformed: Vec<Malformed>,
}

impl KeyConfig {
    /// Nothing is configured.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.malformed.is_empty()
    }
}

/// How a context is written as a table name: quoted when it has a dot, as
/// TOML needs.
pub fn table_name(context: &str) -> String {
    if context.contains('.') {
        format!("\"{context}\"")
    } else {
        context.to_string()
    }
}

/// How an action id is written as a key: quoted when it has a dot.
pub fn key_name(action: &str) -> String {
    table_name(action)
}

/// Something in the config that can't be used. Each says where: the
/// context, the action and the key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyProblem {
    /// A value that is not a key or a list of keys.
    Malformed { at: String, found: String },
    /// No context of that name (or, for a line directly in `[keys]`, no
    /// table around it).
    UnknownContext {
        context: String,
        action: String,
        /// Contexts that have an action of that name.
        elsewhere: Vec<String>,
        known: Vec<String>,
    },
    UnknownAction {
        context: Context,
        action: String,
        /// Contexts that have an action of that name.
        elsewhere: Vec<String>,
        known: Vec<String>,
    },
    /// The same action set twice.
    Twice { context: Context, action: Action },
    /// An action whose keys are fixed.
    Locked { context: Context, action: Action },
    /// A key that could not be read.
    BadKey {
        context: Context,
        action: Action,
        key: String,
        error: KeyError,
    },
    /// `Esc` and `Ctrl+C`.
    Reserved {
        context: Context,
        action: Action,
        key: KeySpec,
    },
    /// Two actions on one key in one context.
    Conflict {
        context: Context,
        action: Action,
        chord: Chord,
        existing: Action,
    },
    /// One chord is the start of another.
    Prefix {
        context: Context,
        action: Action,
        chord: Chord,
        other: Chord,
    },
    /// The keys of a context could not all be kept; it uses its defaults.
    Reverted { context: Context },
}

impl KeyProblem {
    /// The context it is about, as the config names it, when there is one.
    pub fn context(&self) -> Option<&str> {
        match self {
            KeyProblem::UnknownContext { context, .. } => Some(context),
            KeyProblem::UnknownAction { context, .. }
            | KeyProblem::Twice { context, .. }
            | KeyProblem::Locked { context, .. }
            | KeyProblem::BadKey { context, .. }
            | KeyProblem::Reserved { context, .. }
            | KeyProblem::Conflict { context, .. }
            | KeyProblem::Prefix { context, .. }
            | KeyProblem::Reverted { context } => Some(context.id()),
            KeyProblem::Malformed { .. } => None,
        }
    }
}

fn list(items: &[String]) -> String {
    items.join(", ")
}

fn elsewhere(action: &str, contexts: &[String]) -> String {
    match contexts {
        [] => String::new(),
        [one] => format!(" (`{action}` is in `[keys.{}]`)", table_name(one)),
        many => format!(
            " (`{action}` is in {})",
            many.iter()
                .map(|c| format!("`[keys.{}]`", table_name(c)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

impl fmt::Display for KeyProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyProblem::Malformed { at, found } => write!(
                f,
                "[keys] {at}: expected a key like \"x\" or a list of keys, found {found}"
            ),
            KeyProblem::UnknownContext {
                context,
                action,
                elsewhere: there,
                known,
            } if context.is_empty() => write!(
                f,
                "[keys] {action}: an action goes in the table of its context, not directly in \
                 [keys]{}; contexts: {}",
                elsewhere(action, there),
                list(known)
            ),
            KeyProblem::UnknownContext {
                context,
                action,
                elsewhere: there,
                known,
            } => write!(
                f,
                "[keys.{}] {action}: no context called `{context}`{}; contexts: {}",
                table_name(context),
                elsewhere(action, there),
                list(known)
            ),
            KeyProblem::UnknownAction {
                context,
                action,
                elsewhere: there,
                known,
            } => write!(
                f,
                "[keys.{}] {}: no action called `{action}` in `{context}`{}; its actions: {}",
                table_name(context.id()),
                key_name(action),
                elsewhere(action, there),
                list(known)
            ),
            KeyProblem::Twice { context, action } => write!(
                f,
                "[keys.{}] {}: set more than once; the first one is used",
                table_name(context.id()),
                key_name(action.id())
            ),
            KeyProblem::Locked { context, action } => write!(
                f,
                "[keys.{}] {}: this action's keys are fixed on purpose (the answers to approvals \
                 and Esc stay where they are) and can't be rebound",
                table_name(context.id()),
                key_name(action.id())
            ),
            KeyProblem::BadKey {
                context,
                action,
                key,
                error,
            } => write!(
                f,
                "[keys.{}] {} = \"{key}\": {error}",
                table_name(context.id()),
                key_name(action.id())
            ),
            KeyProblem::Reserved {
                context,
                action,
                key,
            } => write!(
                f,
                "[keys.{}] {} = \"{key}\": `{key}` is reserved (Esc and Ctrl+C always go back \
                 and quit) and can't be bound",
                table_name(context.id()),
                key_name(action.id())
            ),
            KeyProblem::Conflict {
                context,
                action,
                chord,
                existing,
            } => write!(
                f,
                "[keys.{}] {} = \"{chord}\": `{chord}` is already {existing} in {context}; \
                 move or unbind {existing} too ({} = [])",
                table_name(context.id()),
                key_name(action.id()),
                key_name(existing.id())
            ),
            KeyProblem::Prefix {
                context,
                action,
                chord,
                other,
            } => write!(
                f,
                "[keys.{}] {} = \"{chord}\": `{chord}` and `{other}` can't both be bound in \
                 {context}, because one starts the other",
                table_name(context.id()),
                key_name(action.id())
            ),
            KeyProblem::Reverted { context } => write!(
                f,
                "[keys.{}]: the keys of {context} could not all be kept, so {context} uses its \
                 default keys",
                table_name(context.id())
            ),
        }
    }
}

/// The keymap in use, and what was wrong with the config.
#[derive(Clone, Debug)]
pub struct Applied {
    pub keymap: Keymap,
    pub problems: Vec<KeyProblem>,
}

/// One entry that passed every check on its own.
struct Accepted {
    context: Context,
    action: Action,
    chords: Vec<Chord>,
}

/// The defaults with the entries of `config` applied.
///
/// Each entry stands on its own: one that names something unknown, can't be
/// read, uses a reserved key or an action whose keys are fixed, or that
/// clashes with another key in its context, is reported and leaves its
/// action on the default keys. An entry replaces all the default keys of
/// its action, and an empty list leaves the action without a key.
pub fn apply(defaults: &Keymap, catalog: &Catalog, config: &KeyConfig) -> Applied {
    let mut problems: Vec<KeyProblem> = config
        .malformed
        .iter()
        .map(|m| KeyProblem::Malformed {
            at: m.at.clone(),
            found: m.found.clone(),
        })
        .collect();

    let mut accepted: Vec<Accepted> = Vec::new();
    for entry in &config.entries {
        let (context, action) = match resolve(catalog, entry) {
            Ok(found) => found,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        if accepted
            .iter()
            .any(|a| a.context == context && a.action == action)
        {
            problems.push(KeyProblem::Twice { context, action });
            continue;
        }
        if catalog.is_locked(context, action) {
            problems.push(KeyProblem::Locked { context, action });
            continue;
        }
        match read_chords(context, action, &entry.keys) {
            Ok(chords) => accepted.push(Accepted {
                context,
                action,
                chords,
            }),
            Err(mut found) => problems.append(&mut found),
        }
    }

    let mut contexts: Vec<Context> = accepted.iter().map(|a| a.context).collect();
    contexts.dedup();
    let mut seen: Vec<Context> = Vec::new();
    let mut keymap = defaults.clone();
    for context in contexts {
        if seen.contains(&context) {
            continue;
        }
        seen.push(context);
        let wanted: Vec<&Accepted> = accepted.iter().filter(|a| a.context == context).collect();
        let rebuilt = rebuild(defaults, context, &wanted, &mut problems);
        keymap = replace_context(&keymap, context, rebuilt);
    }
    Applied { keymap, problems }
}

/// The context and action an entry names.
///
/// TOML turns `[keys.mission.wall]` into a table inside a table, and an
/// unquoted `list.prev` into another, so the path is cut into a context and
/// an action wherever the catalog knows both.
fn resolve(catalog: &Catalog, entry: &KeyEntry) -> Result<(Context, Action), KeyProblem> {
    let path = if entry.section.is_empty() {
        entry.name.clone()
    } else {
        format!("{}.{}", entry.section, entry.name)
    };
    let parts: Vec<&str> = path.split('.').collect();
    for cut in 1..parts.len() {
        let (context, action) = (parts[..cut].join("."), parts[cut..].join("."));
        if let Some(found) = catalog.find(&context, &action)
            && let Some(context) = catalog.context(&context)
        {
            return Ok((context, found));
        }
    }
    let known_contexts: Vec<String> = catalog
        .contexts()
        .iter()
        .map(|c| c.id().to_string())
        .collect();
    let having = |action: &str| -> Vec<String> {
        catalog
            .defs()
            .iter()
            .filter(|d| d.action.id() == action)
            .map(|d| d.context.id().to_string())
            .collect()
    };
    for cut in (1..parts.len()).rev() {
        if let Some(context) = catalog.context(&parts[..cut].join(".")) {
            let action = parts[cut..].join(".");
            return Err(KeyProblem::UnknownAction {
                context,
                elsewhere: having(&action),
                known: catalog
                    .of(context)
                    .map(|d| d.action.id().to_string())
                    .collect(),
                action,
            });
        }
    }
    let action = if entry.section.is_empty() {
        path.clone()
    } else {
        entry.name.clone()
    };
    Err(KeyProblem::UnknownContext {
        context: entry.section.clone(),
        elsewhere: having(&action),
        known: known_contexts,
        action,
    })
}

/// The chords of an entry, or every reason it has none.
fn read_chords(
    context: Context,
    action: Action,
    keys: &[String],
) -> Result<Vec<Chord>, Vec<KeyProblem>> {
    let mut chords: Vec<Chord> = Vec::new();
    let mut problems = Vec::new();
    for text in keys {
        match Chord::parse(text) {
            Ok(chord) => {
                if let Some(key) = chord.keys().iter().copied().find(|k| is_reserved(*k)) {
                    problems.push(KeyProblem::Reserved {
                        context,
                        action,
                        key,
                    });
                } else if !chords.contains(&chord) {
                    chords.push(chord);
                }
            }
            Err(error) => problems.push(KeyProblem::BadKey {
                context,
                action,
                key: text.clone(),
                error,
            }),
        }
    }
    if problems.is_empty() {
        Ok(chords)
    } else {
        Err(problems)
    }
}

/// One context's bindings with `wanted` in place of the defaults of the
/// actions they name. An entry that clashes is reported and left out, its
/// action back on the defaults; if even that clashes with what the user
/// asked for, the whole context goes back to its defaults.
fn rebuild(
    defaults: &Keymap,
    context: Context,
    wanted: &[&Accepted],
    problems: &mut Vec<KeyProblem>,
) -> Keymap {
    let replaced = |action: Action| wanted.iter().any(|w| w.action == action);
    let mut target = Keymap::new();
    for b in defaults.bindings(context) {
        if !replaced(b.action) {
            let _ = target.bind(context, b.chord.clone(), b.action);
        }
    }
    let mut left_out: Vec<&Accepted> = Vec::new();
    for w in wanted {
        let mut trial = target.clone();
        match trial.bind_all(context, w.chords.clone(), w.action) {
            Ok(()) => target = trial,
            Err(e) => {
                problems.push(clash(context, w.action, e));
                left_out.push(w);
            }
        }
    }
    let mut kept = true;
    for w in left_out {
        for chord in defaults.chords_of(context, w.action) {
            kept &= target.bind(context, chord.clone(), w.action).is_ok();
        }
    }
    if kept {
        target
    } else {
        problems.push(KeyProblem::Reverted { context });
        let mut original = Keymap::new();
        for b in defaults.bindings(context) {
            let _ = original.bind(context, b.chord.clone(), b.action);
        }
        original
    }
}

fn clash(context: Context, action: Action, error: BindError) -> KeyProblem {
    match error {
        BindError::Conflict {
            chord, existing, ..
        } => KeyProblem::Conflict {
            context,
            action,
            chord,
            existing,
        },
        BindError::Prefix { chord, other, .. } => KeyProblem::Prefix {
            context,
            action,
            chord,
            other,
        },
        BindError::Reserved(key) => KeyProblem::Reserved {
            context,
            action,
            key,
        },
    }
}

/// `keymap` with the bindings of `context` taken from `with`.
fn replace_context(keymap: &Keymap, context: Context, with: Keymap) -> Keymap {
    let mut out = Keymap::new();
    for c in keymap.contexts() {
        let source = if c == context { &with } else { keymap };
        for b in source.bindings(c) {
            let _ = out.bind(c, b.chord.clone(), b.action);
        }
    }
    out
}

/// Overlaps `keymap` has, over the screen states in `stacks` (each the
/// contexts active at once, innermost first), that `defaults` does not.
/// Legal, but the user may not have meant them: a key now does something
/// else than what the context further out says.
pub fn new_shadows(defaults: &Keymap, keymap: &Keymap, stacks: &[Vec<Context>]) -> Vec<Shadow> {
    let old: Vec<Shadow> = stacks.iter().flat_map(|s| defaults.shadows(s)).collect();
    let mut out: Vec<Shadow> = Vec::new();
    for stack in stacks {
        for s in keymap.shadows(stack) {
            if !old.contains(&s) && !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}
