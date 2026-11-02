//! The machine's current UTC offset, for cutting days in local time.

/// Minutes east of UTC (e.g. 210 for +03:30); 0 when it can't be read.
#[cfg(windows)]
pub fn utc_offset_minutes() -> i32 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // TIME_ZONE_ID_STANDARD / TIME_ZONE_ID_DAYLIGHT.
    const STANDARD: u32 = 1;
    const DAYLIGHT: u32 = 2;
    // SAFETY: plain-data struct the call fills in; zeroed is a valid value.
    let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `tz` is a valid, writable TIME_ZONE_INFORMATION.
    let which = unsafe { GetTimeZoneInformation(&mut tz) };
    let bias = match which {
        STANDARD => tz.Bias + tz.StandardBias,
        DAYLIGHT => tz.Bias + tz.DaylightBias,
        0 => tz.Bias,
        _ => return 0,
    };
    -bias
}

/// Minutes east of UTC from `date +%z`; 0 when it can't be read.
#[cfg(not(windows))]
pub fn utc_offset_minutes() -> i32 {
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| parse_offset(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(0)
}

/// `+0330` → 210, `-0500` → -300.
#[cfg_attr(windows, allow(dead_code))]
pub fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if rest.len() != 4 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i32 = rest[..2].parse().ok()?;
    let m: i32 = rest[2..].parse().ok()?;
    Some(sign * (h * 60 + m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_parse() {
        assert_eq!(parse_offset("+0330\n"), Some(210));
        assert_eq!(parse_offset("-0500"), Some(-300));
        assert_eq!(parse_offset("+0000"), Some(0));
        assert_eq!(parse_offset("0330"), None);
        assert_eq!(parse_offset("+33"), None);
    }

    #[test]
    fn machine_offset_is_plausible() {
        let m = utc_offset_minutes();
        assert!((-14 * 60..=14 * 60).contains(&m), "{m}");
    }
}
