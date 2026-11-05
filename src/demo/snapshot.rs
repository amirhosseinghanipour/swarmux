//! `swarmux --demo --snapshot DIR`: the demo's key moments as frame files.
//!
//! The demo is played without a terminal, the way its tests play it, and
//! each moment is written as JSON: rows of styled runs with colours as hex.
//! The website and the README images are made from these files, so what
//! they show is what the app draws.

use std::path::Path;

use anyhow::Context;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use swarmux_tui::App;
use unicode_width::UnicodeWidthStr;

use super::{LABEL, Options, build};

/// The frame size, in cells.
pub const WIDTH: u16 = 160;
pub const HEIGHT: u16 = 48;
/// Ticks of the whole scenario, plus a little for the last job to settle.
const TICKS: u32 = 340;

/// A moment caught on the Mission control screen when `needle` first shows.
const MOMENTS: [(&str, &str); 5] = [
    ("guard-deny", "deny Edit tests/checkout_oracle.rs"),
    ("p1-approval", "P1 ◆ approval W web-checkout Bash git push"),
    ("flood", "flood: 13 items from 2 causes"),
    ("gate-blocked", "gate blocked a stop (1 of"),
    ("gate-passed", "gate passed"),
];

/// Screens caught once everything has happened: (file name, key).
const SCREENS: [(&str, char); 6] = [
    ("missions", '2'),
    ("queue", '4'),
    ("ledger", '5'),
    ("sessions", '6'),
    ("focus", '0'),
    ("mission-control", '1'),
];

