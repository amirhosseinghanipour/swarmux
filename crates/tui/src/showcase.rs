//! The showcase screens' state: the Constellation (`z`) and the Spectrum
//! (`S`) show the same agents, so they share one scene that is rebuilt from
//! the wall on every update.
//!
//! The scene is the only thing the renderers read. While a P1 or P2 item
//! waits in the inbox the scene is frozen: nothing in it changes until the
//! item is dealt with, so the screens stand still and say why.

use std::collections::HashMap;

use swarmux_domain::{Priority, SessionId};

use crate::mission::{Tile, TileState};
use crate::ui::motion::Motion;

/// Which showcase screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Agents orbit a core: radius is context, brightness is tokens/s.
    Constellation,
    /// Input above, output below, per agent.
    Spectrum,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self {
            Mode::Constellation => "constellation",
            Mode::Spectrum => "spectrum",
        }
    }
}

/// What an agent is doing, as far as the showcase cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Working,
    /// A tool call waits for the user.
    Waiting,
    Finished,
    Failed,
    Idle,
}

impl Phase {
    fn of(state: TileState) -> Phase {
        match state {
            TileState::Running => Phase::Working,
            TileState::Waiting => Phase::Waiting,
            TileState::Done => Phase::Finished,
            TileState::Failed => Phase::Failed,
            TileState::Queued | TileState::Idle => Phase::Idle,
        }
    }
}

/// Buckets of token flow the spectrum shows per agent (one per minute).
pub const BUCKETS: usize = 30;
/// How long a nova lasts, in milliseconds.
pub const NOVA_MS: u64 = 1_600;
/// How long a tool call keeps a tail, in milliseconds.
pub const COMET_MS: u64 = 3_000;
/// Tokens per second that count as full brightness.
const BRIGHT_AT_TPS: f64 = 500.0;
/// How far a peak cap falls per second (of full height).
const PEAK_FALL_PER_S: f64 = 0.45;

/// One agent as the showcase draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub session: SessionId,
    pub name: String,
    /// Position on the theme gradient, `0.0..1.0`; stable per name.
    pub hue: f64,
    /// Context used, `0.0..=1.0`.
    pub context: f64,
    /// Tokens per second now.
    pub rate_tps: f64,
    /// `0.0..=1.0`, from `rate_tps`.
    pub brightness: f64,
    pub phase: Phase,
    /// Milliseconds since each recent tool call, newest first.
    pub comets: Vec<u64>,
    /// Milliseconds since the agent finished, while the nova lasts.
    pub nova: Option<u64>,
    /// Prompt tokens per minute, scaled to `0.0..=1.0` across all agents.
    pub up: Vec<f64>,
    /// Output tokens per minute, scaled the same way.
    pub down: Vec<f64>,
    /// Peak caps for `up` and `down`.
    pub peak_up: Vec<f64>,
    pub peak_down: Vec<f64>,
}

/// What one agent contributes to a refresh.
#[derive(Clone, Debug)]
pub struct AgentInput {
    pub tile: Tile,
    /// Milliseconds since each recent tool call, newest first.
    pub tool_ages_ms: Vec<u64>,
    /// `(prompt, output)` tokens per minute, oldest first.
    pub io: Vec<(u64, u64)>,
}

/// The P1 or P2 item that freezes the screens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub priority: Priority,
    pub text: String,
}

/// Everything a refresh needs.
#[derive(Clone, Debug)]
pub struct Refresh {
    pub agents: Vec<AgentInput>,
    pub alert: Option<Alert>,
    pub tier: Motion,
    /// Milliseconds since the previous refresh.
    pub dt_ms: u64,
    pub spend_today: Option<f64>,
}

/// The showcase's state.
#[derive(Clone, Debug, Default)]
pub struct Showcase {
    pub mode: Option<Mode>,
    /// Animation time; advances only at full motion and while not frozen.
    pub clock_ms: u64,
    pub agents: Vec<Agent>,
    /// The item that holds the screens still.
    pub alert: Option<Alert>,
    pub spend_today: Option<f64>,
    seen: HashMap<SessionId, TileState>,
    finished_at: HashMap<SessionId, u64>,
    peaks: HashMap<SessionId, (Vec<f64>, Vec<f64>)>,
}

