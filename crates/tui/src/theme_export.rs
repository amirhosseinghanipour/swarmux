//! `swarmux theme export`: a theme as text other tools read. TOML is the
//! shareable block for swarmux's own config; the rest carry the palette to
//! the terminal and to base16-aware tools.

use swarmux_application::hex;

use crate::theme::{Theme, rgb_of};

/// What to write a theme as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// A `[themes.<name>]` block for `config.toml`.
    Toml,
    /// A base16 YAML scheme.
    Base16,
    Kitty,
    Alacritty,
    Wezterm,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::Toml,
        ExportFormat::Base16,
        ExportFormat::Kitty,
        ExportFormat::Alacritty,
        ExportFormat::Wezterm,
    ];

    pub fn parse(s: &str) -> Option<ExportFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "toml" => Some(ExportFormat::Toml),
            "base16" => Some(ExportFormat::Base16),
            "kitty" => Some(ExportFormat::Kitty),
            "alacritty" => Some(ExportFormat::Alacritty),
            "wezterm" => Some(ExportFormat::Wezterm),
            _ => None,
        }
    }
}

fn h(c: ratatui::style::Color) -> String {
    hex(rgb_of(c))
}

/// `theme` written in `format`.
pub fn export(theme: &Theme, format: ExportFormat) -> String {
    match format {
        ExportFormat::Toml => toml(theme),
        ExportFormat::Base16 => base16(theme),
        ExportFormat::Kitty => kitty(theme),
        ExportFormat::Alacritty => alacritty(theme),
        ExportFormat::Wezterm => wezterm(theme),
    }
}

fn toml(t: &Theme) -> String {
    let mut out = format!(
        "# swarmux theme \"{0}\": paste into config.toml (leave out the schema_version line\n# when it has one already).\nschema_version = 1\ntheme = \"{0}\"\n\n[themes.{0}]\n",
        t.name
    );
    for (role, colours) in t.roles() {
        let rendered: Vec<String> = colours.iter().map(|c| format!("\"{}\"", hex(*c))).collect();
        match rendered.as_slice() {
            [one] => out += &format!("{role} = {one}\n"),
            many => out += &format!("{role} = [{}]\n", many.join(", ")),
        }
    }
    out
}

/// The base16 slots a theme fills: `base00` to `base0F`.
pub fn base16_slots(t: &Theme) -> [u32; 16] {
    let c = rgb_of;
    [
        c(t.bg),
        c(t.panel_bg),
        c(t.dim),
        c(t.border),
        c(t.muted),
        c(t.text),
        c(t.title),
        c(t.label),
        c(t.error),
        c(t.lead),
        c(t.builder),
        c(t.ok),
        c(t.chart[0]),
        c(t.haiku),
        c(t.reviewer),
        c(t.border_focus),
    ]
}

fn base16(t: &Theme) -> String {
    let mut out = format!(
        "system: \"base16\"\nname: \"swarmux {0}\"\nauthor: \"swarmux\"\nvariant: \"dark\"\npalette:\n",
        t.name
    );
    for (i, slot) in base16_slots(t).iter().enumerate() {
        out += &format!(
            "  base{i:02X}: \"{}\"\n",
            hex(*slot).trim_start_matches('#')
        );
    }
    out
}

/// The sixteen terminal colours: normal 0-7, then bright 8-15.
fn ansi(t: &Theme) -> [String; 16] {
    [
        h(t.dim),
        h(t.error),
        h(t.ok),
        h(t.warn),
        h(t.haiku),
        h(t.reviewer),
        h(t.chart[0]),
        h(t.label),
        h(t.muted),
        h(t.error),
        h(t.ok),
        h(t.builder),
        h(t.haiku),
        h(t.reviewer),
        h(t.chart[0]),
        h(t.title),
    ]
}

fn kitty(t: &Theme) -> String {
    let mut out = format!(
        "# swarmux {}\nforeground {}\nbackground {}\ncursor {}\nselection_foreground {}\nselection_background {}\nurl_color {}\n",
        t.name,
        h(t.text),
        h(t.bg),
        h(t.border_focus),
        h(t.bg),
        h(t.border_focus),
        h(t.haiku),
    );
    for (i, c) in ansi(t).iter().enumerate() {
        out += &format!("color{i} {c}\n");
    }
    out
}

