//! Windows attached to the background service, as with tmux.
//!
//! In the service the app runs with no terminal of its own: [`Service`]
//! steps it, renders it for each attached window at that window's size
//! and sends the cells that changed. [`attach`] is a window's end: it
//! draws what it is sent and sends back what is typed. Every window sees
//! the same app, so whatever is done in one (an approval answered, a
//! prompt sent) shows in all of them at once.
//!
//! Both ends talk through the ports in `swarmux_application::daemon`;
//! nothing here knows about sockets or processes.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use swarmux_application::daemon::{
    Answer, Ask, CellStyle, Colour, DaemonStatus, Frame, HubEvent, KeyInput, KeyName, PROTOCOL,
    Run, Screen, Shown, Welcome, WindowHub, WindowId, WindowInput, bye_message,
};
use swarmux_domain::Millis;

use crate::App;
use crate::theme::ColorDepth;

/// How often the app is stepped while no window is attached.
const IDLE_TICK: Duration = Duration::from_millis(250);

/// What the service says about itself.
#[derive(Clone, Debug)]
pub struct ServiceInfo {
    pub pid: u32,
    pub version: String,
    pub started_at: Millis,
}

/// One attached window.
struct Window {
    cols: u16,
    rows: u16,
    /// What it was last sent: the next frame is the difference.
    last: Option<Buffer>,
    focused: bool,
}

/// The service's side: the app, stepped and shown to every window.
pub struct Service {
    info: ServiceInfo,
    windows: BTreeMap<WindowId, Window>,
    /// Some window has the user's attention.
    focused: bool,
    /// Stopping once the running jobs have finished.
    draining: bool,
    /// Lines for the service's log.
    log: Vec<String>,
}

impl Service {
    /// Takes over `app` for the service: colours stay full (each window
    /// brings them down to what its terminal shows) and `q` lets the
    /// window go instead of quitting.
    pub fn new(app: &mut App, info: ServiceInfo) -> Service {
        app.look.depth = ColorDepth::TrueColor;
        app.set_service(true);
        // Nobody sees a splash at the start; windows get theirs on attaching.
        app.look.splash_until = None;
        // Nobody looks until a window attaches.
        app.on_event(Event::FocusLost);
        Service {
            info,
            windows: BTreeMap::new(),
            focused: false,
            draining: false,
            log: Vec::new(),
        }
    }

    /// Windows attached now.
    pub fn windows(&self) -> usize {
        self.windows.len()
    }

    /// What happened since the last call, for the service's log.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// One step: takes what arrived (waiting up to a frame for it), steps
    /// the app and sends each window what changed. Returns `Some(wait)`
    /// once the service is to stop: `wait` means the running jobs finished
    /// first, otherwise they are still to be stopped.
    pub fn step(&mut self, app: &mut App, hub: &dyn WindowHub) -> Option<bool> {
        let wait = if self.windows.is_empty() {
            IDLE_TICK
        } else {
            app.frame_time()
        };
        let mut stop = None;
        for event in hub.poll(wait) {
            if let Some(s) = self.take(app, hub, event) {
                stop = Some(s);
            }
        }
        let focused = self.windows.values().any(|w| w.focused);
        if focused != self.focused {
            self.focused = focused;
            app.on_event(if focused {
                Event::FocusGained
            } else {
                Event::FocusLost
            });
        }
        app.update();
        if self.draining && app.engine().busy().0 == 0 {
            stop = Some(true);
        }
        if let Some(wait) = stop {
            for id in std::mem::take(&mut self.windows).into_keys() {
                hub.send(id, Shown::Bye("The background service stopped.".into()));
                hub.close(id);
            }
            return Some(wait);
        }
        for (id, w) in &mut self.windows {
            let started = std::time::Instant::now();
            let buf = app.render_buffer(w.cols, w.rows);
            app.look.auto_motion.observe_frame(started.elapsed());
            let frame = frame_of(w.last.as_ref(), &buf);
            if frame.full || !frame.runs.is_empty() {
                hub.send(*id, Shown::Frame(frame));
            }
            w.last = Some(buf);
        }
        app.frame = app.frame.wrapping_add(1);
        None
    }

