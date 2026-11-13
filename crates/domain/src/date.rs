//! Calendar dates from unix time, without a date library.

use crate::Millis;

const MS_PER_DAY: i64 = 86_400_000;

/// UTC date of `ms` as `YYYY-MM-DD`.
pub fn iso_date(ms: Millis) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(MS_PER_DAY));
    format!("{y:04}-{m:02}-{d:02}")
}

/// UTC date and time of `ms` as `YYYY-MM-DD HH:MM`.
pub fn iso_minute(ms: Millis) -> String {
    let minutes = ms.rem_euclid(MS_PER_DAY) / 60_000;
    format!("{} {:02}:{:02}", iso_date(ms), minutes / 60, minutes % 60)
}

/// UTC date and time of `ms` as an RFC 3339 timestamp, `YYYY-MM-DDTHH:MM:SSZ`
/// (milliseconds are dropped).
pub fn iso_utc(ms: Millis) -> String {
    let secs = ms.rem_euclid(MS_PER_DAY) / 1000;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        iso_date(ms),
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Parses an RFC 3339 timestamp that says where it is: after the seconds
/// (and a fraction) it must have `Z` or `+hh:mm` / `-hh:mm`. Unlike
/// [`parse_iso_millis`] a timestamp with no offset is refused, not read as
/// UTC: a time with no zone says nothing about when.
pub fn parse_rfc3339_millis(s: &str) -> Option<Millis> {
    let s = s.trim();
    let b = s.as_bytes();
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let has_offset = match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => b.len() == i + 6 && b[i + 3] == b':',
        _ => false,
    };
    if has_offset {
        parse_iso_millis(s)
    } else {
        None
    }
}

/// Parses an RFC 3339 UTC or offset timestamp (`2026-11-01T21:51:00.454Z`,
/// `2026-11-01T21:51:00+03:30`) into unix milliseconds.
pub fn parse_iso_millis(s: &str) -> Option<Millis> {
    let b = s.trim().as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let digits = b.get(from..from + len)?;
        digits.iter().try_fold(0i64, |acc, &d| {
            d.is_ascii_digit().then(|| acc * 10 + i64::from(d - b'0'))
        })
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, sec) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = &b[start..i];
        for k in 0..3 {
            ms = ms * 10 + frac.get(k).map_or(0, |&c| i64::from(c - b'0'));
        }
    }
    let offset_min = match b.get(i) {
        Some(b'Z' | b'z') if i + 1 == b.len() => 0,
        Some(&sign @ (b'+' | b'-')) if b.len() == i + 6 && b[i + 3] == b':' => {
            let m = num(i + 1, 2)? * 60 + num(i + 4, 2)?;
            if sign == b'-' { -m } else { m }
        }
        None => 0,
        _ => return None,
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

/// (year, month, day) to days since 1970-01-01. Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(iso_date(0), "1970-01-01");
        assert_eq!(iso_date(MS_PER_DAY - 1), "1970-01-01");
        assert_eq!(iso_date(951_782_400_000), "2000-02-29");
        assert_eq!(iso_date(1_709_164_800_000), "2024-02-29");
        assert_eq!(iso_date(1_798_761_599_000), "2026-12-31");
    }

    #[test]
    fn date_and_minute() {
        assert_eq!(iso_minute(0), "1970-01-01 00:00");
        assert_eq!(iso_minute(1_793_613_600_000 + 61_000), "2026-11-02 10:01");
        assert_eq!(iso_minute(-60_000), "1969-12-31 23:59");
    }

    #[test]
    fn utc_timestamps_round_trip() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        let ms = 1_793_613_600_000 + 61_000;
        assert_eq!(iso_utc(ms), "2026-11-02T10:01:01Z");
        assert_eq!(parse_iso_millis(&iso_utc(ms)), Some(ms));
        assert_eq!(parse_iso_millis(&iso_utc(-1000)), Some(-1000));
    }

    #[test]
    fn rfc3339_needs_an_offset() {
        let at = 1_793_613_600_000; // 2026-11-02T10:00:00Z
        for ok in [
            "2026-11-02T10:00:00Z",
            "2026-11-02T10:00:00z",
            "2026-11-02T13:30:00+03:30",
            "2026-11-02T10:00:00.250Z",
            " 2026-11-02T10:00:00Z ",
        ] {
            assert!(parse_rfc3339_millis(ok).is_some(), "{ok}");
        }
        assert_eq!(parse_rfc3339_millis("2026-11-02T10:00:00Z"), Some(at));
        assert_eq!(parse_rfc3339_millis("2026-11-02T13:30:00+03:30"), Some(at));
        for bad in [
            "",
            "2026-11-02",
            "2026-11-02T10:00:00",
            "2026-11-02T10:00:00.5",
            "2026-11-02T10:00:00+0330",
            "2026-11-02T10:00:00Zx",
            "2026-11-02 10:00",
            "tomorrow",
        ] {
            assert_eq!(parse_rfc3339_millis(bad), None, "{bad}");
        }
    }

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(parse_iso_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_millis("2026-11-01T21:51:00.454Z"),
            Some(
                1_793_613_600_000 - 86_400_000 + 21 * 3_600_000 - 10 * 3_600_000
                    + 51 * 60_000
                    + 454
            )
        );
        assert_eq!(
            parse_iso_millis("2024-02-29T00:00:00.5Z"),
            Some(1_709_164_800_500)
        );
        assert_eq!(
            parse_iso_millis("2024-02-29T03:30:00+03:30"),
            Some(1_709_164_800_000)
        );
        assert_eq!(parse_iso_millis("1969-12-31T23:59:59Z"), Some(-1000));
        for bad in [
            "",
            "2024-13-01T00:00:00Z",
            "2024-02-29",
            "x024-02-29T00:00:00Z",
            "2024-02-29T00:00:00Q",
        ] {
            assert_eq!(parse_iso_millis(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_round_trip() {
        for days in [-1000, -1, 0, 1, 11_016, 20_393, 60_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn before_the_epoch() {
        assert_eq!(iso_date(-1), "1969-12-31");
        assert_eq!(iso_date(-MS_PER_DAY), "1969-12-31");
    }
}
