//! `swarmux theme`: list the themes and export one for other tools.

use anyhow::{Context, anyhow};
use swarmux_application::PaletteSource;
use swarmux_infrastructure::{FilePalette, Paths, load_settings};
use swarmux_tui::theme::{THEMES, Theme};
use swarmux_tui::theme_export::{ExportFormat, export};

/// The theme called `name` (default: the one in the config), whether it is
/// built in, defined in `[themes.<name>]`, or imported from a palette file.
pub fn find(name: Option<&str>) -> anyhow::Result<Theme> {
    let paths = Paths::default_home()?;
    let settings = load_settings(&paths.config)?;
    let want = name.unwrap_or(&settings.ui.theme);
    if let Some(source) = FilePalette::from_spec(want, &paths.home) {
        let palette = source.load().map_err(|e| anyhow!(e))?;
        return Ok(Theme::from_base16(&palette));
    }
    Theme::resolve(want, &settings.ui.themes).with_context(|| {
        let names: Vec<_> = THEMES.iter().map(|t| t.name).collect();
        format!(
            "no theme called \"{want}\" (built in: {})",
            names.join(", ")
        )
    })
}

/// Prints `name` in `format`.
pub fn export_cmd(name: Option<&str>, format: &str) -> anyhow::Result<()> {
    let format = ExportFormat::parse(format).with_context(|| {
        format!("unknown format \"{format}\": toml, base16, kitty, alacritty or wezterm")
    })?;
    print!("{}", export(&find(name)?, format));
    Ok(())
}

/// Prints the built-in themes and the user's own.
pub fn list() -> anyhow::Result<()> {
    let paths = Paths::default_home()?;
    let settings = load_settings(&paths.config)?;
    for t in THEMES {
        let alias = if t.name == "reference" {
            " (alias oled)"
        } else {
            ""
        };
        println!("{}{alias}", t.name);
    }
    for def in &settings.ui.themes {
        println!("{} (yours)", def.name);
    }
    println!("wal, base16:<file>, matugen:<file>  (imported, reloaded when the file changes)");
    Ok(())
}