    fn take(&mut self, app: &mut App, hub: &dyn WindowHub, event: HubEvent) -> Option<bool> {
        match event {
            HubEvent::Hello(id, hello) => {
                let (running, queued) = app.engine().busy();
                let refused = if hello.version != self.info.version || hello.protocol != PROTOCOL {
                    Some(format!(
                        "this service runs swarmux {}, the window swarmux {}",
                        self.info.version, hello.version
                    ))
                } else if self.draining {
                    Some("the service is stopping".to_string())
                } else {
                    None
                };
                hub.send(
                    id,
                    Shown::Welcome(Welcome {
                        version: self.info.version.clone(),
                        protocol: PROTOCOL,
                        pid: self.info.pid,
                        busy: running + queued,
                        attached: refused.is_none(),
                        refused: refused.clone(),
                    }),
                );
                if let Some(why) = refused {
                    self.log.push(format!("window refused: {why}"));
                    hub.close(id);
                    return None;
                }
                self.windows.insert(
                    id,
                    Window {
                        cols: hello.cols.max(1),
                        rows: hello.rows.max(1),
                        last: None,
                        focused: true,
                    },
                );
                // The newest window's terminal decides the glyphs.
                app.set_glyphs(&hello.glyphs);
                if let Some(cwd) = &hello.cwd {
                    app.set_cwd(cwd.into());
                }
                if hello.splash {
                    app.splash_again();
                }
                self.log
                    .push(format!("window attached ({} now)", self.windows.len()));
            }
            HubEvent::Input(id, input) => {
                let w = self.windows.get_mut(&id)?;
                match input {
                    WindowInput::Key(key) => {
                        w.focused = true;
                        app.on_event(Event::Key(key_event(key)));
                        if app.take_quit() {
                            let (running, queued) = app.engine().busy();
                            hub.send(id, Shown::Bye(bye_message(running, queued)));
                            hub.close(id);
                            self.windows.remove(&id);
                            self.log
                                .push(format!("window left ({} now)", self.windows.len()));
                        }
                    }
                    WindowInput::Resize { cols, rows } => {
                        w.cols = cols.max(1);
                        w.rows = rows.max(1);
                        w.last = None;
                    }
                    WindowInput::Focus(on) => w.focused = on,
                }
            }
            HubEvent::Left(id) => {
                if self.windows.remove(&id).is_some() {
                    self.log
                        .push(format!("window closed ({} now)", self.windows.len()));
                }
            }
            HubEvent::Asked(id, Ask::Status) => {
                let (running, queued) = app.engine().busy();
                hub.answer(
                    id,
                    Answer::Status(DaemonStatus {
                        pid: self.info.pid,
                        version: self.info.version.clone(),
                        started_at: self.info.started_at,
                        running,
                        queued,
                        windows: self.windows.len(),
                        draining: self.draining,
                    }),
                );
            }
            HubEvent::Asked(id, Ask::Stop { wait }) => {
                let (running, _) = app.engine().busy();
                hub.answer(id, Answer::Stopping { running, wait });
                self.log.push(format!(
                    "asked to stop ({running} running, {})",
                    if wait {
                        "letting them finish"
                    } else {
                        "stopping them"
                    }
                ));
                if wait && running > 0 {
                    self.draining = true;
                    app.engine_mut().jobs.set_draining(true);
                } else {
                    return Some(wait);
                }
            }
        }
        None
    }
}

/// How a window's attachment ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detached {
    /// The service let it go, with what to tell the user.
    Bye(String),
    /// The connection broke.
    Lost(String),
}

/// A window's end: takes over the terminal, draws what the service sends
/// and sends what is typed, until the service lets it go. The terminal
/// is restored before this returns.
pub fn attach(screen: &mut dyn Screen, cols: u16, rows: u16) -> io::Result<Detached> {
    let mut terminal = ratatui::init();
    // Terminals that don't know focus reporting ignore this.
    let _ = crossterm::execute!(io::stdout(), crossterm::event::EnableFocusChange);
    let result = attached(&mut terminal, screen, cols, rows);
    let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableFocusChange);
    ratatui::restore();
    result
}