impl Showcase {
    /// Whether a showcase screen is up.
    pub fn active(&self) -> bool {
        self.mode.is_some()
    }

    /// Whether the screens are held still by a P1 or P2 item.
    pub fn frozen(&self) -> bool {
        self.alert.is_some()
    }

    /// Rebuilds the scene. While an alert is up the animation stands still:
    /// the clock stops and only the agents' phases follow the wall, so a
    /// star that now needs the user still flares. An empty scene is built
    /// once so there is something to hold.
    pub fn refresh(&mut self, input: Refresh) {
        self.alert = input.alert;
        let frozen = self.alert.is_some();
        if frozen && !self.agents.is_empty() {
            self.follow_phases(&input.agents);
            return;
        }
        let full = input.tier == Motion::Full;
        if full && !frozen {
            self.clock_ms = self.clock_ms.saturating_add(input.dt_ms);
        }
        self.spend_today = input.spend_today;
        let scale = flow_scale(&input.agents);
        let mut agents = Vec::with_capacity(input.agents.len());
        for a in &input.agents {
            let tile = &a.tile;
            let phase = Phase::of(tile.state);
            let was = self.seen.insert(tile.session, tile.state);
            if matches!(was, Some(TileState::Running | TileState::Waiting))
                && tile.state == TileState::Done
            {
                self.finished_at.insert(tile.session, self.clock_ms);
            }
            let nova = self
                .finished_at
                .get(&tile.session)
                .map(|at| self.clock_ms.saturating_sub(*at))
                .filter(|age| full && *age < NOVA_MS && phase == Phase::Finished);
            let rate_tps = tile.flow.last().copied().unwrap_or(0.0).max(0.0) / 60.0;
            let (up, down) = series(&a.io, scale);
            let (peak_up, peak_down) = self.hold_peaks(tile.session, &up, &down, input.dt_ms, full);
            agents.push(Agent {
                session: tile.session,
                name: tile.name.clone(),
                hue: hue_of(&tile.name),
                context: tile.context.unwrap_or(0.0).clamp(0.0, 1.0),
                rate_tps,
                brightness: brightness(rate_tps, phase),
                phase,
                comets: if full {
                    a.tool_ages_ms
                        .iter()
                        .copied()
                        .filter(|age| *age < COMET_MS)
                        .collect()
                } else {
                    Vec::new()
                },
                nova,
                up,
                down,
                peak_up,
                peak_down,
            });
        }
        let live: Vec<SessionId> = agents.iter().map(|a| a.session).collect();
        self.seen.retain(|id, _| live.contains(id));
        self.finished_at.retain(|id, _| live.contains(id));
        self.peaks.retain(|id, _| live.contains(id));
        self.agents = agents;
    }

    /// Updates the phase of the agents already shown, nothing else.
    fn follow_phases(&mut self, inputs: &[AgentInput]) {
        for agent in &mut self.agents {
            let Some(input) = inputs.iter().find(|i| i.tile.session == agent.session) else {
                continue;
            };
            agent.phase = Phase::of(input.tile.state);
            if agent.phase == Phase::Waiting {
                agent.brightness = 1.0;
            }
        }
    }

    /// Peak caps: they jump up with the bar and fall slowly, only at full
    /// motion; otherwise they sit on the bar.
    fn hold_peaks(
        &mut self,
        id: SessionId,
        up: &[f64],
        down: &[f64],
        dt_ms: u64,
        full: bool,
    ) -> (Vec<f64>, Vec<f64>) {
        let fall = PEAK_FALL_PER_S * dt_ms as f64 / 1000.0;
        let held = |old: Option<&Vec<f64>>, now: &[f64]| -> Vec<f64> {
            now.iter()
                .enumerate()
                .map(|(i, v)| {
                    let before = old.and_then(|o| o.get(i)).copied().unwrap_or(0.0);
                    if full { v.max(before - fall) } else { *v }
                })
                .collect()
        };
        let old = self.peaks.get(&id);
        let peaks = (held(old.map(|p| &p.0), up), held(old.map(|p| &p.1), down));
        self.peaks.insert(id, peaks.clone());
        peaks
    }
}

/// A stable position on the theme gradient for `name`.
pub fn hue_of(name: &str) -> f64 {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    f64::from(h % 1000) / 1000.0
}

