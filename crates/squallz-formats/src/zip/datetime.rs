//! Conversions between `SystemTime` and the MS-DOS timestamps used by ZIP.
//!
//! ZIP timestamps are timezone-less local times; like most tools we treat
//! them as UTC for round-tripping. Civil-date math follows Howard Hinnant's
//! `days_from_civil` / `civil_from_days` algorithms.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zip::DateTime;

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

/// Converts a `SystemTime` to a ZIP `DateTime`. Returns `None` outside the
/// representable range (1980–2107); callers then keep the format default.
pub(super) fn to_zip_datetime(t: SystemTime) -> Option<DateTime> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = (secs / SECONDS_PER_DAY) as i64;
    let rem = secs % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        rem / SECONDS_PER_HOUR,
        (rem % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE,
        rem % SECONDS_PER_MINUTE,
    );
    DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        month,
        day,
        hour as u8,
        minute as u8,
        second as u8,
    )
    .ok()
}

/// Converts a ZIP `DateTime` to a `SystemTime`.
pub(super) fn from_zip_datetime(dt: DateTime) -> SystemTime {
    let days = days_from_civil(i64::from(dt.year()), dt.month(), dt.day());
    let secs = days * SECONDS_PER_DAY as i64
        + i64::from(dt.hour()) * SECONDS_PER_HOUR as i64
        + i64::from(dt.minute()) * SECONDS_PER_MINUTE as i64
        + i64::from(dt.second());
    // ZIP years start at 1980, so the result is always after the epoch.
    UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64)
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(mut y: i64, m: u8, d: u8) -> i64 {
    let m = i64::from(m);
    let d = i64::from(d);
    y -= i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date (year, month, day) for days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m as u8, d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_time(epoch_secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(epoch_secs)
    }

    fn zip_datetime(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> DateTime {
        DateTime::from_date_and_time(year, month, day, hour, minute, second).unwrap()
    }

    #[test]
    fn roundtrip_within_dos_resolution() {
        // 2024-05-06 07:08:10 UTC (even seconds: DOS stores seconds/2).
        let t = UNIX_EPOCH + Duration::from_secs(1_714_979_290);
        let dt = to_zip_datetime(t).unwrap();
        assert_eq!((dt.year(), dt.month(), dt.day()), (2024, 5, 6));
        let back = from_zip_datetime(dt);
        assert_eq!(back, t);
    }

    #[test]
    fn out_of_range_returns_none() {
        assert!(to_zip_datetime(UNIX_EPOCH).is_none()); // 1970 < 1980
    }

    #[test]
    fn minimum_and_maximum_zip_datetimes_are_representable() {
        let min = to_zip_datetime(system_time(315_532_800)).unwrap();
        assert_eq!(
            (
                min.year(),
                min.month(),
                min.day(),
                min.hour(),
                min.minute(),
                min.second()
            ),
            (1980, 1, 1, 0, 0, 0)
        );

        let max = to_zip_datetime(system_time(4_354_819_198)).unwrap();
        assert_eq!(
            (
                max.year(),
                max.month(),
                max.day(),
                max.hour(),
                max.minute(),
                max.second()
            ),
            (2107, 12, 31, 23, 59, 58)
        );
    }

    #[test]
    fn pre_1980_and_post_2107_times_are_rejected() {
        assert!(to_zip_datetime(system_time(315_532_798)).is_none());
        assert!(to_zip_datetime(system_time(4_354_819_200)).is_none());
    }

    #[test]
    fn zip_datetime_to_system_time_preserves_leap_day_and_maximum() {
        let leap = from_zip_datetime(zip_datetime(2024, 2, 29, 23, 59, 58));
        assert_eq!(leap, system_time(1_709_251_198));

        let max = from_zip_datetime(zip_datetime(2107, 12, 31, 23, 59, 58));
        assert_eq!(max, system_time(4_354_819_198));
    }
}
