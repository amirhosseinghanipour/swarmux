//! The Constellation: agents orbit a core on a braille canvas. An orbit's
//! radius is the agent's context fill and a star's brightness is its tokens
//! per second. Tool calls shed a comet tail, an approval makes a star flare,
//! and a finished agent goes nova before it settles as a dim check.

use std::f64::consts::TAU;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::showcase_chrome::{self, Chrome};
use super::widgets::BrailleGrid;
use crate::app::App;
use crate::showcase::{Agent, COMET_MS, NOVA_MS, Phase, Showcase};
use crate::theme::{Theme, mix};

/// Smallest orbit, as a share of the largest.
const INNER: f64 = 0.16;
/// Where the orbits end, as a share of the room (leaves space for labels).
const OUTER: f64 = 0.80;
/// Radians per second at the innermost orbit; outer ones are slower.
const SPEED: f64 = 0.42;

/// Where one agent's star is, in dots from the canvas's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    pub x: i32,
    pub y: i32,
    /// Orbit radii in dots.
    pub rx: f64,
    pub ry: f64,
    /// Angle on the orbit, radians.
    pub angle: f64,
}

/// The star of `agent` (number `index` of `count`) on a canvas `w` × `h`
/// dots at animation time `clock_ms`.
pub fn star(agent: &Agent, index: usize, count: usize, w: i32, h: i32, clock_ms: u64) -> Star {
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let reach = INNER + (OUTER - INNER) * agent.context.clamp(0.0, 1.0);
    let (rx, ry) = (cx * reach, cy * reach);
    let spread = TAU * index as f64 / count.max(1) as f64;
    let start = spread + agent.hue * 0.9;
    let speed = SPEED * (1.0 - 0.7 * reach);
    let angle = start + speed * clock_ms as f64 / 1000.0;
    Star {
        x: (cx + rx * angle.cos()).round() as i32,
        y: (cy + ry * angle.sin()).round() as i32,
        rx,
        ry,
        angle,
    }
}

/// A braille canvas that remembers the colour of the last dot set in each cell.
struct Colored {
    grid: BrailleGrid,
    colors: Vec<Option<Color>>,
}

impl Colored {
    fn new(w: u16, h: u16) -> Colored {
        Colored {
            grid: BrailleGrid::new(w, h),
            colors: vec![None; usize::from(w) * usize::from(h)],
        }
    }

    fn set(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.grid.dot_width() || y >= self.grid.dot_height() {
            return;
        }
        self.grid.set(x, y);
        let i = (y / 4) as usize * usize::from(self.grid.width()) + (x / 2) as usize;
        if let Some(slot) = self.colors.get_mut(i) {
            *slot = Some(color);
        }
    }

    fn ring(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, step_dots: f64, color: Color) {
        let n = ((TAU * rx.max(ry)) / step_dots).ceil().max(8.0) as usize;
        for k in 0..n {
            let a = TAU * k as f64 / n as f64;
            self.set(
                (cx + rx * a.cos()).round() as i32,
                (cy + ry * a.sin()).round() as i32,
                color,
            );
        }
    }

    fn draw(&self, area: Rect, buf: &mut Buffer) {
        let w = usize::from(self.grid.width());
        self.grid.render_with(area, buf, |cx, cy| {
            self.colors
                .get(usize::from(cy) * w + usize::from(cx))
                .copied()
                .flatten()
                .unwrap_or(Color::Reset)
        });
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let show = &app.showcase;
    let chrome = Chrome::new(app, "constellation");
    let (head, body, foot) = showcase_chrome::split(area);
    showcase_chrome::header(frame, head, app, &chrome);
    if body.width >= 8 && body.height >= 4 {
        scene(frame.buffer_mut(), body, t, show, app.look.animated());
    }
    showcase_chrome::dim_when_frozen(frame.buffer_mut(), body, t, show);
    showcase_chrome::footer(frame, foot, app, &chrome);
}

/// Draws the scene into `area` of `buf`.
fn scene(buf: &mut Buffer, area: Rect, t: &Theme, show: &Showcase, animated: bool) {
    let mut canvas = Colored::new(area.width, area.height);
    let (w, h) = (canvas.grid.dot_width(), canvas.grid.dot_height());
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let n = show.agents.len();
    let faint = mix(t.bg, t.dim, 0.7);
    let clock = show.clock_ms;
    let working = show.agents.iter().any(|a| a.phase == Phase::Working);
    let stars: Vec<Star> = show
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| star(a, i, n, w, h, clock))
        .collect();