fn alacritty(t: &Theme) -> String {
    let a = ansi(t);
    let names = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    let block = |title: &str, colours: &[String]| {
        let mut s = format!("\n[colors.{title}]\n");
        for (name, c) in names.iter().zip(colours) {
            s += &format!("{name} = \"{c}\"\n");
        }
        s
    };
    format!(
        "# swarmux {}\n[colors.primary]\nbackground = \"{}\"\nforeground = \"{}\"\n\n[colors.cursor]\ntext = \"{}\"\ncursor = \"{}\"\n\n[colors.selection]\ntext = \"{}\"\nbackground = \"{}\"\n{}{}",
        t.name,
        h(t.bg),
        h(t.text),
        h(t.bg),
        h(t.border_focus),
        h(t.bg),
        h(t.border_focus),
        block("normal", &a[..8]),
        block("bright", &a[8..]),
    )
}

fn wezterm(t: &Theme) -> String {
    let a = ansi(t);
    let list = |colours: &[String]| {
        colours
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "# swarmux {0}: save as ~/.config/wezterm/colors/swarmux-{0}.toml\n[colors]\nforeground = \"{1}\"\nbackground = \"{2}\"\ncursor_bg = \"{3}\"\ncursor_fg = \"{2}\"\ncursor_border = \"{3}\"\nselection_bg = \"{3}\"\nselection_fg = \"{2}\"\nansi = [{4}]\nbrights = [{5}]\n\n[metadata]\nname = \"swarmux {0}\"\n",
        t.name,
        h(t.text),
        h(t.bg),
        h(t.border_focus),
        list(&a[..8]),
        list(&a[8..]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{NORD, THEMES};

    #[test]
    fn formats_parse_by_name() {
        for (name, f) in [
            ("toml", ExportFormat::Toml),
            ("BASE16", ExportFormat::Base16),
            ("kitty", ExportFormat::Kitty),
            ("alacritty", ExportFormat::Alacritty),
            ("wezterm", ExportFormat::Wezterm),
        ] {
            assert_eq!(ExportFormat::parse(name), Some(f));
        }
        assert_eq!(ExportFormat::parse("css"), None);
    }

    #[test]
    fn kitty_carries_the_palette() {
        let out = export(&NORD, ExportFormat::Kitty);
        assert!(out.contains("background #2e3440"), "{out}");
        assert!(out.contains("foreground #d8dee9"));
        assert!(out.contains("color1 #bf616a"), "error is red");
        assert!(out.contains("color15 "));
        assert_eq!(out.matches("\ncolor").count(), 16);
    }

    #[test]
    fn alacritty_and_wezterm_are_valid_toml() {
        for t in THEMES {
            for f in [
                ExportFormat::Alacritty,
                ExportFormat::Wezterm,
                ExportFormat::Toml,
            ] {
                let out = export(&t, f);
                let parsed: Result<toml::Table, _> = out.parse();
                assert!(parsed.is_ok(), "{} {f:?}: {parsed:?}\n{out}", t.name);
            }
        }
        let w: toml::Table = export(&NORD, ExportFormat::Wezterm)
            .parse()
            .unwrap_or_default();
        let colors = w.get("colors").and_then(toml::Value::as_table);
        let ansi = colors
            .and_then(|c| c.get("ansi"))
            .and_then(toml::Value::as_array);
        assert_eq!(ansi.map(Vec::len), Some(8));
        let a: toml::Table = export(&NORD, ExportFormat::Alacritty)
            .parse()
            .unwrap_or_default();
        assert!(a["colors"]["bright"]["white"].is_str());
    }

    #[test]
    fn base16_names_all_sixteen_slots() {
        let out = export(&NORD, ExportFormat::Base16);
        for i in 0..16 {
            assert!(out.contains(&format!("base{i:02X}:")), "{i}\n{out}");
        }
        assert!(out.contains("base00: \"2e3440\""));
    }
}