fn attached(
    terminal: &mut ratatui::DefaultTerminal,
    screen: &mut dyn Screen,
    cols: u16,
    rows: u16,
) -> io::Result<Detached> {
    let depth = ColorDepth::from_env();
    let mut shown = Buffer::empty(Rect::new(0, 0, cols, rows));
    loop {
        let mut dirty = false;
        loop {
            match screen.recv(Duration::ZERO) {
                Ok(Some(Shown::Frame(f))) => {
                    apply(&mut shown, &f);
                    dirty = true;
                }
                Ok(Some(Shown::Notify(seq))) => {
                    let mut out = io::stdout().lock();
                    let _ = out.write_all(seq.as_bytes()).and_then(|()| out.flush());
                }
                Ok(Some(Shown::Bye(text))) => return Ok(Detached::Bye(text)),
                Ok(Some(Shown::Welcome(_))) => {}
                Ok(None) => break,
                Err(e) => return Ok(Detached::Lost(e.to_string())),
            }
        }
        if dirty {
            terminal.draw(|f| {
                let out = f.buffer_mut();
                copy_into(&shown, out);
                depth.apply(out);
            })?;
        }
        if event::poll(Duration::from_millis(15))? {
            let input = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    key_input(k).map(WindowInput::Key)
                }
                Event::Resize(cols, rows) => Some(WindowInput::Resize { cols, rows }),
                Event::FocusGained => Some(WindowInput::Focus(true)),
                Event::FocusLost => Some(WindowInput::Focus(false)),
                _ => None,
            };
            if let Some(input) = input
                && let Err(e) = screen.send(input)
            {
                return Ok(Detached::Lost(e.to_string()));
            }
        }
    }
}

/// The cells of `next` that differ from `prev` (all of them when there is
/// no `prev` or its size differs), as runs of one style per row.
pub(crate) fn frame_of(prev: Option<&Buffer>, next: &Buffer) -> Frame {
    let area = next.area;
    let prev = prev.filter(|p| p.area == area);
    let full = prev.is_none();
    let changed = |x: u16, y: u16| prev.is_none_or(|p| p[(x, y)] != next[(x, y)]);
    let mut runs = Vec::new();
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            if !changed(x, y) {
                x += 1;
                continue;
            }
            let style = style_of(&next[(x, y)]);
            let mut run = Run {
                x,
                y,
                cells: Vec::new(),
                style,
            };
            while x < area.right() && changed(x, y) && style_of(&next[(x, y)]) == style {
                run.cells.push(next[(x, y)].symbol().to_string());
                x += 1;
            }
            runs.push(run);
        }
    }
    Frame {
        cols: area.width,
        rows: area.height,
        full,
        runs,
    }
}

/// Applies a frame to what a window holds.
pub(crate) fn apply(buf: &mut Buffer, frame: &Frame) {
    let area = Rect::new(0, 0, frame.cols, frame.rows);
    if frame.full || buf.area != area {
        *buf = Buffer::empty(area);
    }
    for run in &frame.runs {
        for (i, symbol) in run.cells.iter().enumerate() {
            let x = run.x.saturating_add(i as u16);
            if x >= area.right() || run.y >= area.bottom() {
                continue;
            }
            let cell = &mut buf[(x, run.y)];
            cell.set_symbol(symbol);
            cell.fg = color(run.style.fg);
            cell.bg = color(run.style.bg);
            cell.underline_color = color(run.style.underline);
            cell.modifier = Modifier::from_bits_truncate(run.style.modifiers);
        }
    }
}

/// Copies what fits of `from` into `to`; the rest of `to` stays blank.
fn copy_into(from: &Buffer, to: &mut Buffer) {
    let a = from.area.intersection(to.area);
    for y in a.top()..a.bottom() {
        for x in a.left()..a.right() {
            to[(x, y)] = from[(x, y)].clone();
        }
    }
}

