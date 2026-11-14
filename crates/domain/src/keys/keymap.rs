//! Named actions, the keymap that binds keys to them, and the resolver that
//! turns a stream of key presses (chords included) into actions.
//!
//! A binding lives in a *context*: a screen, a pane, an overlay, or the
//! global one. The caller says which contexts are active, innermost first;
//! the first that knows the key wins, and an action that does not apply
//! right now lets the next candidate try.

use std::collections::BTreeMap;

use super::spec::{Chord, KeySpec, is_reserved};
use crate::Millis;

/// A named thing a key can do (`nav.go_work`, `list.next`).
///
/// Ids are written `group.name`, or a bare word for the few that belong to
/// no group (`back`, `quit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Action(&'static str);

impl Action {
    pub const fn new(id: &'static str) -> Action {
        Action(id)
    }

    pub const fn id(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Where a binding applies: one screen, one pane, an overlay, or everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Context(&'static str);

impl Context {
    pub const fn new(id: &'static str) -> Context {
        Context(id)
    }

    pub const fn id(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// The context whose bindings apply on every screen, after the screen's own.
pub const GLOBAL: Context = Context::new("global");

/// The actions every screen shares, and the navigation vocabulary.
pub mod actions {
    use super::Action;

    /// `Esc`: dismiss what is on top, else go back one screen. Reserved.
    pub const BACK: Action = Action::new("back");
    /// `Ctrl+C` (and `q` by default): leave. Reserved on `Ctrl+C`.
    pub const QUIT: Action = Action::new("quit");
    pub const LIST_PREV: Action = Action::new("list.prev");
    pub const LIST_NEXT: Action = Action::new("list.next");
    /// Open the selected row.
    pub const LIST_OPEN: Action = Action::new("list.open");

    pub const GO_MISSION: Action = Action::new("nav.go_mission");
    pub const GO_MISSIONS: Action = Action::new("nav.go_missions");
    pub const GO_PROMPTS: Action = Action::new("nav.go_prompts");
    pub const GO_QUEUE: Action = Action::new("nav.go_queue");
    pub const GO_LEDGER: Action = Action::new("nav.go_ledger");
    pub const GO_SESSIONS: Action = Action::new("nav.go_sessions");
    pub const GO_FOCUS: Action = Action::new("nav.go_focus");
    pub const GO_BOARD: Action = Action::new("nav.go_board");
    pub const GO_KNOWLEDGE: Action = Action::new("nav.go_knowledge");
    pub const GO_FOLLOWUPS: Action = Action::new("nav.go_followups");

    /// Screens that are planned but not built yet. Each stands in for the
    /// nearest screen that exists (or does nothing); the app says which.
    pub const GO_DECK: Action = Action::new("nav.go_deck");
    pub const GO_WORK: Action = Action::new("nav.go_work");
    pub const GO_BRIEF: Action = Action::new("nav.go_brief");
    pub const GO_RECORD: Action = Action::new("nav.go_record");
}

/// One key sequence and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub chord: Chord,
    pub action: Action,
}

/// Why a binding was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    #[error("`{0}` is reserved and can't be bound")]
    Reserved(KeySpec),
    #[error("`{chord}` is already `{existing}` in {context}; it can't also be `{new}`")]
    Conflict {
        context: Context,
        chord: Chord,
        existing: Action,
        new: Action,
    },
    #[error("`{chord}` and `{other}` can't both be bound in {context}: one starts the other")]
    Prefix {
        context: Context,
        chord: Chord,
        other: Chord,
    },
}

/// What the keymap makes of a key sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// These actions, the innermost context's first: the first that applies
    /// is run.
    Actions(Vec<Action>),
    /// The start of a longer chord; wait for the next key.
    Pending,
    /// Nothing is bound to it.
    None,
}

/// A binding that hides another because its context is searched first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shadow {
    pub winner: (Context, Action, Chord),
    pub hidden: (Context, Action, Chord),
}

impl std::fmt::Display for Shadow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ((wc, wa, wk), (hc, ha, hk)) = (&self.winner, &self.hidden);
        write!(
            f,
            "`{wk}` is {wa} in {wc}, which comes first and hides {ha} (`{hk}` in {hc})"
        )
    }
}

/// Keys bound to actions, by context. Pure data: nothing here knows a
/// terminal or a screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keymap {
    contexts: BTreeMap<Context, Vec<Binding>>,
}

impl Keymap {
    pub fn new() -> Keymap {
        Keymap::default()
    }

