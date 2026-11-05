//! The background service: one per user (per data folder) runs the
//! engine, and terminal windows attach to it.
//!
//! A window is a remote screen, as with tmux: it sends what the user types
//! and the size it has, and is sent what to show, cell by cell, as changes
//! to the frame before. Every window attached sees the same screen, so an
//! approval answered in one is gone from all of them, and closing a window
//! leaves the work running.
//!
//! The transport is behind two ports declared here: [`WindowHub`], the
//! service's end, and [`Screen`], a window's end. Both carry the plain
//! types below; how they travel (a named pipe, a Unix socket) is the
//! adapter's business.

use std::time::Duration;

use crate::AppResult;
use swarmux_domain::Millis;

/// Changes whenever a message below changes shape. A window and a service
/// that differ in it, or in their versions, never attach.
pub const PROTOCOL: u32 = 1;

/// One attached window, as the service knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// A key, without the terminal library's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyName {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
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
    F(u8),
}

/// A key press and the modifiers held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub key: KeyName,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyInput {
    pub fn plain(key: KeyName) -> KeyInput {
        KeyInput {
            key,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
}

/// What a window sends after its [`Hello`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowInput {
    Key(KeyInput),
    /// The window's size changed; the next frame is sent whole.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// The window gained (`true`) or lost the user's attention.
    Focus(bool),
}

/// How a window wants glyphs drawn: what its environment says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphHints {
    /// `SWARMUX_ICONS`.
    pub icons: Option<String>,
    /// `NERD_FONT` or `SWARMUX_NERD_FONT`.
    pub nerd_font: Option<String>,
    /// `SWARMUX_FONT`.
    pub font: Option<String>,
}

/// The first thing a window says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The window's swarmux version.
    pub version: String,
    pub protocol: u32,
    pub cols: u16,
    pub rows: u16,
    pub glyphs: GlyphHints,
    /// Show the splash screen on attaching.
    pub splash: bool,
    /// The folder the window was opened in: new sessions start there.
    pub cwd: Option<String>,
}

/// A colour as the screen holds it: the terminal's default, one of its 16
/// named colours, an entry of the 256-colour palette, or RGB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Colour {
    #[default]
    Reset,
    Named(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// How a run of cells looks. `modifiers` are the terminal library's bits
/// (bold, dim, italic, ...), passed through as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub fg: Colour,
    pub bg: Colour,
    pub underline: Colour,
    pub modifiers: u16,
}

/// Cells side by side on one row with one style; `text` holds one symbol
/// per cell (a wide symbol's second cell is in it as nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub x: u16,
    pub y: u16,
    /// The symbol of each cell, in order.
    pub cells: Vec<String>,
    pub style: CellStyle,
}

/// What changed on the screen since the frame before, or (`full`) the
/// whole of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    pub full: bool,
    pub runs: Vec<Run>,
}

/// What the service says when a window says hello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Welcome {
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
    /// Jobs running or queued now.
    pub busy: usize,
    /// The window was attached; when not, `refused` says why.
    pub attached: bool,
    pub refused: Option<String>,
}

/// What the service sends a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Welcome(Welcome),
    Frame(Frame),
    /// A notification sequence (OSC 777, OSC 9 or the bell) for the
    /// window's terminal to write as it is.
    Notify(String),
    /// The window was let go (it pressed `q`, or the service stops), with
    /// what to tell the user once the terminal is back to normal.
    Bye(String),
}

/// A one-off question to the service, from `swarmux daemon`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Status,
    /// Stop: start nothing more, then end. With `wait`, running jobs
    /// finish first; without, they are stopped and resume next start.
    Stop {
        wait: bool,
    },
}

/// What `swarmux daemon status` shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DaemonStatus {
    pub pid: u32,
    pub version: String,
    pub started_at: Millis,
    pub running: usize,
    pub queued: usize,
    /// Windows attached now.
    pub windows: usize,
    /// It stops once the running jobs have finished.
    pub draining: bool,
}

/// The service's answer to an [`Ask`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Status(DaemonStatus),
    /// It is stopping; `running` jobs are let finish (with `wait`) or stopped.
    Stopping {
        running: usize,
        wait: bool,
    },
}

/// What reaches the service from windows and askers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubEvent {
    Hello(WindowId, Hello),
    Input(WindowId, WindowInput),
    /// The window closed or its connection broke.
    Left(WindowId),
    Asked(WindowId, Ask),
}

