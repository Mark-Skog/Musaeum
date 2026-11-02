//! Rotating copies of `library.db`, which is the one file that cannot be rebuilt.
//!
//! A copy is taken from a single read snapshot, so it is consistent however busy the writer is,
//! and it holds the writer up not at all. It gets its final name only after SQLite has checked
//! it, so whatever is called a backup here can be restored.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::error::{Error, Result, io_error};
use crate::sql::now_ms;

const PREFIX: &str = "library-";
const EXTENSION: &str = ".db";
const TEMPORARY_EXTENSION: &str = ".db.partial";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How many old copies to keep.
#[derive(Clone, Copy, Debug)]
pub struct BackupPolicy {
    /// How old the newest copy may get before another is made.
    pub interval: Duration,
    /// The newest copy of this many most recent days.
    pub keep_daily: usize,
    /// The newest copy of this many earlier weeks.
    pub keep_weekly: usize,
    /// Copies made right before a schema upgrade, which stay apart from the rest because they are
    /// the only way back to the old schema.
    pub keep_before_upgrade: usize,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(24 * 60 * 60),
            keep_daily: 7,
            keep_weekly: 4,
            keep_before_upgrade: 3,
        }
    }
}

/// A copy that exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupInfo {
    /// Where it is.
    pub path: PathBuf,
    /// When it was made, in milliseconds since the Unix epoch.
    pub created_at_ms: i64,
    /// The schema version it is from, if it was made right before an upgrade.
    pub before_upgrade_from: Option<i64>,
    /// Its size in bytes.
    pub size: u64,
}

/// `2026-11-01T12:34:56.789Z` as `20261101T123456789Z`, so that sorting names sorts by time.
fn format_timestamp(ms: i64) -> String {
    let days = ms.div_euclid(DAY_MS);
    let in_day = ms.rem_euclid(DAY_MS);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000,
    )
}

fn parse_timestamp(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let time = rest.strip_suffix('Z')?;
    if date.len() != 8 || time.len() != 9 {
        return None;
    }
    let number =
        |range: std::ops::Range<usize>, source: &str| source.get(range)?.parse::<i64>().ok();
    let days = days_from_civil(
        number(0..4, date)?,
        number(4..6, date)?,
        number(6..8, date)?,
    );
    let in_day = number(0..2, time)? * 3_600_000
        + number(2..4, time)? * 60_000
        + number(4..6, time)? * 1000
        + number(6..9, time)?;
    Some(days * DAY_MS + in_day)
}

/// Howard Hinnant's algorithm for the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * shifted_month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn file_name(created_at_ms: i64, before_upgrade_from: Option<i64>) -> String {
    let timestamp = format_timestamp(created_at_ms);
    match before_upgrade_from {
        Some(version) => format!("{PREFIX}{timestamp}-before-upgrade-from-v{version}{EXTENSION}"),
        None => format!("{PREFIX}{timestamp}{EXTENSION}"),
    }
}

fn parse_file_name(name: &str) -> Option<(i64, Option<i64>)> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(EXTENSION)?;
    match stem.split_once("-before-upgrade-from-v") {
        Some((timestamp, version)) => {
            Some((parse_timestamp(timestamp)?, Some(version.parse().ok()?)))
        }
        None => Some((parse_timestamp(stem)?, None)),
    }
}

/// Copies `source` to a new file in `directory`, checks the copy and gives it its name.
pub(crate) fn create(
    source: &Path,
    directory: &Path,
    before_upgrade_from: Option<i64>,
) -> Result<BackupInfo> {
    fs::create_dir_all(directory).map_err(io_error("creating the backups folder"))?;
    remove_leftovers(directory);

    let created_at_ms = now_ms();
    let final_name = file_name(created_at_ms, before_upgrade_from);
    let temporary = directory.join(final_name.replace(EXTENSION, TEMPORARY_EXTENSION));
    let result = copy_and_check(source, &temporary);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let path = directory.join(final_name);
    fs::rename(&temporary, &path).map_err(io_error("naming a finished backup"))?;
    let size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    Ok(BackupInfo {
        path,
        created_at_ms,
        before_upgrade_from,
        size,
    })
}