fn style_of(c: &Cell) -> CellStyle {
    CellStyle {
        fg: colour(c.fg),
        bg: colour(c.bg),
        underline: colour(c.underline_color),
        modifiers: c.modifier.bits(),
    }
}

const NAMED: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

fn colour(c: Color) -> Colour {
    match c {
        Color::Reset => Colour::Reset,
        Color::Indexed(i) => Colour::Indexed(i),
        Color::Rgb(r, g, b) => Colour::Rgb(r, g, b),
        named => NAMED
            .iter()
            .position(|n| *n == named)
            .map_or(Colour::Reset, |i| Colour::Named(i as u8)),
    }
}

fn color(c: Colour) -> Color {
    match c {
        Colour::Reset => Color::Reset,
        Colour::Named(n) => NAMED[usize::from(n.min(15))],
        Colour::Indexed(i) => Color::Indexed(i),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// A key the terminal read, as the service is sent it.
fn key_input(k: KeyEvent) -> Option<KeyInput> {
    let key = match k.code {
        KeyCode::Char(c) => KeyName::Char(c),
        KeyCode::Enter => KeyName::Enter,
        KeyCode::Esc => KeyName::Esc,
        KeyCode::Backspace => KeyName::Backspace,
        KeyCode::Tab => KeyName::Tab,
        KeyCode::BackTab => KeyName::BackTab,
        KeyCode::Up => KeyName::Up,
        KeyCode::Down => KeyName::Down,
        KeyCode::Left => KeyName::Left,
        KeyCode::Right => KeyName::Right,
        KeyCode::Home => KeyName::Home,
        KeyCode::End => KeyName::End,
        KeyCode::PageUp => KeyName::PageUp,
        KeyCode::PageDown => KeyName::PageDown,
        KeyCode::Delete => KeyName::Delete,
        KeyCode::Insert => KeyName::Insert,
        KeyCode::F(n) => KeyName::F(n),
        _ => return None,
    };
    Some(KeyInput {
        key,
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
        shift: k.modifiers.contains(KeyModifiers::SHIFT),
    })
}

/// A key a window sent, as the app reads keys.
fn key_event(k: KeyInput) -> KeyEvent {
    let code = match k.key {
        KeyName::Char(c) => KeyCode::Char(c),
        KeyName::Enter => KeyCode::Enter,
        KeyName::Esc => KeyCode::Esc,
        KeyName::Backspace => KeyCode::Backspace,
        KeyName::Tab => KeyCode::Tab,
        KeyName::BackTab => KeyCode::BackTab,
        KeyName::Up => KeyCode::Up,
        KeyName::Down => KeyCode::Down,
        KeyName::Left => KeyCode::Left,
        KeyName::Right => KeyCode::Right,
        KeyName::Home => KeyCode::Home,
        KeyName::End => KeyCode::End,
        KeyName::PageUp => KeyCode::PageUp,
        KeyName::PageDown => KeyCode::PageDown,
        KeyName::Delete => KeyCode::Delete,
        KeyName::Insert => KeyCode::Insert,
        KeyName::F(n) => KeyCode::F(n),
    };
    let mut modifiers = KeyModifiers::NONE;
    modifiers.set(KeyModifiers::CONTROL, k.ctrl);
    modifiers.set(KeyModifiers::ALT, k.alt);
    modifiers.set(KeyModifiers::SHIFT, k.shift);
    KeyEvent::new(code, modifiers)
}

/// This terminal's size, columns then rows (a common size when it can't be
/// read).
pub fn window_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((120, 40))
}

/// What this terminal's environment says about glyphs.
pub fn glyph_hints() -> swarmux_application::daemon::GlyphHints {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    swarmux_application::daemon::GlyphHints {
        icons: var("SWARMUX_ICONS"),
        nerd_font: var("NERD_FONT").or_else(|| var("SWARMUX_NERD_FONT")),
        font: var("SWARMUX_FONT"),
    }
}

#[cfg(test)]
mod tests;
