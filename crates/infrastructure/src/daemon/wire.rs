//! The background service's messages on the wire: one JSON object per
//! line each way, as on the hook socket.
//!
//! A connection starts with the window's `hello` (or a one-off `ask`);
//! after a hello the window sends keys, sizes and focus, and is sent a
//! welcome, frames, notifications and finally a bye.

use serde::{Deserialize, Serialize};
use swarmux_application::daemon::{
    Answer, Ask, CellStyle, Colour, DaemonStatus, Frame, GlyphHints, Hello, KeyInput, KeyName, Run,
    Shown, Welcome, WindowInput,
};

/// Longest line read, in bytes: a whole frame of a large window fits.
pub const LINE_MAX: usize = 8 << 20;

/// What a window or an asker sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Up {
    Hello {
        version: String,
        protocol: u32,
        cols: u16,
        rows: u16,
        #[serde(default)]
        icons: Option<String>,
        #[serde(default)]
        nerd_font: Option<String>,
        #[serde(default)]
        font: Option<String>,
        #[serde(default)]
        splash: bool,
        #[serde(default)]
        cwd: Option<String>,
    },
    Key {
        k: String,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        shift: bool,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Focus {
        on: bool,
    },
    Status,
    Stop {
        #[serde(default)]
        wait: bool,
    },
}

/// One run of cells: column, row, style (an index into the frame's
/// styles) and the cells' symbols.
pub type WireRun = (u16, u16, u32, Vec<String>);

/// A style: foreground, background, underline colour and modifier bits.
pub type WireStyle = (u32, u32, u32, u16);

/// What the service sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Down {
    Welcome {
        version: String,
        protocol: u32,
        pid: u32,
        busy: usize,
        attached: bool,
        #[serde(default)]
        refused: Option<String>,
    },
    Frame {
        cols: u16,
        rows: u16,
        full: bool,
        styles: Vec<WireStyle>,
        runs: Vec<WireRun>,
    },
    Notify {
        seq: String,
    },
    Bye {
        text: String,
    },
    Status {
        pid: u32,
        version: String,
        started_at: i64,
        running: usize,
        queued: usize,
        windows: usize,
        draining: bool,
    },
    Stopping {
        running: usize,
        wait: bool,
    },
}

/// What the first line of a connection asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opening {
    Hello(Hello),
    Ask(Ask),
}

impl Up {
    /// The first line as a hello or an ask; anything else opens nothing.
    pub fn opening(self) -> Option<Opening> {
        match self {
            Up::Hello {
                version,
                protocol,
                cols,
                rows,
                icons,
                nerd_font,
                font,
                splash,
                cwd,
            } => Some(Opening::Hello(Hello {
                version,
                protocol,
                cols,
                rows,
                glyphs: GlyphHints {
                    icons,
                    nerd_font,
                    font,
                },
                splash,
                cwd,
            })),
            Up::Status => Some(Opening::Ask(Ask::Status)),
            Up::Stop { wait } => Some(Opening::Ask(Ask::Stop { wait })),
            _ => None,
        }
    }

    /// A later line as window input; `None` for lines that are not.
    pub fn input(self) -> Option<WindowInput> {
        match self {
            Up::Key {
                k,
                ctrl,
                alt,
                shift,
            } => Some(WindowInput::Key(KeyInput {
                key: key_from(&k)?,
                ctrl,
                alt,
                shift,
            })),
            Up::Resize { cols, rows } => Some(WindowInput::Resize { cols, rows }),
            Up::Focus { on } => Some(WindowInput::Focus(on)),
            _ => None,
        }
    }

    pub fn hello(h: &Hello) -> Up {
        Up::Hello {
            version: h.version.clone(),
            protocol: h.protocol,
            cols: h.cols,
            rows: h.rows,
            icons: h.glyphs.icons.clone(),
            nerd_font: h.glyphs.nerd_font.clone(),
            font: h.glyphs.font.clone(),
            splash: h.splash,
            cwd: h.cwd.clone(),
        }
    }

    pub fn ask(ask: Ask) -> Up {
        match ask {
            Ask::Status => Up::Status,
            Ask::Stop { wait } => Up::Stop { wait },
        }
    }

    pub fn of_input(input: &WindowInput) -> Up {
        match *input {
            WindowInput::Key(k) => Up::Key {
                k: key_name(k.key),
                ctrl: k.ctrl,
                alt: k.alt,
                shift: k.shift,
            },
            WindowInput::Resize { cols, rows } => Up::Resize { cols, rows },
            WindowInput::Focus(on) => Up::Focus { on },
        }
    }
}

impl Down {
    pub fn of_shown(shown: &Shown) -> Down {
        match shown {
            Shown::Welcome(w) => Down::Welcome {
                version: w.version.clone(),
                protocol: w.protocol,
                pid: w.pid,
                busy: w.busy,
                attached: w.attached,
                refused: w.refused.clone(),
            },
            Shown::Frame(f) => frame_down(f),
            Shown::Notify(seq) => Down::Notify { seq: seq.clone() },
            Shown::Bye(text) => Down::Bye { text: text.clone() },
        }
    }