fn copy_and_check(source: &Path, target: &Path) -> Result<()> {
    let from = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut to = Connection::open(target)?;
    {
        let backup = Backup::new(&from, &mut to)?;
        // Everything in one step, so one snapshot: a writer on another connection would make a
        // copy done in small steps start over, possibly forever.
        backup.run_to_completion(i32::MAX, Duration::from_millis(100), None)?;
    }
    check_integrity(&to)?;
    // The copy needs nothing but its one file.
    to.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

fn check_integrity(connection: &Connection) -> Result<()> {
    let verdict: String =
        connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    if verdict == "ok" {
        Ok(())
    } else {
        Err(Error::Refused(format!(
            "the copy failed its integrity check: {verdict}"
        )))
    }
}

/// A copy interrupted by a crash, which is not a backup.
fn remove_leftovers(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_partial = name.to_str().is_some_and(|name| {
            name.starts_with(PREFIX)
                && (name.ends_with(TEMPORARY_EXTENSION) || name.contains(".db.partial-"))
        });
        if is_partial {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The copies in `directory`, newest first. Files not named like a backup are not looked at.
pub(crate) fn list(directory: &Path) -> Result<Vec<BackupInfo>> {
    let mut backups = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(backups),
        Err(error) => return Err(io_error("listing the backups")(error)),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((created_at_ms, before_upgrade_from)) = name.to_str().and_then(parse_file_name)
        else {
            continue;
        };
        backups.push(BackupInfo {
            path: entry.path(),
            created_at_ms,
            before_upgrade_from,
            size: entry.metadata().map_or(0, |metadata| metadata.len()),
        });
    }
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at_ms));
    Ok(backups)
}

/// Which copies a policy keeps, out of a list that is newest first.
fn chosen_to_keep(backups: &[BackupInfo], policy: &BackupPolicy) -> HashSet<PathBuf> {
    let mut keep = HashSet::new();

    let upgrades = backups
        .iter()
        .filter(|backup| backup.before_upgrade_from.is_some());
    keep.extend(
        upgrades
            .take(policy.keep_before_upgrade)
            .map(|backup| backup.path.clone()),
    );

    let regular = || {
        backups
            .iter()
            .filter(|backup| backup.before_upgrade_from.is_none())
    };

    let mut days = HashSet::new();
    let mut oldest_kept_day = i64::MAX;
    for backup in regular() {
        let day = backup.created_at_ms.div_euclid(DAY_MS);
        if days.len() == policy.keep_daily && !days.contains(&day) {
            break;
        }
        if days.insert(day) {
            keep.insert(backup.path.clone());
            oldest_kept_day = day;
        }
    }

    let mut weeks = HashSet::new();
    for backup in regular() {
        let day = backup.created_at_ms.div_euclid(DAY_MS);
        if day >= oldest_kept_day {
            continue;
        }
        let week = day.div_euclid(7);
        if weeks.len() == policy.keep_weekly && !weeks.contains(&week) {
            break;
        }
        if weeks.insert(week) {
            keep.insert(backup.path.clone());
        }
    }
    keep
}

