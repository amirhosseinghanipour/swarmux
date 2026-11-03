//! `swarmux theme export` and the config loader agree: what one prints, the
//! other reads back as the same theme.

use swarmux_application::PaletteSource;
use swarmux_infrastructure::{FilePalette, load_settings};
use swarmux_tui::theme::{THEMES, Theme};
use swarmux_tui::theme_export::{ExportFormat, export};

#[test]
fn exported_toml_loads_back_as_the_same_theme() {
    let dir = tempfile::tempdir().unwrap();
    for theme in THEMES {
        let path = dir.path().join(format!("{}.toml", theme.name));
        std::fs::write(&path, export(&theme, ExportFormat::Toml)).unwrap();
        let settings = load_settings(&path).unwrap();
        assert_eq!(settings.ui.theme, theme.name);
        let back = Theme::resolve(&settings.ui.theme, &settings.ui.themes).unwrap();
        // A user theme with a built-in's name wins over the built-in.
        assert_eq!(settings.ui.themes.len(), 1, "{}", theme.name);
        assert_eq!(back, theme, "{}", theme.name);
    }
}

#[test]
fn exported_base16_imports_as_the_same_palette_roles() {
    let dir = tempfile::tempdir().unwrap();
    for theme in THEMES {
        let path = dir.path().join(format!("{}.yaml", theme.name));
        std::fs::write(&path, export(&theme, ExportFormat::Base16)).unwrap();
        let spec = format!("base16:{}", path.display());
        let source = FilePalette::from_spec(&spec, dir.path()).unwrap();
        let back = Theme::from_base16(&source.load().unwrap());
        assert_eq!(back.bg, theme.bg, "{}", theme.name);
        assert_eq!(back.text, theme.text, "{}", theme.name);
        assert_eq!(back.title, theme.title, "{}", theme.name);
        assert_eq!(back.error, theme.error, "{}", theme.name);
        assert_eq!(back.ok, theme.ok, "{}", theme.name);
        assert_eq!(back.lead, theme.lead, "{}", theme.name);
        assert_eq!(back.builder, theme.builder, "{}", theme.name);
        assert_eq!(back.haiku, theme.haiku, "{}", theme.name);
        assert_eq!(back.reviewer, theme.reviewer, "{}", theme.name);
    }
}

#[test]
fn a_rewritten_palette_file_is_seen_by_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join(".cache/wal");
    std::fs::create_dir_all(&wal).unwrap();
    let json = |bg: &str| {
        let colors: String = (0..16)
            .map(|i| {
                format!(
                    "\"color{i}\":\"#{:02x}{:02x}{:02x}\"",
                    16 * i + 8,
                    200 - 8 * i,
                    40 + 9 * i
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"special\":{{\"background\":\"{bg}\",\"foreground\":\"#dddddd\"}},\"colors\":{{{colors}}}}}"
        )
    };
    std::fs::write(wal.join("colors.json"), json("#101010")).unwrap();
    let source = FilePalette::from_spec("wal", dir.path()).unwrap();
    let before = source.stamp();
    assert_eq!(source.load().unwrap().slots[0], 0x101010);
    std::fs::write(wal.join("colors.json"), json("#1a1a2a ")).unwrap();
    assert_ne!(source.stamp(), before);
    assert_eq!(source.load().unwrap().slots[0], 0x1a1a2a);
}
