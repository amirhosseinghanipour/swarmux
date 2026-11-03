//! A brief highlight when a watched value changes.

use ratatui::style::Color;

use crate::theme::mix;

/// Tracks a value's fingerprint and pulses for `duration` ticks after it
/// changes. The first observation never pulses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pulse {
    fingerprint: Option<u64>,
    start: Option<u64>,
    duration: u64,
}

impl Pulse {
    /// A pulse lasting `duration` ticks (0 never shows).
    pub fn new(duration: u64) -> Pulse {
        Pulse {
            fingerprint: None,
            start: None,
            duration,
        }
    }

    /// Records the value's `fingerprint` at `tick`; a change starts a pulse.
    pub fn observe(&mut self, fingerprint: u64, tick: u64) {
        match self.fingerprint {
            Some(seen) if seen != fingerprint => self.start = Some(tick),
            _ => {}
        }
        self.fingerprint = Some(fingerprint);
    }

    /// The pulse strength at `tick`: 1 when it starts, easing out to 0 at
    /// the end of `duration`.
    pub fn intensity(&self, tick: u64) -> f64 {
        let Some(start) = self.start else { return 0.0 };
        if self.duration == 0 {
            return 0.0;
        }
        let elapsed = tick.saturating_sub(start);
        if elapsed >= self.duration {
            return 0.0;
        }
        let t = elapsed as f64 / self.duration as f64;
        (1.0 - t).powi(3)
    }
}

/// `base` blended toward `highlight` by `intensity` (0..1).
pub fn pulse_color(base: Color, highlight: Color, intensity: f64) -> Color {
    mix(base, highlight, intensity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sight_is_quiet_and_changes_pulse() {
        let mut p = Pulse::new(10);
        p.observe(7, 0);
        assert_eq!(p.intensity(0), 0.0);
        p.observe(7, 3);
        assert_eq!(p.intensity(3), 0.0);
        p.observe(8, 5);
        assert_eq!(p.intensity(5), 1.0);
        assert_eq!(p.intensity(4), 1.0, "before the start counts as the start");
        let mid = p.intensity(10);
        assert!(mid > 0.0 && mid < 0.5, "eases out: {mid}");
        assert!(p.intensity(8) > p.intensity(9));
        assert_eq!(p.intensity(15), 0.0);
        assert_eq!(p.intensity(u64::MAX), 0.0);
    }

    #[test]
    fn zero_duration_never_shows() {
        let mut p = Pulse::new(0);
        p.observe(1, 0);
        p.observe(2, 1);
        assert_eq!(p.intensity(1), 0.0);
    }

    #[test]
    fn colours_blend_toward_the_highlight() {
        let base = Color::Rgb(0, 0, 0);
        let hi = Color::Rgb(200, 100, 50);
        assert_eq!(pulse_color(base, hi, 1.0), hi);
        assert_eq!(pulse_color(base, hi, 0.0), base);
        assert_eq!(pulse_color(base, hi, 0.5), Color::Rgb(100, 50, 25));
        assert_eq!(pulse_color(base, hi, f64::NAN), base);
    }

    #[test]
    fn tick_fuzz_never_panics() {
        for duration in [0, 1, 5, u64::MAX] {
            let mut p = Pulse::new(duration);
            for tick in (0..40).chain([u64::MAX - 1, u64::MAX]) {
                p.observe(tick % 3, tick);
                let i = p.intensity(tick);
                assert!((0.0..=1.0).contains(&i));
                assert!((0.0..=1.0).contains(&p.intensity(0)));
            }
        }
    }
}