    // Orbits.
    for (a, s) in show.agents.iter().zip(&stars) {
        let c = if a.phase == Phase::Working {
            mix(t.bg, t.dim, 1.0)
        } else {
            faint
        };
        canvas.ring(cx, cy, s.rx, s.ry, 3.0, c);
    }
    // Tails: the more recent tool calls, the longer and brighter the trail.
    for (a, s) in show.agents.iter().zip(&stars) {
        let power: f64 = a
            .comets
            .iter()
            .map(|age| 1.0 - (*age as f64 / COMET_MS as f64).clamp(0.0, 1.0))
            .sum();
        if power <= 0.0 {
            continue;
        }
        let hue = t.chart_at(a.hue);
        let length = (0.14 + 0.18 * power).min(1.1);
        let steps = (length * s.rx.max(s.ry) / 1.5) as usize;
        for j in 1..=steps {
            let along = j as f64 / steps.max(1) as f64;
            let angle = s.angle - length * along;
            let fade = (1.0 - along).powf(1.4) * (0.45 + 0.55 * power.min(1.0));
            canvas.set(
                (cx + s.rx * angle.cos()).round() as i32,
                (cy + s.ry * angle.sin()).round() as i32,
                mix(t.bg, hue, 0.12 + 0.88 * fade),
            );
        }
    }
    // Flares and novas.
    for (a, s) in show.agents.iter().zip(&stars) {
        match (a.phase, a.nova) {
            (Phase::Waiting, _) => flare(&mut canvas, s, t.warn, clock, animated),
            (_, Some(age)) => nova(&mut canvas, s, t, a, age),
            _ => {}
        }
    }
    // The core.
    let halo = if working && animated {
        5.0 + 1.5 * (clock as f64 / 450.0).sin()
    } else {
        5.0
    };
    let core = if working { t.title } else { t.muted };
    canvas.ring(cx, cy, halo, halo * 0.9, 2.0, mix(t.bg, core, 0.5));
    canvas.draw(area, buf);

    // Glyphs and labels go on top of the dots.
    let place = |buf: &mut Buffer, x: i32, y: i32, text: &str, style: Style| {
        let (px, py) = (x.div_euclid(2), y.div_euclid(4));
        if px < 0 || py < 0 {
            return;
        }
        let (px, py) = (area.x + px as u16, area.y + py as u16);
        if px < area.right() && py < area.bottom() {
            let room = usize::from(area.right() - px);
            buf.set_stringn(px, py, text, room, style);
        }
    };
    place(
        buf,
        cx as i32,
        cy as i32,
        "◉",
        Style::new().fg(core).add_modifier(Modifier::BOLD),
    );
    place(
        buf,
        cx as i32 - 6,
        cy as i32 + 9,
        "swarmux",
        Style::new().fg(t.dim),
    );
    for (a, s) in show.agents.iter().zip(&stars) {
        let hue = t.chart_at(a.hue);
        let color = mix(t.bg, hue, 0.2 + 0.8 * a.brightness);
        let (glyph, style) = match a.phase {
            Phase::Waiting => ("✦", Style::new().fg(t.warn).add_modifier(Modifier::BOLD)),
            Phase::Finished if a.nova.is_some() => {
                ("✦", Style::new().fg(t.ok).add_modifier(Modifier::BOLD))
            }
            Phase::Finished => ("✓", Style::new().fg(mix(t.bg, t.ok, 0.45))),
            Phase::Failed => ("✗", Style::new().fg(mix(t.bg, t.error, 0.6))),
            Phase::Idle => ("·", Style::new().fg(color)),
            Phase::Working => ("●", Style::new().fg(color).add_modifier(Modifier::BOLD)),
        };
        place(buf, s.x, s.y, glyph, style);
        let name = super::dashboard::fit(&a.name, 16);
        let note = match a.phase {
            Phase::Waiting => "  needs you".to_string(),
            Phase::Working | Phase::Idle => format!("  {:.0}%", a.context * 100.0),
            Phase::Finished | Phase::Failed => String::new(),
        };
        let label = format!("{name}{note}");
        let width = label.chars().count() as i32;
        let right = s.x >= w / 2;
        let at = if right { s.x + 4 } else { s.x - 4 - width * 2 };
        let at = at.clamp(0, (w - width * 2).max(0));
        let tone = match a.phase {
            Phase::Waiting => t.warn,
            Phase::Working => mix(t.bg, hue, 0.55 + 0.45 * a.brightness),
            _ => t.dim,
        };
        place(buf, at, s.y, &label, Style::new().fg(tone));
    }
    if show.agents.is_empty() {
        place(
            buf,
            cx as i32 - 22,
            cy as i32 - 10,
            "no agents yet: press 6, then n to start a session",
            Style::new().fg(t.muted),
        );
    }
}

