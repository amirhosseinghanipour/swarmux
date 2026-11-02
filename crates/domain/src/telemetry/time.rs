//! Local calendar days and short durations, with the UTC offset passed in.

use crate::{Millis, iso_date};

pub const MINUTE: Millis = 60_000;
pub const HOUR: Millis = 60 * MINUTE;
pub const DAY: Millis = 24 * HOUR;

/// Local day number (days since 1970-01-01 in local time) of `ts`.
pub fn local_day(ts: Millis, offset_min: i32) -> i64 {
    (ts + Millis::from(offset_min) * MINUTE).div_euclid(DAY)
}

/// Unix time at which local day `day` starts.
pub fn day_start(day: i64, offset_min: i32) -> Millis {
    day * DAY - Millis::from(offset_min) * MINUTE
}

/// 0 = Monday … 6 = Sunday.
pub fn weekday(day: i64) -> u32 {
    // 1970-01-01 was a Thursday.
    (day + 3).rem_euclid(7) as u32
}

/// The Monday of the week `day` is in.
pub fn week_start(day: i64) -> i64 {
    day - i64::from(weekday(day))
}

/// `YYYY-MM-DD` of a local day number.
pub fn day_date(day: i64) -> String {
    iso_date(day * DAY)
}

/// `HH:MM` local time of `ts`.
pub fn local_clock(ts: Millis, offset_min: i32) -> String {
    let m = (ts + Millis::from(offset_min) * MINUTE).rem_euclid(DAY) / MINUTE;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// A short span: `45s`, `12m`, `3h 05m`, `2d 4h`.
pub fn short_span(ms: Millis) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
    }
}

/// How long ago `then` was: `just now`, `5m ago`, `3h ago`, `2d ago`.
pub fn ago(then: Millis, now: Millis) -> String {
    let s = (now - then).max(0) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_respect_the_offset() {
        // 2026-11-02 23:30 UTC is 2026-11-03 03:00 at +03:30.
        let ts = 1_793_613_600_000 + 13 * HOUR + 30 * MINUTE;
        assert_eq!(day_date(local_day(ts, 0)), "2026-11-02");
        assert_eq!(day_date(local_day(ts, 210)), "2026-11-03");
        assert_eq!(local_clock(ts, 210), "03:00");
        assert_eq!(day_start(local_day(ts, 210), 210), ts - 3 * HOUR);
    }

    #[test]
    fn weeks_start_on_monday() {
        assert_eq!(weekday(0), 3); // Thursday
        let monday = local_day(1_793_613_600_000, 0); // 2026-11-02, a Monday
        assert_eq!(weekday(monday), 0);
        assert_eq!(week_start(monday + 6), monday);
        assert_eq!(week_start(monday + 7), monday + 7);
    }

    #[test]
    fn spans_and_ago() {
        assert_eq!(short_span(45_000), "45s");
        assert_eq!(short_span(12 * MINUTE), "12m");
        assert_eq!(short_span(3 * HOUR + 5 * MINUTE), "3h 05m");
        assert_eq!(short_span(2 * DAY + 4 * HOUR), "2d 4h");
        assert_eq!(ago(0, 30_000), "just now");
        assert_eq!(ago(0, 5 * MINUTE), "5m ago");
        assert_eq!(ago(0, 3 * HOUR), "3h ago");
        assert_eq!(ago(0, 2 * DAY), "2d ago");
    }
}
