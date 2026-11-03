//! Motion tiers: how much the interface moves.
//!
//! `full` runs every ambient effect, `reduced` keeps data updates and
//! spinners, `off` keeps data updates only. With `motion = "auto"` (the
//! default) full motion drops to reduced on battery, over SSH, and when
//! drawing a frame takes too long.

use std::time::Duration;

use swarmux_application::MotionSetting;

/// A frame that takes longer than this (on average) is too slow for motion.
pub const SLOW_FRAME_MS: f64 = 8.0;
/// Motion comes back once frames average under this.
const RECOVER_MS: f64 = 5.0;
/// Frames seen before the average is believed.
const WARM_UP: u32 = 20;
/// Weight of the newest frame in the average.
const SMOOTHING: f64 = 0.2;

/// The tier in force.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Motion {
    #[default]
    Full,
    Reduced,
    Off,
}

impl Motion {
    pub fn as_str(self) -> &'static str {
        match self {
            Motion::Full => "full",
            Motion::Reduced => "reduced",
            Motion::Off => "off",
        }
    }
}

/// Why `auto` chose reduced motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Ssh,
    Battery,
    SlowFrames,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Ssh => "ssh",
            Reason::Battery => "battery",
            Reason::SlowFrames => "slow frames",
        }
    }
}

/// What `auto` senses: SSH, battery power and frame time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoMotion {
    ssh: bool,
    battery: bool,
    slow: bool,
    average_ms: f64,
    seen: u32,
}

impl AutoMotion {
    /// Whether the session runs over SSH, from the environment variables
    /// `has` looks up.
    pub fn from_vars(has: impl Fn(&str) -> bool) -> AutoMotion {
        AutoMotion {
            ssh: has("SSH_CONNECTION") || has("SSH_TTY"),
            ..AutoMotion::default()
        }
    }

    /// Reads the process environment.
    pub fn from_env() -> AutoMotion {
        AutoMotion::from_vars(|name| std::env::var_os(name).is_some())
    }

    /// Whether the machine runs on battery now (`None`: unknown).
    pub fn set_battery(&mut self, on_battery: Option<bool>) {
        self.battery = on_battery.unwrap_or(false);
    }

    /// Records how long drawing a frame took. Motion is dropped once the
    /// average passes [`SLOW_FRAME_MS`] and returns when it falls well
    /// below it.
    pub fn observe_frame(&mut self, took: Duration) {
        let ms = took.as_secs_f64() * 1000.0;
        self.average_ms = if self.seen == 0 {
            ms
        } else {
            self.average_ms + (ms - self.average_ms) * SMOOTHING
        };
        self.seen = self.seen.saturating_add(1);
        if self.seen >= WARM_UP {
            if self.average_ms > SLOW_FRAME_MS {
                self.slow = true;
            } else if self.average_ms < RECOVER_MS {
                self.slow = false;
            }
        }
    }

    /// The first reason to reduce motion, if any.
    pub fn reason(&self) -> Option<Reason> {
        if self.ssh {
            Some(Reason::Ssh)
        } else if self.battery {
            Some(Reason::Battery)
        } else if self.slow {
            Some(Reason::SlowFrames)
        } else {
            None
        }
    }
}

/// The tier for `setting`. `low_power` is the older name for reduced and
/// never raises a tier; `reason` only matters when the setting is `auto`.
pub fn resolve(setting: MotionSetting, low_power: bool, reason: Option<Reason>) -> Motion {
    match setting {
        MotionSetting::Off => Motion::Off,
        MotionSetting::Reduced => Motion::Reduced,
        MotionSetting::Full if low_power => Motion::Reduced,
        MotionSetting::Full => Motion::Full,
        MotionSetting::Auto if low_power || reason.is_some() => Motion::Reduced,
        MotionSetting::Auto => Motion::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_reduces_for_ssh_battery_and_slow_frames_but_full_does_not() {
        let ssh = AutoMotion::from_vars(|k| k == "SSH_TTY");
        assert_eq!(ssh.reason(), Some(Reason::Ssh));
        let remote = AutoMotion::from_vars(|k| k == "SSH_CONNECTION");
        assert_eq!(remote.reason(), Some(Reason::Ssh));
        assert_eq!(AutoMotion::from_vars(|_| false).reason(), None);

        let mut auto = AutoMotion::default();
        auto.set_battery(Some(true));
        assert_eq!(auto.reason(), Some(Reason::Battery));
        auto.set_battery(None);
        assert_eq!(auto.reason(), None);

        assert_eq!(
            resolve(MotionSetting::Auto, false, Some(Reason::Ssh)),
            Motion::Reduced
        );
        assert_eq!(resolve(MotionSetting::Auto, false, None), Motion::Full);
        assert_eq!(
            resolve(MotionSetting::Full, false, Some(Reason::Battery)),
            Motion::Full,
            "an explicit full is kept"
        );
        assert_eq!(resolve(MotionSetting::Auto, true, None), Motion::Reduced);
        assert_eq!(
            resolve(MotionSetting::Off, false, None),
            Motion::Off,
            "off is off"
        );
        assert_eq!(
            resolve(MotionSetting::Reduced, false, None),
            Motion::Reduced
        );
    }

    #[test]
    fn slow_frames_latch_and_recover() {
        let mut auto = AutoMotion::default();
        for _ in 0..10 {
            auto.observe_frame(Duration::from_millis(30));
        }
        assert_eq!(auto.reason(), None, "not believed before the warm-up");
        for _ in 0..20 {
            auto.observe_frame(Duration::from_millis(30));
        }
        assert_eq!(auto.reason(), Some(Reason::SlowFrames));
        for _ in 0..4 {
            auto.observe_frame(Duration::from_millis(1));
        }
        assert_eq!(auto.reason(), Some(Reason::SlowFrames), "no flapping");
        for _ in 0..60 {
            auto.observe_frame(Duration::from_millis(1));
        }
        assert_eq!(auto.reason(), None);
    }
}