/// Plays the demo and writes one `<name>.json` per moment into `dir`.
/// Returns the names written, in order.
pub fn write(options: &Options, dir: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot make the folder {}", dir.display()))?;
    let theme = options.theme.clone().unwrap_or_else(|| "reference".into());
    let mut app = build(options)?;
    let mut written = Vec::new();
    let save = |name: &str, app: &App, written: &mut Vec<String>| -> anyhow::Result<()> {
        let frame = frame(&app.render_buffer(WIDTH, HEIGHT), name, &theme);
        let path = dir.join(format!("{name}.json"));
        std::fs::write(&path, serde_json::to_string(&frame)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
        written.push(name.to_string());
        Ok(())
    };

    let mut seen = [false; MOMENTS.len()];
    for _ in 0..TICKS {
        tick(&mut app);
        let text = app.render_text(WIDTH, HEIGHT);
        for (i, (name, needle)) in MOMENTS.iter().enumerate() {
            if !seen[i] && text.contains(needle) {
                seen[i] = true;
                save(name, &app, &mut written)?;
            }
        }
    }
    for (name, key) in SCREENS {
        app.press_char(key);
        tick(&mut app);
        save(name, &app, &mut written)?;
    }
    Ok(written)
}

/// One update and one frame, as the run loop does them.
fn tick(app: &mut App) {
    app.update();
    app.frame += 1;
}

/// A buffer as `{name, theme, label, width, height, rows: [[run…]…]}`, where
/// a run is `{t, fg, bg, b?, d?, i?, u?, r?}` and colours are `#rrggbb` or
/// null for the terminal's own.
fn frame(buffer: &Buffer, name: &str, theme: &str) -> Value {
    let area = buffer.area;
    let mut rows = Vec::with_capacity(usize::from(area.height));
    for y in area.top()..area.bottom() {
        let mut runs: Vec<Value> = Vec::new();
        let mut text = String::new();
        let mut style: Option<Style> = None;
        // Cells covered by a wide symbol on their left are not drawn, as
        // in a terminal.
        let mut covered = 0;
        for x in area.left()..area.right() {
            if covered > 0 {
                covered -= 1;
                continue;
            }
            let cell = &buffer[(x, y)];
            covered = cell.symbol().width().saturating_sub(1);
            let here = Style::of(cell.fg, cell.bg, cell.modifier);
            if style.as_ref() != Some(&here) {
                if let Some(done) = style.take() {
                    runs.push(done.run(std::mem::take(&mut text)));
                }
                style = Some(here);
            }
            text.push_str(cell.symbol());
        }
        if let Some(done) = style {
            runs.push(done.run(text));
        }
        rows.push(Value::Array(runs));
    }
    json!({
        "name": name,
        "theme": theme,
        "label": LABEL,
        "width": area.width,
        "height": area.height,
        "rows": rows,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Style {
    fg: Option<String>,
    bg: Option<String>,
    modifier: Modifier,
}

impl Style {
    fn of(fg: Color, bg: Color, modifier: Modifier) -> Style {
        Style {
            fg: hex(fg),
            bg: hex(bg),
            modifier,
        }
    }

    fn run(self, text: String) -> Value {
        let mut run = json!({ "t": text, "fg": self.fg, "bg": self.bg });
        for (flag, key) in [
            (Modifier::BOLD, "b"),
            (Modifier::DIM, "d"),
            (Modifier::ITALIC, "i"),
            (Modifier::UNDERLINED, "u"),
            (Modifier::REVERSED, "r"),
        ] {
            if self.modifier.contains(flag) {
                run[key] = Value::Bool(true);
            }
        }
        run
    }
}

/// A colour as `#rrggbb`; `None` for the terminal's default.
fn hex(color: Color) -> Option<String> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0xcd, 0x31, 0x31),
        Color::Green => (0x0d, 0xbc, 0x79),
        Color::Yellow => (0xe5, 0xe5, 0x10),
        Color::Blue => (0x24, 0x72, 0xc8),
        Color::Magenta => (0xbc, 0x3f, 0xbc),
        Color::Cyan => (0x11, 0xa8, 0xcd),
        Color::Gray => (0xe5, 0xe5, 0xe5),
        Color::DarkGray => (0x66, 0x66, 0x66),
        Color::LightRed => (0xf1, 0x4c, 0x4c),
        Color::LightGreen => (0x23, 0xd1, 0x8b),
        Color::LightYellow => (0xf5, 0xf5, 0x43),
        Color::LightBlue => (0x3b, 0x8e, 0xea),
        Color::LightMagenta => (0xd6, 0x70, 0xd6),
        Color::LightCyan => (0x29, 0xb8, 0xdb),
        Color::White => (0xff, 0xff, 0xff),
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// The xterm 256-colour palette.
fn indexed(i: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match i {
        0..=15 => BASE[usize::from(i)],
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_moment_and_screen_is_written_with_the_demo_tag() {
        let dir = tempfile::tempdir().unwrap();
        let names = write(&Options::default(), dir.path()).unwrap();
        assert_eq!(names.len(), MOMENTS.len() + SCREENS.len(), "{names:?}");
        for name in &names {
            let raw = std::fs::read_to_string(dir.path().join(format!("{name}.json"))).unwrap();
            let frame: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(frame["rows"].as_array().unwrap().len(), usize::from(HEIGHT));
            assert!(raw.contains(LABEL), "{name} has no demo tag");
        }
    }

    #[test]
    fn a_row_keeps_every_cell() {
        let dir = tempfile::tempdir().unwrap();
        write(&Options::default(), dir.path()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("mission-control.json")).unwrap();
        let frame: Value = serde_json::from_str(&raw).unwrap();
        for row in frame["rows"].as_array().unwrap() {
            let width: usize = row
                .as_array()
                .unwrap()
                .iter()
                .map(|run| run["t"].as_str().unwrap().width())
                .sum();
            assert_eq!(width, usize::from(WIDTH), "a row is not the frame's width");
        }
    }

    #[test]
    fn colours_are_hex() {
        assert_eq!(hex(Color::Rgb(232, 140, 70)).as_deref(), Some("#e88c46"));
        assert_eq!(hex(Color::Reset), None);
        assert_eq!(indexed(16), (0, 0, 0));
        assert_eq!(indexed(231), (255, 255, 255));
        assert_eq!(indexed(232), (8, 8, 8));
    }
}
