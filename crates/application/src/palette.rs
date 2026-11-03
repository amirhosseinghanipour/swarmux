//! Colour data that crosses the layers: palettes read from other tools'
//! files, and user-defined themes. Plain data; the adapters parse the files
//! and the interface turns the data into a theme.

use std::collections::BTreeMap;

/// A base16 palette: sixteen `0xRRGGBB` slots, `base00` to `base0F`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base16 {
    /// Where it came from: `wal`, `base16` or `matugen`.
    pub origin: &'static str,
    /// The scheme's own name, when the file has one.
    pub name: String,
    pub slots: [u32; 16],
}

/// A file that holds a palette and may change while swarmux runs.
pub trait PaletteSource {
    /// Reads the palette now.
    fn load(&self) -> Result<Base16, String>;
    /// A value that changes when the file does (`None`: it can't be seen).
    fn stamp(&self) -> Option<u64>;
    /// What the source is, for messages.
    fn describe(&self) -> String;
}

/// A user-defined theme: colours by role, over an optional built-in base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDef {
    pub name: String,
    /// The built-in theme the unset roles come from (default `reference`).
    pub base: Option<String>,
    /// Role name to colours (`0xRRGGBB`): one, or three for `gauge`/`chart`.
    pub roles: BTreeMap<String, Vec<u32>>,
}

/// Every role a theme definition may set, and how many colours it takes.
pub const THEME_ROLES: [(&str, usize); 19] = [
    ("bg", 1),
    ("panel_bg", 1),
    ("border", 1),
    ("border_focus", 1),
    ("title", 1),
    ("text", 1),
    ("muted", 1),
    ("label", 1),
    ("dim", 1),
    ("lead", 1),
    ("builder", 1),
    ("haiku", 1),
    ("guard", 1),
    ("reviewer", 1),
    ("ok", 1),
    ("warn", 1),
    ("error", 1),
    ("on_accent", 1),
    ("gauge", 3),
];

/// `chart` is the twentieth role; kept apart so the table above stays flat.
pub const CHART_ROLE: (&str, usize) = ("chart", 3);

/// The arity of `role`, or `None` when it is not a role.
pub fn role_arity(role: &str) -> Option<usize> {
    THEME_ROLES
        .iter()
        .chain(std::iter::once(&CHART_ROLE))
        .find(|(name, _)| *name == role)
        .map(|(_, n)| *n)
}

/// `#rrggbb`, `rrggbb` or `0xrrggbb` as `0xRRGGBB`.
pub fn parse_hex(text: &str) -> Option<u32> {
    let t = text.trim();
    let t = t
        .strip_prefix('#')
        .or_else(|| t.strip_prefix("0x"))
        .unwrap_or(t);
    if t.len() == 6 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        u32::from_str_radix(t, 16).ok()
    } else {
        None
    }
}

/// `0xRRGGBB` as `#rrggbb`.
pub fn hex(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0x00ff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms() {
        assert_eq!(parse_hex("#1e1e2e"), Some(0x1e1e2e));
        assert_eq!(parse_hex(" 1E1E2E "), Some(0x1e1e2e));
        assert_eq!(parse_hex("0xff0000"), Some(0xff0000));
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
        assert_eq!(hex(0x0b0e14), "#0b0e14");
    }

    #[test]
    fn roles_and_arity() {
        assert_eq!(role_arity("bg"), Some(1));
        assert_eq!(role_arity("chart"), Some(3));
        assert_eq!(role_arity("gauge"), Some(3));
        assert_eq!(role_arity("nope"), None);
    }
}