    /// Binds `chord` to `action` in `context`. Binding the same chord to
    /// the same action again changes nothing.
    pub fn bind(
        &mut self,
        context: Context,
        chord: Chord,
        action: Action,
    ) -> Result<(), BindError> {
        if let Some(key) = chord.keys().iter().copied().find(|k| is_reserved(*k)) {
            return Err(BindError::Reserved(key));
        }
        let bindings = self.contexts.entry(context).or_default();
        for b in bindings.iter() {
            if b.chord == chord {
                if b.action == action {
                    return Ok(());
                }
                return Err(BindError::Conflict {
                    context,
                    chord,
                    existing: b.action,
                    new: action,
                });
            }
            let nested = (b.chord.len() < chord.len() && chord.starts_with(b.chord.keys()))
                || (chord.len() < b.chord.len() && b.chord.starts_with(chord.keys()));
            if nested {
                return Err(BindError::Prefix {
                    context,
                    chord,
                    other: b.chord.clone(),
                });
            }
        }
        bindings.push(Binding { chord, action });
        Ok(())
    }

    /// Binds every chord of `chords` to `action`.
    pub fn bind_all(
        &mut self,
        context: Context,
        chords: impl IntoIterator<Item = Chord>,
        action: Action,
    ) -> Result<(), BindError> {
        for chord in chords {
            self.bind(context, chord, action)?;
        }
        Ok(())
    }

    /// Drops every binding of `action` in `context`.
    pub fn unbind(&mut self, context: Context, action: Action) {
        if let Some(bindings) = self.contexts.get_mut(&context) {
            bindings.retain(|b| b.action != action);
        }
    }

    /// Replaces the keys of `action` in `context` with `chords`. If any is
    /// refused the keymap stays as it was.
    pub fn rebind(
        &mut self,
        context: Context,
        action: Action,
        chords: Vec<Chord>,
    ) -> Result<(), BindError> {
        let mut next = self.clone();
        next.unbind(context, action);
        next.bind_all(context, chords, action)?;
        *self = next;
        Ok(())
    }