    pub fn of_answer(answer: &Answer) -> Down {
        match answer {
            Answer::Status(s) => Down::Status {
                pid: s.pid,
                version: s.version.clone(),
                started_at: s.started_at,
                running: s.running,
                queued: s.queued,
                windows: s.windows,
                draining: s.draining,
            },
            Answer::Stopping { running, wait } => Down::Stopping {
                running: *running,
                wait: *wait,
            },
        }
    }

    /// What a window shows of it; `None` for answers to asks.
    pub fn shown(self) -> Option<Shown> {
        match self {
            Down::Welcome {
                version,
                protocol,
                pid,
                busy,
                attached,
                refused,
            } => Some(Shown::Welcome(Welcome {
                version,
                protocol,
                pid,
                busy,
                attached,
                refused,
            })),
            Down::Frame {
                cols,
                rows,
                full,
                styles,
                runs,
            } => Some(Shown::Frame(Frame {
                cols,
                rows,
                full,
                runs: runs
                    .into_iter()
                    .map(|(x, y, s, cells)| Run {
                        x,
                        y,
                        cells,
                        style: styles.get(s as usize).map_or_else(
                            CellStyle::default,
                            |&(fg, bg, ul, m)| CellStyle {
                                fg: colour_from(fg),
                                bg: colour_from(bg),
                                underline: colour_from(ul),
                                modifiers: m,
                            },
                        ),
                    })
                    .collect(),
            })),
            Down::Notify { seq } => Some(Shown::Notify(seq)),
            Down::Bye { text } => Some(Shown::Bye(text)),
            Down::Status { .. } | Down::Stopping { .. } => None,
        }
    }

    /// The answer to an ask; `None` for what windows are sent.
    pub fn answer(self) -> Option<Answer> {
        match self {
            Down::Status {
                pid,
                version,
                started_at,
                running,
                queued,
                windows,
                draining,
            } => Some(Answer::Status(DaemonStatus {
                pid,
                version,
                started_at,
                running,
                queued,
                windows,
                draining,
            })),
            Down::Stopping { running, wait } => Some(Answer::Stopping { running, wait }),
            _ => None,
        }
    }
}

/// A frame with its styles gathered into one table.
fn frame_down(f: &Frame) -> Down {
    let mut styles: Vec<WireStyle> = Vec::new();
    let runs = f
        .runs
        .iter()
        .map(|r| {
            let s = (
                colour_code(r.style.fg),
                colour_code(r.style.bg),
                colour_code(r.style.underline),
                r.style.modifiers,
            );
            let i = styles.iter().position(|x| *x == s).unwrap_or_else(|| {
                styles.push(s);
                styles.len() - 1
            });
            (r.x, r.y, i as u32, r.cells.clone())
        })
        .collect();
    Down::Frame {
        cols: f.cols,
        rows: f.rows,
        full: f.full,
        styles,
        runs,
    }
}

const NAMED: u32 = 1;
const INDEXED: u32 = 0x100;
const RGB: u32 = 0x0100_0000;

fn colour_code(c: Colour) -> u32 {
    match c {
        Colour::Reset => 0,
        Colour::Named(n) => NAMED + u32::from(n.min(15)),
        Colour::Indexed(i) => INDEXED | u32::from(i),
        Colour::Rgb(r, g, b) => RGB | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
    }
}

fn colour_from(code: u32) -> Colour {
    match code {
        0 => Colour::Reset,
        c if c >= RGB => Colour::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8),
        c if c >= INDEXED => Colour::Indexed(c as u8),
        c => Colour::Named((c - NAMED).min(15) as u8),
    }
}

fn key_name(k: KeyName) -> String {
    match k {
        KeyName::Char(c) => format!("c:{c}"),
        KeyName::F(n) => format!("f{n}"),
        other => match other {
            KeyName::Enter => "enter",
            KeyName::Esc => "esc",
            KeyName::Backspace => "backspace",
            KeyName::Tab => "tab",
            KeyName::BackTab => "backtab",
            KeyName::Up => "up",
            KeyName::Down => "down",
            KeyName::Left => "left",
            KeyName::Right => "right",
            KeyName::Home => "home",
            KeyName::End => "end",
            KeyName::PageUp => "pageup",
            KeyName::PageDown => "pagedown",
            KeyName::Delete => "delete",
            KeyName::Insert => "insert",
            KeyName::Char(_) | KeyName::F(_) => unreachable!("handled above"),
        }
        .to_string(),
    }
}

