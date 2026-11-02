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
    fn before_the_epoch() {
        assert_eq!(iso_date(-1), "1969-12-31");
        assert_eq!(iso_date(-MS_PER_DAY), "1969-12-31");
    }
}
