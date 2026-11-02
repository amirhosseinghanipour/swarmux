//! Battery charge, where the platform reports one.

use swarmux_application::Battery;

/// The battery's state; `None` on machines without one or where it can't be read.
#[cfg(windows)]
pub fn read() -> Option<Battery> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: plain-data struct the call fills in; zeroed is a valid value.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `s` is a valid, writable SYSTEM_POWER_STATUS.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
        return None;
    }
    from_power_status(
        s.ACLineStatus,
        s.BatteryFlag,
        s.BatteryLifePercent,
        s.BatteryLifeTime,
    )
}

/// Windows `SYSTEM_POWER_STATUS` fields to a battery reading.
#[cfg_attr(not(windows), allow(dead_code))]
fn from_power_status(ac: u8, flag: u8, percent: u8, secs_left: u32) -> Option<Battery> {
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    const CHARGING: u8 = 8;
    if flag == UNKNOWN || flag & NO_BATTERY != 0 || percent == UNKNOWN {
        return None;
    }
    Some(Battery {
        percent: f64::from(percent.min(100)),
        charging: flag & CHARGING != 0,
        plugged_in: ac == 1,
        secs_left: (secs_left != u32::MAX).then_some(u64::from(secs_left)),
    })
}

/// The first battery under `/sys/class/power_supply`.
#[cfg(target_os = "linux")]
pub fn read() -> Option<Battery> {
    let root = std::path::Path::new("/sys/class/power_supply");
    let mut plugged_in = false;
    let mut battery = None;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let dir = entry.path();
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .ok()
                .map(|s| s.trim().to_string())
        };
        match read("type").as_deref() {
            Some("Mains") => plugged_in |= read("online").as_deref() == Some("1"),
            Some("Battery") if battery.is_none() => {
                battery = from_sysfs(read("capacity").as_deref(), read("status").as_deref());
            }
            _ => {}
        }
    }
    battery.map(|mut b| {
        b.plugged_in = plugged_in || b.charging;
        b
    })
}

/// `capacity` and `status` files of a Linux battery.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn from_sysfs(capacity: Option<&str>, status: Option<&str>) -> Option<Battery> {
    let percent: f64 = capacity?.parse().ok()?;
    let charging = matches!(status, Some("Charging") | Some("Full"));
    Some(Battery {
        percent: percent.clamp(0.0, 100.0),
        charging,
        plugged_in: charging,
        secs_left: None,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn read() -> Option<Battery> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_power_status() {
        let b = from_power_status(1, 8, 80, u32::MAX).unwrap();
        assert_eq!(
            (b.percent, b.charging, b.plugged_in, b.secs_left),
            (80.0, true, true, None)
        );
        let b = from_power_status(0, 1, 55, 3600).unwrap();
        assert_eq!(
            (b.charging, b.plugged_in, b.secs_left),
            (false, false, Some(3600))
        );
        assert_eq!(from_power_status(1, 128, 255, 0), None);
        assert_eq!(from_power_status(1, 255, 50, 0), None);
    }

    #[test]
    fn linux_sysfs() {
        let b = from_sysfs(Some("42"), Some("Discharging")).unwrap();
        assert_eq!((b.percent, b.charging), (42.0, false));
        assert!(from_sysfs(Some("100"), Some("Full")).unwrap().charging);
        assert_eq!(from_sysfs(None, Some("Full")), None);
        assert_eq!(from_sysfs(Some("x"), None), None);
    }

    #[test]
    fn reading_never_panics() {
        let _ = read();
    }
}