    /// The contexts that have bindings.
    pub fn contexts(&self) -> impl Iterator<Item = Context> + '_ {
        self.contexts.keys().copied()
    }

    /// The bindings of one context, in the order they were made.
    pub fn bindings(&self, context: Context) -> &[Binding] {
        self.contexts.get(&context).map_or(&[], Vec::as_slice)
    }

    /// The chords bound to `action` in `context`.
    pub fn chords_of(&self, context: Context, action: Action) -> impl Iterator<Item = &Chord> {
        self.bindings(context)
            .iter()
            .filter(move |b| b.action == action)
            .map(|b| &b.chord)
    }

    /// How `action`'s first key is written, for hints and tabs.
    pub fn label_of(&self, context: Context, action: Action) -> Option<String> {
        self.chords_of(context, action).next().map(Chord::to_string)
    }

    /// What `seq` means with `stack` active, innermost context first.
    ///
    /// The first context that has the sequence (or starts a longer chord
    /// with it) decides: a context that only starts a chord makes the
    /// sequence pending, so nothing further out can answer it.
    pub fn resolve(&self, stack: &[Context], seq: &[KeySpec]) -> Lookup {
        let mut found: Vec<Action> = Vec::new();
        for context in stack {
            let mut starts_chord = false;
            for b in self.bindings(*context) {
                if b.chord.keys() == seq {
                    if !found.contains(&b.action) {
                        found.push(b.action);
                    }
                } else if b.chord.len() > seq.len() && b.chord.starts_with(seq) {
                    starts_chord = true;
                }
            }
            if starts_chord && found.is_empty() {
                return Lookup::Pending;
            }
        }
        if found.is_empty() {
            Lookup::None
        } else {
            Lookup::Actions(found)
        }
    }

    /// Bindings of an outer context that an inner one in `stack` covers
    /// (the same keys, or one starting the other), innermost first.
    pub fn shadows(&self, stack: &[Context]) -> Vec<Shadow> {
        let mut out = Vec::new();
        for (i, inner) in stack.iter().enumerate() {
            for outer in &stack[i + 1..] {
                for a in self.bindings(*inner) {
                    for b in self.bindings(*outer) {
                        let overlap = a.chord.starts_with(b.chord.keys())
                            || b.chord.starts_with(a.chord.keys());
                        if overlap && a.action != b.action {
                            out.push(Shadow {
                                winner: (*inner, a.action, a.chord.clone()),
                                hidden: (*outer, b.action, b.chord.clone()),
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Bindings to actions the catalog does not declare in that context.
    pub fn unknown(&self, catalog: &Catalog) -> Vec<(Context, Action)> {
        let mut out = Vec::new();
        for (context, bindings) in &self.contexts {
            for b in bindings {
                if !catalog.declares(*context, b.action) && !out.contains(&(*context, b.action)) {
                    out.push((*context, b.action));
                }
            }
        }
        out
    }
}

/// One action a context offers, with what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionDef {
    pub context: Context,
    pub action: Action,
    pub help: &'static str,
    /// The user can't rebind it: its keys are fixed.
    pub locked: bool,
}

/// Every action that can be bound, by context: what a config's `[keys]`
/// is checked against, and what a help overlay lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    defs: Vec<ActionDef>,
}

impl Catalog {
    pub fn new() -> Catalog {
        Catalog::default()
    }

    pub fn add(&mut self, context: Context, action: Action, help: &'static str) {
        if !self.declares(context, action) {
            self.defs.push(ActionDef {
                context,
                action,
                help,
                locked: false,
            });
        }
    }

    /// Makes `action` in `context` something a config can't rebind.
    pub fn lock(&mut self, context: Context, action: Action) {
        if let Some(d) = self
            .defs
            .iter_mut()
            .find(|d| d.context == context && d.action == action)
        {
            d.locked = true;
        }
    }

    pub fn is_locked(&self, context: Context, action: Action) -> bool {
        self.defs
            .iter()
            .any(|d| d.context == context && d.action == action && d.locked)
    }

    /// The context called `id`, as a config names it.
    pub fn context(&self, id: &str) -> Option<Context> {
        self.defs.iter().map(|d| d.context).find(|c| c.id() == id)
    }

    /// Every context that has actions, in the order they were declared.
    pub fn contexts(&self) -> Vec<Context> {
        let mut out: Vec<Context> = Vec::new();
        for d in &self.defs {
            if !out.contains(&d.context) {
                out.push(d.context);
            }
        }
        out
    }

    pub fn declares(&self, context: Context, action: Action) -> bool {
        self.defs
            .iter()
            .any(|d| d.context == context && d.action == action)
    }

    /// The action called `id` in `context`, as a config names it.
    pub fn find(&self, context: &str, id: &str) -> Option<Action> {
        self.defs
            .iter()
            .find(|d| d.context.id() == context && d.action.id() == id)
            .map(|d| d.action)
    }

    pub fn defs(&self) -> &[ActionDef] {
        &self.defs
    }

    pub fn of(&self, context: Context) -> impl Iterator<Item = &ActionDef> {
        self.defs.iter().filter(move |d| d.context == context)
    }
}

/// How long a half-typed chord waits for its next key.
pub const CHORD_TIMEOUT_MS: Millis = 2_000;

/// What one key press came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Run the first of these that applies.
    Actions(Vec<Action>),
    /// The first key of a chord: more to come.
    Pending,
    /// A chord that went nowhere (or was dropped with `Esc`); the keys.
    Cancelled(Vec<KeySpec>),
    /// Nothing is bound to it.
    Unbound,
}

/// Turns key presses into actions, remembering a half-typed chord.
///
/// `Esc` and `Ctrl+C` are not in any keymap: `Ctrl+C` is always
/// [`actions::QUIT`], `Esc` is [`actions::BACK`] (or, with a chord half
/// typed, drops the chord).
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    pending: Vec<KeySpec>,
    since: Millis,
}

impl Resolver {
    pub fn new() -> Resolver {
        Resolver::default()
    }

    /// The keys of the chord being typed.
    pub fn pending(&self) -> &[KeySpec] {
        &self.pending
    }

    /// Forgets a half-typed chord.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Forgets a half-typed chord that has waited too long.
    pub fn expire(&mut self, now: Millis) {
        if !self.pending.is_empty() && now - self.since > CHORD_TIMEOUT_MS {
            self.pending.clear();
        }
    }

    /// Takes the next key press, `now` being the time it came.
    pub fn feed(
        &mut self,
        keymap: &Keymap,
        stack: &[Context],
        key: KeySpec,
        now: Millis,
    ) -> Outcome {
        self.expire(now);
        if key == KeySpec::ctrl('c') {
            self.pending.clear();
            return Outcome::Actions(vec![actions::QUIT]);
        }
        if is_reserved(key) {
            if self.pending.is_empty() {
                return Outcome::Actions(vec![actions::BACK]);
            }
            let mut keys = std::mem::take(&mut self.pending);
            keys.push(key);
            return Outcome::Cancelled(keys);
        }
        let had_pending = !self.pending.is_empty();
        let mut seq = std::mem::take(&mut self.pending);
        seq.push(key);
        match keymap.resolve(stack, &seq) {
            Lookup::Actions(actions) => Outcome::Actions(actions),
            Lookup::Pending => {
                self.pending = seq;
                self.since = now;
                Outcome::Pending
            }
            Lookup::None if had_pending => Outcome::Cancelled(seq),
            Lookup::None => Outcome::Unbound,
        }
    }
}