fn key_from(s: &str) -> Option<KeyName> {
    if let Some(c) = s.strip_prefix("c:") {
        let mut chars = c.chars();
        let first = chars.next()?;
        return chars.next().is_none().then_some(KeyName::Char(first));
    }
    if let Some(n) = s.strip_prefix('f').and_then(|n| n.parse().ok()) {
        return Some(KeyName::F(n));
    }
    Some(match s {
        "enter" => KeyName::Enter,
        "esc" => KeyName::Esc,
        "backspace" => KeyName::Backspace,
        "tab" => KeyName::Tab,
        "backtab" => KeyName::BackTab,
        "up" => KeyName::Up,
        "down" => KeyName::Down,
        "left" => KeyName::Left,
        "right" => KeyName::Right,
        "home" => KeyName::Home,
        "end" => KeyName::End,
        "pageup" => KeyName::PageUp,
        "pagedown" => KeyName::PageDown,
        "delete" => KeyName::Delete,
        "insert" => KeyName::Insert,
        _ => return None,
    })
}

/// One message as a line, newline included.
pub fn line<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_application::daemon::PROTOCOL;

    fn round_up(u: &Up) -> Up {
        serde_json::from_str(&line(u)).unwrap()
    }

    #[test]
    fn every_key_round_trips() {
        let keys = [
            KeyName::Char('q'),
            KeyName::Char('é'),
            KeyName::Char(':'),
            KeyName::Enter,
            KeyName::Esc,
            KeyName::Backspace,
            KeyName::Tab,
            KeyName::BackTab,
            KeyName::Up,
            KeyName::Down,
            KeyName::Left,
            KeyName::Right,
            KeyName::Home,
            KeyName::End,
            KeyName::PageUp,
            KeyName::PageDown,
            KeyName::Delete,
            KeyName::Insert,
            KeyName::F(5),
        ];
        for key in keys {
            let input = WindowInput::Key(KeyInput {
                key,
                ctrl: true,
                alt: false,
                shift: true,
            });
            assert_eq!(round_up(&Up::of_input(&input)).input(), Some(input));
        }
        assert_eq!(key_from("c:ab"), None);
        assert_eq!(key_from("nope"), None);
    }

    #[test]
    fn hello_and_asks_open_a_connection() {
        let hello = Hello {
            version: "0.1.0".into(),
            protocol: PROTOCOL,
            cols: 120,
            rows: 40,
            glyphs: GlyphHints {
                icons: Some("nerd".into()),
                nerd_font: None,
                font: None,
            },
            splash: true,
            cwd: Some("/work".into()),
        };
        assert_eq!(
            round_up(&Up::hello(&hello)).opening(),
            Some(Opening::Hello(hello))
        );
        let stop = Ask::Stop { wait: true };
        assert_eq!(round_up(&Up::ask(stop)).opening(), Some(Opening::Ask(stop)));
        assert_eq!(Up::Focus { on: true }.opening(), None);
        // An old window's hello without the newer fields still opens.
        let old: Up = serde_json::from_str(
            r#"{"t":"hello","version":"0.1.0","protocol":1,"cols":80,"rows":24}"#,
        )
        .unwrap();
        assert!(matches!(old.opening(), Some(Opening::Hello(_))));
    }

    #[test]
    fn frames_round_trip_with_one_table_of_styles() {
        let bold = CellStyle {
            fg: Colour::Rgb(1, 2, 3),
            bg: Colour::Indexed(236),
            underline: Colour::Reset,
            modifiers: 1,
        };
        let plain = CellStyle {
            fg: Colour::Named(9),
            ..CellStyle::default()
        };
        let frame = Frame {
            cols: 80,
            rows: 24,
            full: true,
            runs: vec![
                Run {
                    x: 0,
                    y: 0,
                    cells: vec!["s".into(), "w".into()],
                    style: bold,
                },
                Run {
                    x: 5,
                    y: 3,
                    cells: vec!["界".into(), String::new()],
                    style: plain,
                },
                Run {
                    x: 9,
                    y: 3,
                    cells: vec!["!".into()],
                    style: bold,
                },
            ],
        };
        let down = Down::of_shown(&Shown::Frame(frame.clone()));
        let Down::Frame { styles, .. } = &down else {
            panic!()
        };
        assert_eq!(styles.len(), 2);
        let back: Down = serde_json::from_str(&line(&down)).unwrap();
        assert_eq!(back.shown(), Some(Shown::Frame(frame)));
    }

    #[test]
    fn answers_round_trip() {
        let status = Answer::Status(DaemonStatus {
            pid: 42,
            version: "0.1.0".into(),
            started_at: 1_700_000_000_000,
            running: 2,
            queued: 1,
            windows: 1,
            draining: false,
        });
        let back: Down = serde_json::from_str(&line(&Down::of_answer(&status))).unwrap();
        assert_eq!(back.answer(), Some(status));
        let w = Shown::Welcome(Welcome {
            version: "0.1.0".into(),
            protocol: PROTOCOL,
            pid: 1,
            busy: 0,
            attached: false,
            refused: Some("versions differ".into()),
        });
        let back: Down = serde_json::from_str(&line(&Down::of_shown(&w))).unwrap();
        assert_eq!(back.shown(), Some(w));
    }
}