/// The service's end of the local connection.
pub trait WindowHub {
    /// What arrived since the last call, waiting up to `wait` for the
    /// first of it.
    fn poll(&self, wait: Duration) -> Vec<HubEvent>;
    /// Sends a window something to show; a window gone is ignored.
    fn send(&self, window: WindowId, shown: Shown);
    /// Answers an [`Ask`] and closes that connection.
    fn answer(&self, window: WindowId, answer: Answer);
    /// Closes a window's connection once what was sent has gone out.
    fn close(&self, window: WindowId);
}

/// A window's end of the local connection.
pub trait Screen {
    fn send(&mut self, input: WindowInput) -> AppResult<()>;
    /// The next thing the service sent, waiting up to `wait`; `Ok(None)`
    /// when nothing came. An error means the service is gone.
    fn recv(&mut self, wait: Duration) -> AppResult<Option<Shown>>;
}

/// Why a window and the service can't be attached, and whether restarting
/// the service (with this window's version) would be safe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skew {
    pub message: String,
    /// Nothing is running or queued: it may be restarted now.
    pub can_restart: bool,
}

/// Compares a window's version with the service's: `None` when they match.
pub fn skew(window: &str, service: &Welcome) -> Option<Skew> {
    if window == service.version && service.protocol == PROTOCOL {
        return None;
    }
    let can_restart = service.busy == 0;
    let mut message = format!(
        "The background service runs swarmux {}, this is swarmux {window}.",
        service.version
    );
    if can_restart {
        message.push_str(" Nothing is running, so it can be restarted with this version.");
    } else {
        message.push_str(&format!(
            " It has {} running or queued; let {} finish, then `swarmux daemon stop` \
             and run swarmux again (or `swarmux daemon stop` now: stopped jobs resume \
             at the next start).",
            jobs(service.busy),
            if service.busy == 1 { "it" } else { "them" }
        ));
    }
    Some(Skew {
        message,
        can_restart,
    })
}

/// What the user is told after leaving a window while the service runs on.
pub fn bye_message(running: usize, queued: usize) -> String {
    match (running, queued) {
        (0, 0) => "Nothing is running. The background service waits for work · \
                   `swarmux daemon stop` to stop it"
            .to_string(),
        (0, q) => format!(
            "{} keep{} waiting in the background · `swarmux daemon stop` to stop them",
            jobs(q),
            if q == 1 { "s" } else { "" }
        ),
        (r, _) => format!(
            "{} keep{} running in the background · `swarmux daemon stop` to stop them",
            jobs(r),
            if r == 1 { "s" } else { "" }
        ),
    }
}

fn jobs(n: usize) -> String {
    if n == 1 {
        "1 job".to_string()
    } else {
        format!("{n} jobs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn welcome(version: &str, busy: usize) -> Welcome {
        Welcome {
            version: version.into(),
            protocol: PROTOCOL,
            pid: 7,
            busy,
            attached: false,
            refused: None,
        }
    }

    #[test]
    fn same_version_attaches() {
        assert_eq!(skew("0.1.0", &welcome("0.1.0", 3)), None);
    }

    #[test]
    fn an_idle_service_of_another_version_may_be_restarted() {
        let s = skew("0.2.0", &welcome("0.1.0", 0)).unwrap();
        assert!(s.can_restart);
        assert!(
            s.message
                .starts_with("The background service runs swarmux 0.1.0, this is swarmux 0.2.0."),
            "{}",
            s.message
        );
    }

    #[test]
    fn a_busy_service_of_another_version_is_left_alone() {
        let s = skew("0.2.0", &welcome("0.1.0", 2)).unwrap();
        assert!(!s.can_restart);
        assert!(
            s.message.contains("It has 2 jobs running or queued"),
            "{}",
            s.message
        );
        assert!(s.message.contains("`swarmux daemon stop`"), "{}", s.message);
    }

    #[test]
    fn another_protocol_is_skew_even_at_the_same_version() {
        let mut w = welcome("0.1.0", 0);
        w.protocol = PROTOCOL + 1;
        assert!(skew("0.1.0", &w).is_some());
    }

    #[test]
    fn leaving_says_what_keeps_going() {
        assert_eq!(
            bye_message(2, 1),
            "2 jobs keep running in the background · `swarmux daemon stop` to stop them"
        );
        assert_eq!(
            bye_message(1, 0),
            "1 job keeps running in the background · `swarmux daemon stop` to stop them"
        );
        assert!(bye_message(0, 3).starts_with("3 jobs keep waiting"));
        assert!(bye_message(0, 0).starts_with("Nothing is running."));
    }
}
