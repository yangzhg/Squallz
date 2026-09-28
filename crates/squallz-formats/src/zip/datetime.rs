//! ZIP DOS timestamps use local wall time. UTC extra fields preserve the
//! actual instant across time zones and daylight-saving transitions.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Local, TimeZone, Timelike, Utc};
use zip::extra_fields::ExtraField;
use zip::result::ZipResult;
use zip::write::FullFileOptions;
use zip::DateTime;

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NTFS_UNIX_EPOCH_TICKS: i128 = 116_444_736_000_000_000;

/// Converts a `SystemTime` to a ZIP `DateTime`. Returns `None` outside the
/// representable local range (1980–2107); UTC extra fields remain available.
pub(super) fn to_zip_datetime(t: SystemTime) -> Option<DateTime> {
    let nanos = unix_nanos(t)?;
    let utc = chrono::DateTime::<Utc>::from_timestamp(
        i64::try_from(nanos.div_euclid(NANOS_PER_SECOND)).ok()?,
        nanos.rem_euclid(NANOS_PER_SECOND) as u32,
    )?;
    // Bound the instant before local conversion, including the dates that
    // can cross into the DOS range after applying a time-zone offset.
    if !(1979..=2108).contains(&utc.year()) {
        return None;
    }
    datetime_to_zip(utc.with_timezone(&Local))
}

fn datetime_to_zip(dt: chrono::DateTime<impl TimeZone>) -> Option<DateTime> {
    DateTime::from_date_and_time(
        u16::try_from(dt.year()).ok()?,
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
    .ok()
}

pub(super) fn from_zip_datetime(dt: DateTime) -> Option<SystemTime> {
    from_zip_datetime_in_zone(dt, &Local)
}

fn from_zip_datetime_in_zone(dt: DateTime, zone: &impl TimeZone) -> Option<SystemTime> {
    // A DOS-only timestamp cannot distinguish the two occurrences of a
    // repeated DST hour. Resolve it consistently to the earlier instant;
    // nonexistent local times have no trustworthy instant to restore.
    let local = zone
        .with_ymd_and_hms(
            i32::from(dt.year()),
            u32::from(dt.month()),
            u32::from(dt.day()),
            u32::from(dt.hour()),
            u32::from(dt.minute()),
            u32::from(dt.second()),
        )
        .earliest()?;
    system_time_from_nanos(i128::from(local.timestamp()) * NANOS_PER_SECOND)
}

/// Prefer NTFS's 100 ns UTC timestamp, then Info-ZIP's signed Unix seconds,
/// then the local DOS timestamp. Extra-field order must not change precedence.
pub(super) fn modified_time<'a>(
    dos: Option<DateTime>,
    fields: impl Iterator<Item = &'a ExtraField>,
) -> Option<SystemTime> {
    let mut unix = None;
    for field in fields {
        match field {
            ExtraField::Ntfs(ntfs) if ntfs.mtime() != 0 => {
                let nanos = (i128::from(ntfs.mtime()) - NTFS_UNIX_EPOCH_TICKS) * 100;
                if let Some(time) = system_time_from_nanos(nanos) {
                    return Some(time);
                }
            }
            ExtraField::ExtendedTimestamp(timestamp) => {
                unix = timestamp.mod_time().and_then(|seconds| {
                    system_time_from_nanos(i128::from(seconds as i32) * NANOS_PER_SECOND)
                });
            }
            _ => {}
        }
    }
    unix.or_else(|| dos.and_then(from_zip_datetime))
}

/// Store UTC metadata in both local and central headers. NTFS carries the
/// subsecond precision and range; UT supports Info-ZIP's signed 32-bit range.
pub(super) fn add_timestamps(options: &mut FullFileOptions<'_>, t: SystemTime) -> ZipResult<()> {
    let Some(nanos) = unix_nanos(t) else {
        return Ok(());
    };
    if let Ok(seconds) = i32::try_from(nanos.div_euclid(NANOS_PER_SECOND)) {
        let mut data = [0u8; 5];
        data[0] = 1; // Only modification time is known.
        data[1..].copy_from_slice(&seconds.to_le_bytes());
        options.add_extra_data(0x5455, data, false)?;
    }
    if let Ok(ticks) = u64::try_from(nanos.div_euclid(100) + NTFS_UNIX_EPOCH_TICKS) {
        let mut data = [0u8; 32];
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        data[6..8].copy_from_slice(&24u16.to_le_bytes());
        data[8..16].copy_from_slice(&ticks.to_le_bytes());
        // Zero access/creation times mean unavailable, not fabricated copies.
        options.add_extra_data(0x000a, data, false)?;
    }
    Ok(())
}

fn unix_nanos(t: SystemTime) -> Option<i128> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos()).ok().map(|n| -n),
    }
}

fn system_time_from_nanos(nanos: i128) -> Option<SystemTime> {
    let magnitude = nanos.unsigned_abs();
    let duration = Duration::new(
        u64::try_from(magnitude / NANOS_PER_SECOND as u128).ok()?,
        (magnitude % NANOS_PER_SECOND as u128) as u32,
    );
    if nanos < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
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
        // The instant is stable in the host time zone, not interpreted as UTC.
        let t = UNIX_EPOCH + Duration::from_secs(1_714_979_290);
        let dt = to_zip_datetime(t).unwrap();
        let back = from_zip_datetime(dt);
        assert_eq!(back, Some(t));
    }

    #[test]
    fn out_of_range_returns_none() {
        assert!(to_zip_datetime(UNIX_EPOCH).is_none()); // 1970 < 1980
    }

    #[test]
    fn minimum_and_maximum_zip_datetimes_are_representable() {
        let min = datetime_to_zip(chrono::DateTime::<Utc>::from(system_time(315_532_800))).unwrap();
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

        let max =
            datetime_to_zip(chrono::DateTime::<Utc>::from(system_time(4_354_819_198))).unwrap();
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
        for seconds in [315_532_798, 4_354_819_200] {
            assert!(datetime_to_zip(chrono::DateTime::<Utc>::from(system_time(seconds))).is_none());
        }
    }

    #[test]
    fn zip_datetime_to_system_time_preserves_leap_day_and_maximum() {
        let leap = from_zip_datetime_in_zone(zip_datetime(2024, 2, 29, 23, 59, 58), &Utc);
        assert_eq!(leap, Some(system_time(1_709_251_198)));

        let max = from_zip_datetime_in_zone(zip_datetime(2107, 12, 31, 23, 59, 58), &Utc);
        assert_eq!(max, Some(system_time(4_354_819_198)));
    }

    #[test]
    fn dos_time_observes_east_and_west_offsets_across_dates() {
        for offset in [8 * 3600, -7 * 3600, 5 * 3600 + 45 * 60] {
            let zone = chrono::FixedOffset::east_opt(offset).unwrap();
            let instant = system_time(1_709_251_198);
            let local = chrono::DateTime::<Utc>::from(instant).with_timezone(&zone);
            let dos = datetime_to_zip(local).unwrap();
            assert_eq!(dos.hour(), local.hour() as u8);
            assert_eq!(dos.day(), local.day() as u8);
            assert_eq!(from_zip_datetime_in_zone(dos, &zone), Some(instant));
        }
    }
}