/// `0.0..=1.0` from tokens per second: dim when idle, full at
/// [`BRIGHT_AT_TPS`] and above, on a log scale.
fn brightness(tps: f64, phase: Phase) -> f64 {
    let level = ((1.0 + tps).ln() / (1.0 + BRIGHT_AT_TPS).ln()).clamp(0.0, 1.0);
    match phase {
        Phase::Working => 0.3 + 0.7 * level,
        Phase::Waiting => 1.0,
        Phase::Finished | Phase::Failed | Phase::Idle => 0.25,
    }
}

/// The largest bucket across all agents, so bars compare with each other.
fn flow_scale(agents: &[AgentInput]) -> f64 {
    agents
        .iter()
        .flat_map(|a| a.io.iter())
        .map(|(i, o)| (*i).max(*o))
        .max()
        .unwrap_or(0) as f64
}

/// The last [`BUCKETS`] buckets as heights `0.0..=1.0` (a gentle curve, so
/// small flows still show).
fn series(io: &[(u64, u64)], scale: f64) -> (Vec<f64>, Vec<f64>) {
    let height = |v: u64| {
        if scale <= 0.0 {
            0.0
        } else {
            (v as f64 / scale).powf(0.7).clamp(0.0, 1.0)
        }
    };
    let from = io.len().saturating_sub(BUCKETS);
    let tail = io.get(from..).unwrap_or(&[]);
    (
        tail.iter().map(|(i, _)| height(*i)).collect(),
        tail.iter().map(|(_, o)| height(*o)).collect(),
    )
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Agents for render tests.
    use super::*;
    use crate::mission::Tile;

    pub(crate) fn tile(n: u128, name: &str, state: TileState, context: f64, tpm: f64) -> Tile {
        Tile {
            session: session_of(n),
            name: name.into(),
            model: "claude-sonnet-4-5".into(),
            permission_mode: "acceptEdits".into(),
            lean: false,
            state,
            task: String::new(),
            context: Some(context),
            flow: vec![tpm / 2.0, tpm],
            cost: None,
            elapsed_ms: None,
            job: None,
            todos: None,
            chips: Vec::new(),
        }
    }

    fn session_of(n: u128) -> SessionId {
        SessionId::parse(&format!("00000000-0000-4000-8000-{n:012}"))
            .expect("a well-formed session id")
    }

    pub(crate) fn input(tile: Tile, ages: &[u64]) -> AgentInput {
        let wave: Vec<(u64, u64)> = (0..BUCKETS as u64)
            .map(|i| {
                let k = (tile.name.len() as u64 + 3) * 90;
                (k * (1 + (i * 7) % 11), k * (1 + (i * 3) % 5) / 2)
            })
            .collect();
        AgentInput {
            tile,
            tool_ages_ms: ages.to_vec(),
            io: wave,
        }
    }

    pub(crate) fn refresh(agents: Vec<AgentInput>, dt_ms: u64) -> Refresh {
        Refresh {
            agents,
            alert: None,
            tier: Motion::Full,
            dt_ms,
            spend_today: Some(12.84),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn working(n: u128, name: &str) -> AgentInput {
        input(tile(n, name, TileState::Running, 0.4, 6_000.0), &[200, 900])
    }

    #[test]
    fn the_clock_runs_at_full_motion_only_and_not_while_frozen() {
        let mut s = Showcase::default();
        s.refresh(refresh(vec![working(0, "api")], 100));
        s.refresh(refresh(vec![working(0, "api")], 100));
        assert_eq!(s.clock_ms, 200);
        let mut calm = refresh(vec![working(0, "api")], 100);
        calm.tier = Motion::Reduced;
        s.refresh(calm);
        assert_eq!(s.clock_ms, 200, "reduced motion keeps time still");
        assert!(s.agents[0].comets.is_empty(), "and draws no tails");
        let mut held = refresh(vec![working(0, "api")], 100);
        held.alert = Some(Alert {
            priority: Priority::P1,
            text: "approve Bash".into(),
        });
        s.refresh(held);
        assert!(s.frozen());
        assert_eq!(s.clock_ms, 200);
    }

    #[test]
    fn a_frozen_scene_keeps_what_it_had_but_follows_phases() {
        let mut s = Showcase::default();
        s.refresh(refresh(vec![working(0, "api")], 100));
        let before = s.agents[0].clone();
        let mut held = refresh(
            vec![
                input(tile(0, "api", TileState::Waiting, 0.9, 0.0), &[]),
                working(1, "web"),
            ],
            500,
        );
        held.alert = Some(Alert {
            priority: Priority::P2,
            text: "review".into(),
        });
        s.refresh(held);
        assert_eq!(s.agents.len(), 1, "new agents wait until the alert is gone");
        assert_eq!(s.agents[0].phase, Phase::Waiting, "the flare still shows");
        let agent = &s.agents[0];
        assert_eq!(
            (&agent.up, &agent.comets, agent.context),
            (&before.up, &before.comets, before.context)
        );
        s.refresh(refresh(vec![working(0, "api"), working(1, "web")], 100));
        assert!(!s.frozen());
        assert_eq!(s.agents.len(), 2);
    }

    #[test]
    fn opening_while_an_alert_waits_builds_a_still_scene() {
        let mut s = Showcase::default();
        let mut held = refresh(
            vec![input(tile(0, "api", TileState::Waiting, 0.5, 0.0), &[])],
            100,
        );
        held.alert = Some(Alert {
            priority: Priority::P1,
            text: "approve".into(),
        });
        s.refresh(held);
        assert_eq!(s.agents.len(), 1);
        assert_eq!(s.clock_ms, 0);
    }

    #[test]
    fn finishing_sets_off_a_nova_that_fades() {
        let mut s = Showcase::default();
        s.refresh(refresh(vec![working(0, "api")], 100));
        let done = |dt| {
            refresh(
                vec![input(tile(0, "api", TileState::Done, 0.4, 0.0), &[])],
                dt,
            )
        };
        s.refresh(done(100));
        assert_eq!(s.agents[0].nova, Some(0));
        s.refresh(done(NOVA_MS));
        assert_eq!(s.agents[0].nova, None, "settled into a dim check");
        assert_eq!(s.agents[0].phase, Phase::Finished);
    }

    #[test]
    fn an_agent_that_was_already_done_never_goes_nova() {
        let mut s = Showcase::default();
        s.refresh(refresh(
            vec![input(tile(0, "old", TileState::Done, 0.2, 0.0), &[])],
            100,
        ));
        assert_eq!(s.agents[0].nova, None);
    }

    #[test]
    fn brightness_follows_tokens_per_second_and_tool_calls_make_tails() {
        let mut s = Showcase::default();
        s.refresh(refresh(
            vec![
                input(tile(0, "fast", TileState::Running, 0.5, 60_000.0), &[100]),
                input(tile(1, "slow", TileState::Running, 0.5, 60.0), &[]),
                input(
                    tile(2, "old", TileState::Running, 0.5, 60.0),
                    &[COMET_MS + 1],
                ),
            ],
            100,
        ));
        assert!(s.agents[0].brightness > s.agents[1].brightness);
        assert_eq!(s.agents[0].comets, vec![100]);
        assert!(s.agents[2].comets.is_empty(), "old calls leave no tail");
        assert!((s.agents[0].rate_tps - 1_000.0).abs() < 1e-9);
    }

    #[test]
    fn peaks_hold_and_fall_but_only_at_full_motion() {
        let mut s = Showcase::default();
        s.refresh(refresh(vec![working(0, "api")], 100));
        let top = s.agents[0].peak_up.clone();
        let quiet = AgentInput {
            io: vec![(0, 0); BUCKETS],
            ..working(0, "api")
        };
        s.refresh(refresh(vec![quiet.clone()], 1_000));
        let after = &s.agents[0].peak_up;
        assert!(after.iter().zip(&top).all(|(a, b)| a <= b));
        assert!(
            after.iter().zip(&top).any(|(a, b)| a < b && *a > 0.0),
            "caps fall slowly, not at once"
        );
        let mut calm = refresh(vec![quiet], 1_000);
        calm.tier = Motion::Reduced;
        s.refresh(calm);
        assert!(s.agents[0].peak_up.iter().all(|p| *p == 0.0));
    }

    #[test]
    fn hues_are_stable_and_spread() {
        assert_eq!(hue_of("api"), hue_of("api"));
        assert_ne!(hue_of("api"), hue_of("web"));
        assert!((0.0..1.0).contains(&hue_of("anything")));
    }
}