/// Deletes the copies `policy` does not keep, returning how many.
pub(crate) fn rotate(directory: &Path, policy: &BackupPolicy) -> Result<usize> {
    let backups = list(directory)?;
    let keep = chosen_to_keep(&backups, policy);
    let mut removed = 0;
    for backup in &backups {
        if !keep.contains(&backup.path) {
            fs::remove_file(&backup.path).map_err(io_error("removing an old backup"))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether the newest regular copy is older than `interval`.
pub(crate) fn is_due(directory: &Path, interval: Duration) -> Result<bool> {
    let backups = list(directory)?;
    let newest = backups
        .iter()
        .find(|backup| backup.before_upgrade_from.is_none());
    Ok(newest.is_none_or(|backup| now_ms() - backup.created_at_ms >= interval.as_millis() as i64))
}

/// Replaces `library` with a checked copy of `backup`. The file it replaces is set aside next to
/// the backups, not deleted, because being wrong about which one is the good one must not cost
/// anything. Nothing may have the library open while this runs.
pub(crate) fn restore(library: &Path, directory: &Path, backup: &Path) -> Result<PathBuf> {
    {
        let connection = Connection::open_with_flags(backup, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        check_integrity(&connection)?;
    }
    let set_aside = directory.join(format!("replaced-{}", format_timestamp(now_ms())));
    fs::create_dir_all(&set_aside).map_err(io_error("creating the folder for the old library"))?;
    for suffix in ["", "-wal", "-shm"] {
        let mut name = library.as_os_str().to_owned();
        name.push(suffix);
        let current = PathBuf::from(name);
        if current.exists() {
            let target = set_aside.join(current.file_name().unwrap_or_default());
            fs::rename(&current, target).map_err(io_error("setting the old library aside"))?;
        }
    }
    fs::copy(backup, library).map_err(io_error("copying the backup into place"))?;
    Ok(set_aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR_MS: i64 = 60 * 60 * 1000;

    #[test]
    fn timestamps_read_back() {
        for ms in [0, 1_761_998_096_789, 951_782_400_000, 4_102_444_799_999] {
            assert_eq!(parse_timestamp(&format_timestamp(ms)), Some(ms));
        }
        assert_eq!(format_timestamp(0), "19700101T000000000Z");
        // 2024-02-29, a leap day.
        assert_eq!(&format_timestamp(1_709_164_800_000)[..8], "20240229");
    }

    #[test]
    fn names_read_back() {
        assert_eq!(
            parse_file_name(&file_name(1_761_998_096_789, None)),
            Some((1_761_998_096_789, None))
        );
        assert_eq!(
            parse_file_name(&file_name(1_761_998_096_789, Some(3))),
            Some((1_761_998_096_789, Some(3)))
        );
        for other in ["library.db", "library-x.db", "notes.txt", "library-1.db"] {
            assert_eq!(parse_file_name(other), None, "{other}");
        }
    }

    fn backup_at(created_at_ms: i64, before_upgrade_from: Option<i64>) -> BackupInfo {
        BackupInfo {
            path: PathBuf::from(file_name(created_at_ms, before_upgrade_from)),
            created_at_ms,
            before_upgrade_from,
            size: 1,
        }
    }

    const POLICY: BackupPolicy = BackupPolicy {
        interval: Duration::from_secs(1),
        keep_daily: 3,
        keep_weekly: 2,
        keep_before_upgrade: 1,
    };

    fn kept(backups: &[BackupInfo]) -> Vec<i64> {
        let keep = chosen_to_keep(backups, &POLICY);
        backups
            .iter()
            .filter(|backup| keep.contains(&backup.path))
            .map(|backup| backup.created_at_ms)
            .collect()
    }

    #[test]
    fn the_newest_of_each_recent_day_is_kept() {
        let base = 20_000 * DAY_MS;
        let backups: Vec<_> = [
            base + 3 * DAY_MS + HOUR_MS,
            base + 3 * DAY_MS,
            base + 2 * DAY_MS + HOUR_MS,
            base + 2 * DAY_MS,
            base + DAY_MS,
        ]
        .into_iter()
        .map(|ms| backup_at(ms, None))
        .collect();
        assert_eq!(
            kept(&backups),
            [
                base + 3 * DAY_MS + HOUR_MS,
                base + 2 * DAY_MS + HOUR_MS,
                base + DAY_MS
            ]
        );
    }

    #[test]
    fn older_copies_thin_out_to_one_a_week_and_then_stop() {
        let base = 20_000 * DAY_MS;
        // One a day, for 60 days, newest first.
        let backups: Vec<_> = (0..60)
            .rev()
            .map(|day| backup_at(base + day * DAY_MS, None))
            .collect();
        let kept = kept(&backups);
        assert_eq!(kept.len(), 3 + 2);
        assert_eq!(
            &kept[..3],
            &[base + 59 * DAY_MS, base + 58 * DAY_MS, base + 57 * DAY_MS]
        );
        for pair in kept[3..].windows(2) {
            assert!(pair[0] - pair[1] >= DAY_MS, "different weeks");
        }
        assert!(kept[3] < base + 57 * DAY_MS);
    }

    #[test]
    fn copies_from_before_an_upgrade_rotate_apart_from_the_others() {
        let base = 20_000 * DAY_MS;
        let backups = vec![
            backup_at(base + 2 * DAY_MS, Some(2)),
            backup_at(base + DAY_MS, Some(1)),
            backup_at(base, None),
        ];
        assert_eq!(kept(&backups), [base + 2 * DAY_MS, base]);
    }
}
