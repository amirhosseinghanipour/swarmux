//! Animation state: gauges that ease toward new values and panels that
//! pulse when their value changes. Everything is driven by the frame
//! counter, so a frame is a pure function of its tick.

use std::collections::HashMap;

use super::grid::PanelId;
use super::widgets::{Easing, Pulse, Tween};

/// How long a gauge takes to reach a new value, in milliseconds.
pub const TWEEN_MS: u64 = 600;
/// How long a pulse lasts, in milliseconds.
pub const PULSE_MS: u64 = 900;

/// Tweens by key and pulses by panel.
#[derive(Debug, Default)]
pub struct Anim {
    tweens: HashMap<String, Tween>,
    pulses: HashMap<PanelId, Pulse>,
}

/// Frame-based timing for [`Anim`].
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Off in low-power mode: values jump and nothing pulses.
    pub enabled: bool,
    pub tween_frames: u64,
    pub pulse_frames: u64,
}

impl Anim {
    /// The value shown at `tick` for a gauge whose real value is `target`.
    pub fn value(&mut self, key: &str, target: f64, tick: u64, timing: Timing) -> f64 {
        let target = if target.is_finite() { target } else { 0.0 };
        if !timing.enabled {
            return target;
        }
        let tween = self
            .tweens
            .entry(key.to_string())
            .or_insert_with(|| Tween::new(target, tick).with_easing(Easing::OutCubic));
        tween.retarget(target, tick, timing.tween_frames);
        tween.value_at(tick)
    }

    /// How strongly panel `id` pulses at `tick` (0..1); a new `fingerprint`
    /// starts a pulse.
    pub fn pulse(&mut self, id: PanelId, fingerprint: u64, tick: u64, timing: Timing) -> f64 {
        if !timing.enabled {
            return 0.0;
        }
        let pulse = self
            .pulses
            .entry(id)
            .or_insert_with(|| Pulse::new(timing.pulse_frames));
        pulse.observe(fingerprint, tick);
        pulse.intensity(tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Timing = Timing {
        enabled: true,
        tween_frames: 6,
        pulse_frames: 4,
    };
    const OFF: Timing = Timing {
        enabled: false,
        ..ON
    };

    #[test]
    fn gauges_ease_toward_new_values() {
        let mut a = Anim::default();
        assert_eq!(
            a.value("ctx", 0.2, 0, ON),
            0.2,
            "first value is shown as is"
        );
        let mid = a.value("ctx", 0.8, 10, ON);
        assert_eq!(mid, 0.2, "the tween starts where it was");
        let later = a.value("ctx", 0.8, 13, ON);
        assert!(later > 0.2 && later < 0.8, "{later}");
        assert!((a.value("ctx", 0.8, 16, ON) - 0.8).abs() < 1e-9);
        assert_eq!(a.value("ctx", f64::NAN, 17, OFF), 0.0);
        assert_eq!(a.value("x", 0.5, 0, OFF), 0.5);
    }

    #[test]
    fn panels_pulse_once_per_change() {
        let mut a = Anim::default();
        assert_eq!(a.pulse(PanelId::Guard, 1, 0, ON), 0.0, "no pulse at start");
        assert_eq!(a.pulse(PanelId::Guard, 1, 1, ON), 0.0);
        let hot = a.pulse(PanelId::Guard, 2, 2, ON);
        assert!(hot > 0.9, "{hot}");
        assert_eq!(a.pulse(PanelId::Guard, 2, 9, ON), 0.0, "faded");
        assert_eq!(a.pulse(PanelId::Lead, 5, 0, OFF), 0.0);
    }
}
