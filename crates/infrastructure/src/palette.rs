//! Palettes other tools write: pywal's `colors.json`, base16 YAML schemes
//! and matugen's JSON. Each is read into a [`Base16`]; the interface maps
//! that onto its colour roles.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use swarmux_application::{Base16, PaletteSource, parse_hex};

/// The file behind `theme = "wal"`, `"base16:<file>"` or `"matugen:<file>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePalette {
    kind: Kind,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Wal,
    Base16,
    Matugen,
}

impl FilePalette {
    /// The source a theme setting names, or `None` when it names a built-in
    /// theme. `~` in a path means `home`.
    pub fn from_spec(spec: &str, home: &Path) -> Option<FilePalette> {
        let spec = spec.trim();
        let lower = spec.to_ascii_lowercase();
        if lower == "wal" || lower == "pywal" {
            return Some(FilePalette {
                kind: Kind::Wal,
                path: home.join(".cache").join("wal").join("colors.json"),
            });
        }
        let (kind, rest) = if let Some(rest) = strip_prefix_ci(spec, "base16:") {
            (Kind::Base16, rest)
        } else if let Some(rest) = strip_prefix_ci(spec, "matugen:") {
            (Kind::Matugen, rest)
        } else {
            (Kind::Wal, strip_prefix_ci(spec, "wal:")?)
        };
        let rest = rest.trim();
        let path = match rest.strip_prefix("~/").or_else(|| rest.strip_prefix("~\\")) {
            Some(tail) => home.join(tail),
            None => PathBuf::from(rest),
        };
        Some(FilePalette { kind, path })
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

impl PaletteSource for FilePalette {
    fn load(&self) -> Result<Base16, String> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let parsed = match self.kind {
            Kind::Wal => parse_wal(&text),
            Kind::Base16 => parse_base16(&text, &self.path),
            Kind::Matugen => parse_matugen(&text),
        };
        parsed.map_err(|e| format!("{}: {e}", self.path.display()))
    }

    fn stamp(&self) -> Option<u64> {
        let meta = std::fs::metadata(&self.path).ok()?;
        let nanos = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some((nanos as u64) ^ meta.len().rotate_left(48))
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }
}

fn mix(a: u32, b: u32, t: f64) -> u32 {
    let ch = |shift: u32| {
        let x = f64::from(((a >> shift) & 0xff) as u8);
        let y = f64::from(((b >> shift) & 0xff) as u8);
        ((x + (y - x) * t).round() as u32) & 0xff
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn json_hex(value: Option<&serde_json::Value>) -> Option<u32> {
    parse_hex(value?.as_str()?)
}

/// pywal's `colors.json`: `special.background/foreground` and `colors.color0-15`.
pub fn parse_wal(text: &str) -> Result<Base16, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let color = |i: usize| json_hex(json.pointer(&format!("/colors/color{i}")));
    let c = |i: usize| color(i).ok_or_else(|| format!("colors.color{i} is missing"));
    let bg = match json_hex(json.pointer("/special/background")) {
        Some(v) => v,
        None => c(0)?,
    };
    let fg = match json_hex(json.pointer("/special/foreground")) {
        Some(v) => v,
        None => c(7)?,
    };
    let (red, green, yellow, blue, magenta, cyan) = (c(1)?, c(2)?, c(3)?, c(4)?, c(5)?, c(6)?);
    let grey = color(8).unwrap_or_else(|| mix(bg, fg, 0.4));
    let slots = [
        bg,
        mix(bg, fg, 0.08),
        mix(bg, fg, 0.18),
        grey,
        mix(grey, fg, 0.5),
        fg,
        mix(fg, 0xffffff, 0.3),
        mix(fg, 0xffffff, 0.5),
        red,
        mix(red, yellow, 0.5),
        yellow,
        green,
        cyan,
        blue,
        magenta,
        mix(red, magenta, 0.5),
    ];
    Ok(Base16 {
        origin: "wal",
        name: "wal".into(),
        slots,
    })
}

/// A base16 scheme in YAML (the flat form and the `palette:` form).
pub fn parse_base16(text: &str, path: &Path) -> Result<Base16, String> {
    let mut slots = [None; 16];
    let mut name = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
        let value = value.trim();
        let slot = key
            .strip_prefix("base0")
            .filter(|rest| rest.len() == 1)
            .and_then(|rest| usize::from_str_radix(rest, 16).ok());
        if let Some(i) = slot {
            let digits: String = value
                .trim_start_matches(['"', '\'', '#'])
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .collect();
            slots[i] = parse_hex(&digits);
        } else if (key == "scheme" || key == "name") && name.is_none() {
            let v = value.trim_matches(['"', '\'']).trim();
            if !v.is_empty() {
                name = Some(v.to_string());
            }
        }
    }
    let mut out = [0u32; 16];
    for (i, slot) in slots.iter().enumerate() {
        out[i] = slot.ok_or_else(|| format!("base0{i:X} is missing"))?;
    }
    let name = name.unwrap_or_else(|| {
        path.file_stem()
            .map_or_else(|| "base16".into(), |s| s.to_string_lossy().into_owned())
    });
    Ok(Base16 {
        origin: "base16",
        name,
        slots: out,
    })
}

/// matugen's JSON: `colors.dark.<role>` (or `colors.<role>`) as `#rrggbb`.
pub fn parse_matugen(text: &str) -> Result<Base16, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let colors = json.get("colors").ok_or("no \"colors\" object")?;
    let scheme = colors
        .get("dark")
        .filter(|d| d.is_object())
        .unwrap_or(colors);
    let get = |key: &str| {
        json_hex(scheme.get(key)).or_else(|| json_hex(colors.get(key).and_then(|v| v.get("dark"))))
    };
    let need = |key: &str| get(key).ok_or_else(|| format!("colors.dark.{key} is missing"));
    let bg = need("surface")?;
    let fg = need("on_surface")?;
    let primary = need("primary")?;
    let secondary = get("secondary").unwrap_or(primary);
    let tertiary = get("tertiary").unwrap_or(primary);
    let error = get("error").unwrap_or(0xf2b8b5);
    let outline = get("outline").unwrap_or_else(|| mix(bg, fg, 0.4));
    let variant = get("on_surface_variant").unwrap_or_else(|| mix(fg, bg, 0.3));
    // Material has no green, yellow or cyan: pull fixed hues toward the
    // wallpaper's primary, as Material's own harmonisation does.
    let green = mix(0x8fd49a, primary, 0.2);
    let yellow = mix(0xe6c97a, primary, 0.2);
    let cyan = mix(0x7fd1d6, primary, 0.2);
    let slots = [
        bg,
        get("surface_container").unwrap_or_else(|| mix(bg, fg, 0.06)),
        get("surface_container_high").unwrap_or_else(|| mix(bg, fg, 0.12)),
        outline,
        variant,
        fg,
        mix(fg, 0xffffff, 0.2),
        mix(fg, 0xffffff, 0.4),
        error,
        tertiary,
        yellow,
        green,
        cyan,
        primary,
        secondary,
        mix(error, tertiary, 0.5),
    ];
    Ok(Base16 {
        origin: "matugen",
        name: "matugen".into(),
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAL: &str = r##"{"wallpaper":"x","special":{"background":"#0f1115","foreground":"#d0d4dc","cursor":"#d0d4dc"},
      "colors":{"color0":"#0f1115","color1":"#e06c75","color2":"#98c379","color3":"#e5c07b","color4":"#61afef","color5":"#c678dd","color6":"#56b6c2","color7":"#d0d4dc","color8":"#5c6370","color9":"#e06c75","color10":"#98c379","color11":"#e5c07b","color12":"#61afef","color13":"#c678dd","color14":"#56b6c2","color15":"#ffffff"}}"##;

    const SCHEME: &str = "scheme: \"Test Scheme\"\nauthor: me\nbase00: \"1e1e2e\"\nbase01: \"#181825\"\nbase02: '313244'\nbase03: \"45475a\" # comment\nbase04: \"585b70\"\nbase05: \"cdd6f4\"\nbase06: \"f5e0dc\"\nbase07: \"b4befe\"\nbase08: \"f38ba8\"\nbase09: \"fab387\"\nbase0A: \"f9e2af\"\nbase0B: \"a6e3a1\"\nbase0C: \"94e2d5\"\nbase0D: \"89b4fa\"\nbase0E: \"cba6f7\"\nbase0F: \"f2cdcd\"\n";

    #[test]
    fn wal_colours_land_in_the_standard_slots() {
        let p = parse_wal(WAL).unwrap();
        assert_eq!(p.origin, "wal");
        assert_eq!(p.slots[0], 0x0f1115);
        assert_eq!(p.slots[5], 0xd0d4dc);
        assert_eq!(p.slots[8], 0xe06c75, "red");
        assert_eq!(p.slots[0xB], 0x98c379, "green");
        assert_eq!(p.slots[0xD], 0x61afef, "blue");
        assert!(parse_wal("{}").is_err());
        assert!(parse_wal("nope").is_err());
    }

    #[test]
    fn base16_yaml_flat_and_nested() {
        let p = parse_base16(SCHEME, Path::new("x.yaml")).unwrap();
        assert_eq!(p.name, "Test Scheme");
        assert_eq!(p.slots[0], 0x1e1e2e);
        assert_eq!(p.slots[1], 0x181825);
        assert_eq!(p.slots[2], 0x313244);
        assert_eq!(p.slots[3], 0x45475a);
        assert_eq!(p.slots[15], 0xf2cdcd);
        let nested = format!(
            "system: base16\nname: Nested\npalette:\n{}",
            SCHEME
                .lines()
                .filter(|l| l.starts_with("base"))
                .map(|l| format!("  {l}\n"))
                .collect::<String>()
        );
        let p2 = parse_base16(&nested, Path::new("mine.yaml")).unwrap();
        assert_eq!(p2.slots, p.slots);
        assert_eq!(p2.name, "Nested");
        assert!(parse_base16("base00: \"000000\"", Path::new("x.yaml")).is_err());
    }

    #[test]
    fn matugen_dark_scheme() {
        let json = r##"{"colors":{"dark":{"surface":"#10141a","on_surface":"#dfe2eb","primary":"#a2c9fe","secondary":"#bbc7db","tertiary":"#d6bee4","error":"#ffb4ab","outline":"#8b919d","on_surface_variant":"#c1c7d0","surface_container":"#1c2026"},"light":{"surface":"#f7f9ff"}}}"##;
        let p = parse_matugen(json).unwrap();
        assert_eq!(p.slots[0], 0x10141a);
        assert_eq!(p.slots[5], 0xdfe2eb);
        assert_eq!(p.slots[0xD], 0xa2c9fe);
        assert_eq!(p.slots[8], 0xffb4ab);
        assert_eq!(p.slots[1], 0x1c2026);
        assert!(parse_matugen("{\"colors\":{}}").is_err());
    }

    #[test]
    fn specs_name_their_files() {
        let home = Path::new("/home/u");
        let wal = FilePalette::from_spec("wal", home).unwrap();
        assert_eq!(wal.path, home.join(".cache/wal/colors.json"));
        let b = FilePalette::from_spec("base16:~/schemes/x.yaml", home).unwrap();
        assert_eq!(b.path, home.join("schemes/x.yaml"));
        assert_eq!(b.kind, Kind::Base16);
        let m = FilePalette::from_spec("Matugen:/tmp/m.json", home).unwrap();
        assert_eq!(m.kind, Kind::Matugen);
        assert!(FilePalette::from_spec("nord", home).is_none());
    }

    #[test]
    fn a_changed_file_changes_the_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("colors.json");
        std::fs::write(&path, WAL).unwrap();
        let src = FilePalette {
            kind: Kind::Wal,
            path: path.clone(),
        };
        let first = src.stamp().unwrap();
        assert_eq!(src.load().unwrap().slots[0], 0x0f1115);
        std::fs::write(&path, WAL.replace("#0f1115", "#101216 ")).unwrap();
        assert_ne!(src.stamp().unwrap(), first, "length changed at least");
        assert_eq!(src.load().unwrap().slots[0], 0x101216);
        std::fs::remove_file(&path).unwrap();
        assert!(src.stamp().is_none());
        assert!(src.load().is_err());
    }
}
