//! Keys as data: what a key press is, independent of any terminal library,
//! and how it is written in a config file (`q`, `ctrl+s`, `g w`).

use std::fmt;

/// Longest chord: keys pressed one after the other for one action.
pub const MAX_CHORD_LEN: usize = 3;

/// Why a key or chord could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("no key given")]
    Empty,
    #[error("unknown key `{0}`")]
    Unknown(String),
    #[error("function keys run from f1 to f24, not `{0}`")]
    FunctionKey(String),
    #[error("a chord has at most {MAX_CHORD_LEN} keys, `{0}` has more")]
    TooLong(String),
}

/// A key without its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// A character; its case is part of it (`D` is shift+d).
    Char(char),
    Enter,
    Esc,
    Tab,
    /// Shift+Tab. Terminals report it as a key of its own.
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    /// A function key, 1 to 24.
    F(u8),
}

/// The modifier keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);

    /// Every modifier of `other` is held (and `other` names at least one).
    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;

    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

/// One key press: a key and the modifiers held with it.
///
/// Always in its canonical form, so two presses that mean the same compare
/// equal: shift is part of a character (`Shift+d` is `D`) and of `BackTab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeySpec {
    key: Key,
    mods: Mods,
}

impl KeySpec {
    /// The canonical form of `key` pressed with `mods`.
    pub fn new(key: Key, mods: Mods) -> KeySpec {
        let (key, mods) = match key {
            Key::Char(c) if mods.contains(Mods::SHIFT) => {
                (Key::Char(upper(c)), mods.without(Mods::SHIFT))
            }
            Key::Tab if mods.contains(Mods::SHIFT) => (Key::BackTab, mods.without(Mods::SHIFT)),
            Key::BackTab => (Key::BackTab, mods.without(Mods::SHIFT)),
            other => (other, mods),
        };
        KeySpec { key, mods }
    }

    /// A key with no modifiers.
    pub fn plain(key: Key) -> KeySpec {
        KeySpec::new(key, Mods::NONE)
    }

    /// A character with no modifiers.
    pub fn char(c: char) -> KeySpec {
        KeySpec::plain(Key::Char(c))
    }

    /// Ctrl and a character.
    pub fn ctrl(c: char) -> KeySpec {
        KeySpec::new(Key::Char(c), Mods::CTRL)
    }

    pub fn key(&self) -> Key {
        self.key
    }

    pub fn mods(&self) -> Mods {
        self.mods
    }

    /// Reads `q`, `D`, `ctrl+s`, `alt+x`, `shift+tab`, `enter`, `space`, `f5`.
    pub fn parse(text: &str) -> Result<KeySpec, KeyError> {
        let mut rest = text.trim();
        if rest.is_empty() {
            return Err(KeyError::Empty);
        }
        let mut mods = Mods::NONE;
        loop {
            let lower = rest.to_ascii_lowercase();
            let (m, len) = if lower.starts_with("ctrl+") {
                (Mods::CTRL, 5)
            } else if lower.starts_with("alt+") {
                (Mods::ALT, 4)
            } else if lower.starts_with("shift+") {
                (Mods::SHIFT, 6)
            } else {
                break;
            };
            if rest.len() == len {
                // `ctrl+` with nothing after it.
                return Err(KeyError::Unknown(text.trim().to_string()));
            }
            mods = mods | m;
            rest = &rest[len..];
        }
        let mut chars = rest.chars();
        let key = match (chars.next(), chars.next()) {
            (Some(c), None) => Key::Char(c),
            _ => named(rest)?,
        };
        Ok(KeySpec::new(key, mods))
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(Mods::CTRL) {
            f.write_str("ctrl+")?;
        }
        if self.mods.contains(Mods::ALT) {
            f.write_str("alt+")?;
        }
        if self.mods.contains(Mods::SHIFT) {
            // Only keys other than characters and tab keep shift.
            f.write_str("shift+")?;
        }
        match self.key {
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::Enter => f.write_str("enter"),
            Key::Esc => f.write_str("esc"),
            Key::Tab => f.write_str("tab"),
            Key::BackTab => f.write_str("shift+tab"),
            Key::Backspace => f.write_str("backspace"),
            Key::Up => f.write_str("up"),
            Key::Down => f.write_str("down"),
            Key::Left => f.write_str("left"),
            Key::Right => f.write_str("right"),
            Key::Home => f.write_str("home"),
            Key::End => f.write_str("end"),
            Key::PageUp => f.write_str("pageup"),
            Key::PageDown => f.write_str("pagedown"),
            Key::Delete => f.write_str("delete"),
            Key::Insert => f.write_str("insert"),
            Key::F(n) => write!(f, "f{n}"),
        }
    }
}

/// `c` in upper case when it has a single upper-case form, else `c`.
fn upper(c: char) -> char {
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

fn named(name: &str) -> Result<Key, KeyError> {
    let lower = name.to_ascii_lowercase();
    Ok(match lower.as_str() {
        "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Esc,
        "tab" => Key::Tab,
        "backtab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "space" => Key::Char(' '),
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        other => match other.strip_prefix('f').map(str::parse::<u8>) {
            Some(Ok(n)) if (1..=24).contains(&n) => Key::F(n),
            Some(Ok(_)) => return Err(KeyError::FunctionKey(name.to_string())),
            _ => return Err(KeyError::Unknown(name.to_string())),
        },
    })
}

/// Keys pressed one after the other: `g` then `w`. A single key is a chord
/// of one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord(Vec<KeySpec>);

impl Chord {
    /// A chord of the given keys: at least one, at most [`MAX_CHORD_LEN`].
    pub fn new(keys: Vec<KeySpec>) -> Result<Chord, KeyError> {
        if keys.is_empty() {
            return Err(KeyError::Empty);
        }
        if keys.len() > MAX_CHORD_LEN {
            let text: Vec<String> = keys.iter().map(ToString::to_string).collect();
            return Err(KeyError::TooLong(text.join(" ")));
        }
        Ok(Chord(keys))
    }

    /// A chord of one key.
    pub fn single(key: KeySpec) -> Chord {
        Chord(vec![key])
    }

    /// Reads keys separated by spaces: `g w`, `ctrl+x ctrl+s`.
    pub fn parse(text: &str) -> Result<Chord, KeyError> {
        let keys = text
            .split_whitespace()
            .map(KeySpec::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Chord::new(keys)
    }

    pub fn keys(&self) -> &[KeySpec] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Never true: a chord has at least one key.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The chord starts with `keys` (and is exactly them or longer).
    pub fn starts_with(&self, keys: &[KeySpec]) -> bool {
        self.0.starts_with(keys)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, k) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{k}")?;
        }
        Ok(())
    }
}

/// Keys that can't be rebound and never take part in a chord: `Esc` goes
/// back (and drops a half-typed chord), `Ctrl+C` quits.
pub fn is_reserved(key: KeySpec) -> bool {
    key == KeySpec::plain(Key::Esc) || key == KeySpec::ctrl('c')
}
