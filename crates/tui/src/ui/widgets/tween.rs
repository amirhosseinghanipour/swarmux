//! Easing and tweening driven only by an integer tick (pure, deterministic).

/// An easing curve mapping progress 0..1 to eased progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Easing {
    Linear,
    /// Fast start, gentle stop.
    #[default]
    OutCubic,
    /// Gentle start and stop.
    InOutCubic,
    /// Overshoots slightly past 1 before settling.
    OutBack,
}

impl Easing {
    /// The eased value of `t`, which is clamped to 0..=1 (NaN → 0).
    /// Every curve maps 0 → 0 and 1 → 1; `OutBack` exceeds 1 in between.
    pub fn apply(self, t: f64) -> f64 {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self {
            Easing::Linear => t,
            Easing::OutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::InOutCubic if t < 0.5 => 4.0 * t.powi(3),
            Easing::InOutCubic => 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0,
            Easing::OutBack => {
                const C1: f64 = 1.70158;
                const C3: f64 = C1 + 1.0;
                1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
            }
        }
    }
}

/// A value animating from `from` to `to` over `duration` ticks from `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    from: f64,
    to: f64,
    start: u64,
    duration: u64,
    easing: Easing,
}

/// `v` if finite, else 0.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

impl Tween {
    /// A tween settled at `value` (non-finite → 0) as of `tick`.
    pub fn new(value: f64, tick: u64) -> Tween {
        let value = finite_or_zero(value);
        Tween {
            from: value,
            to: value,
            start: tick,
            duration: 0,
            easing: Easing::default(),
        }
    }

    /// The same tween with another easing curve.
    pub fn with_easing(mut self, easing: Easing) -> Tween {
        self.easing = easing;
        self
    }

    /// Where the tween is heading.
    pub fn target(&self) -> f64 {
        self.to
    }

    /// The value at `tick`: `from` before the start, `to` once settled.
    pub fn value_at(&self, tick: u64) -> f64 {
        if self.is_settled(tick) {
            return self.to;
        }
        let elapsed = tick.saturating_sub(self.start);
        let t = elapsed as f64 / self.duration as f64;
        self.from + (self.to - self.from) * self.easing.apply(t)
    }

    /// Heads for `to` over `duration` ticks, starting from the value at
    /// `tick`. A no-op when `to` is already the target or not finite;
    /// a `duration` of 0 jumps.
    pub fn retarget(&mut self, to: f64, tick: u64, duration: u64) {
        if !to.is_finite() || to == self.to {
            return;
        }
        self.from = self.value_at(tick);
        self.to = to;
        self.start = tick;
        self.duration = duration;
    }

    /// Whether the tween has reached its target by `tick`.
    pub fn is_settled(&self, tick: u64) -> bool {
        self.duration == 0 || tick >= self.start.saturating_add(self.duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Easing; 4] = [
        Easing::Linear,
        Easing::OutCubic,
        Easing::InOutCubic,
        Easing::OutBack,
    ];

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn curves_hit_their_ends_and_clamp() {
        for e in ALL {
            assert!(close(e.apply(0.0), 0.0), "{e:?}");
            assert!(close(e.apply(1.0), 1.0), "{e:?}");
            assert!(
                close(e.apply(-3.0), 0.0) && close(e.apply(9.0), 1.0),
                "{e:?}"
            );
            assert!(close(e.apply(f64::NAN), 0.0), "{e:?}");
        }
        assert!(close(Easing::Linear.apply(0.25), 0.25));
        assert!(close(Easing::OutCubic.apply(0.5), 0.875));
        assert!(close(Easing::InOutCubic.apply(0.5), 0.5));
        assert!(close(Easing::InOutCubic.apply(0.25), 0.0625));
        assert!(Easing::OutBack.apply(0.7) > 1.0);
    }

    #[test]
    fn tweens_interpolate_and_settle() {
        let mut t = Tween::new(0.0, 10).with_easing(Easing::Linear);
        assert!(t.is_settled(10));
        t.retarget(100.0, 10, 10);
        assert_eq!(t.target(), 100.0);
        assert_eq!(t.value_at(5), 0.0);
        assert_eq!(t.value_at(10), 0.0);
        assert!(close(t.value_at(15), 50.0));
        assert!(!t.is_settled(19));
        assert_eq!(t.value_at(20), 100.0);
        assert!(t.is_settled(20));
    }

    #[test]
    fn retarget_starts_from_the_current_value() {
        let mut t = Tween::new(0.0, 0).with_easing(Easing::Linear);
        t.retarget(100.0, 0, 10);
        t.retarget(0.0, 5, 10);
        assert!(close(t.value_at(5), 50.0));
        assert!(close(t.value_at(10), 25.0));
        let before = t;
        t.retarget(0.0, 7, 99);
        assert_eq!(t, before, "same target is a no-op");
        t.retarget(f64::NAN, 7, 3);
        assert_eq!(t, before, "non-finite targets are ignored");
        t.retarget(-4.0, 8, 0);
        assert_eq!(t.value_at(8), -4.0, "duration 0 jumps");
    }

    #[test]
    fn tick_fuzz_never_panics() {
        for e in ALL {
            let mut t = Tween::new(f64::INFINITY, u64::MAX - 3).with_easing(e);
            for tick in [0, 1, u64::MAX - 2, u64::MAX] {
                t.retarget(
                    f64::from(u32::try_from(tick % 97).unwrap_or(0)),
                    tick,
                    u64::MAX,
                );
                assert!(t.value_at(tick).is_finite());
                assert!(t.value_at(0).is_finite());
                let _ = t.is_settled(u64::MAX);
            }
        }
    }
}