/// Rays around a star that wants the user, pulsing with the clock.
fn flare(canvas: &mut Colored, s: &Star, color: Color, clock: u64, animated: bool) {
    let pulse = if animated {
        0.5 + 0.5 * (clock as f64 / 260.0).sin().abs()
    } else {
        0.7
    };
    let length = 5.0 + 9.0 * pulse;
    for ray in 0..8 {
        let a = TAU * f64::from(ray) / 8.0 + 0.4;
        let reach = if ray % 2 == 0 { length } else { length * 0.55 };
        for step in 2..=(reach as i32) {
            canvas.set(
                s.x + (f64::from(step) * a.cos()).round() as i32,
                s.y + (f64::from(step) * a.sin() * 0.9).round() as i32,
                color,
            );
        }
    }
}

/// A ring that grows from the star and fades from white-hot to the agent's hue.
fn nova(canvas: &mut Colored, s: &Star, t: &Theme, a: &Agent, age: u64) {
    let p = (age as f64 / NOVA_MS as f64).clamp(0.0, 1.0);
    let radius = 3.0 + 26.0 * p;
    let hot = mix(t.title, t.chart_at(a.hue), p);
    let color = mix(hot, t.bg, p * 0.8);
    canvas.ring(
        f64::from(s.x),
        f64::from(s.y),
        radius,
        radius * 0.9,
        2.5,
        color,
    );
    if p < 0.5 {
        canvas.ring(
            f64::from(s.x),
            f64::from(s.y),
            radius * 0.55,
            radius * 0.5,
            2.0,
            hot,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::TileState;
    use crate::showcase::fixtures::{input, refresh, tile};

    fn agent(context: f64, hue: f64) -> Agent {
        Agent {
            session: crate::showcase::fixtures::tile(0, "a", TileState::Running, 0.0, 0.0).session,
            name: "a".into(),
            hue,
            context,
            rate_tps: 0.0,
            brightness: 0.5,
            phase: Phase::Working,
            comets: Vec::new(),
            nova: None,
            up: Vec::new(),
            down: Vec::new(),
            peak_up: Vec::new(),
            peak_down: Vec::new(),
        }
    }

    #[test]
    fn radius_is_context_fill() {
        let small = star(&agent(0.1, 0.0), 0, 1, 320, 184, 0);
        let big = star(&agent(0.9, 0.0), 0, 1, 320, 184, 0);
        assert!(big.rx > small.rx * 2.0 && big.ry > small.ry * 2.0);
        let dist = |s: &Star| {
            let (dx, dy) = (f64::from(s.x) - 160.0, f64::from(s.y) - 92.0);
            (dx / s.rx).hypot(dy / s.ry)
        };
        assert!((dist(&small) - 1.0).abs() < 0.05, "on its orbit");
        assert!((dist(&big) - 1.0).abs() < 0.05);
    }

    #[test]
    fn stars_move_with_the_clock_and_spread_by_index() {
        let a = agent(0.5, 0.2);
        assert_ne!(
            (star(&a, 0, 4, 320, 184, 0).x, star(&a, 0, 4, 320, 184, 0).y),
            (
                star(&a, 0, 4, 320, 184, 4_000).x,
                star(&a, 0, 4, 320, 184, 4_000).y
            )
        );
        assert_ne!(
            star(&a, 0, 4, 320, 184, 0).angle,
            star(&a, 1, 4, 320, 184, 0).angle
        );
    }

    #[test]
    fn every_size_renders_without_panicking() {
        let mut show = Showcase::default();
        show.refresh(refresh(
            vec![
                input(
                    tile(0, "api", TileState::Running, 0.9, 9_000.0),
                    &[100, 600],
                ),
                input(tile(1, "web", TileState::Waiting, 0.3, 100.0), &[]),
                input(tile(2, "doc", TileState::Done, 0.5, 0.0), &[]),
            ],
            100,
        ));
        let t = crate::theme::REFERENCE;
        for w in [0u16, 1, 5, 8, 20, 80, 200] {
            for h in [0u16, 1, 3, 4, 10, 24, 60] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                if w >= 8 && h >= 4 {
                    scene(&mut buf, area, &t, &show, true);
                }
            }
        }
    }
}
