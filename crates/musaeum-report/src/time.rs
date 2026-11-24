//! Dates as the reports show them, worked out from Unix milliseconds and an offset from UTC, with
//! no clock and no time zone database: the same instant gives the same text everywhere.

use musaeum_i18n::tr_in;

use crate::Locale;

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

/// The date of a day counted from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A moment on a wall clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub millisecond: i64,
    /// Minutes ahead of UTC.
    pub offset: i32,
}

/// The wall clock `offset` minutes ahead of UTC at Unix milliseconds `ms`.
pub fn moment(ms: i64, offset: i32) -> Moment {
    let local = ms.saturating_add(i64::from(offset) * 60_000);
    let days = local.div_euclid(86_400_000);
    let of_day = local.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    Moment {
        year,
        month,
        day,
        hour: of_day / 3_600_000,
        minute: of_day / 60_000 % 60,
        second: of_day / 1000 % 60,
        millisecond: of_day % 1000,
        offset,
    }
}

/// Unix milliseconds of a moment.
pub fn to_ms(moment: &Moment) -> i64 {
    let days = days_from_civil(moment.year, moment.month, moment.day);
    days * 86_400_000
        + moment.hour * 3_600_000
        + moment.minute * 60_000
        + moment.second * 1000
        + moment.millisecond
        - i64::from(moment.offset) * 60_000
}

fn offset_parts(offset: i32) -> (char, i32, i32) {
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.unsigned_abs() as i32;
    (sign, minutes / 60, minutes % 60)
}

/// `23/11/2026` in Portuguese, `2026-11-23` in English.
pub fn date(ms: i64, offset: i32, locale: Locale) -> String {
    let at = moment(ms, offset);
    tr_in!(
        locale,
        ReportDate,
        day = format!("{:02}", at.day),
        month = format!("{:02}", at.month),
        year = format!("{:04}", at.year)
    )
}

/// `23/11/2026 às 17:40` in Portuguese, `2026-11-23 at 17:40` in English.
pub fn date_time(ms: i64, offset: i32, locale: Locale) -> String {
    let at = moment(ms, offset);
    tr_in!(
        locale,
        ReportDateTime,
        day = format!("{:02}", at.day),
        month = format!("{:02}", at.month),
        year = format!("{:04}", at.year),
        hour = format!("{:02}", at.hour),
        minute = format!("{:02}", at.minute)
    )
}

/// `UTC−03:00`, with a true minus sign.
pub fn offset_label(offset: i32) -> String {
    let (sign, hours, minutes) = offset_parts(offset);
    let sign = if sign == '-' { '\u{2212}' } else { '+' };
    format!("UTC{sign}{hours:02}:{minutes:02}")
}

/// `2026-11-23T17:40:00-03:00`, ISO 8601 with the offset.
pub fn iso_local(ms: i64, offset: i32) -> String {
    let at = moment(ms, offset);
    let (sign, hours, minutes) = offset_parts(offset);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{hours:02}:{minutes:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

/// `D:20261123174000-03'00'`, a PDF date (ISO 32000-1, 7.9.4).
pub fn pdf_date(ms: i64, offset: i32) -> String {
    let at = moment(ms, offset);
    let (sign, hours, minutes) = offset_parts(offset);
    let zone = if offset == 0 {
        "Z".to_owned()
    } else {
        format!("{sign}{hours:02}'{minutes:02}'")
    };
    format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}{zone}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_written_on_the_person_s_clock() {
        // 2026-11-23 20:40:05.123 UTC.
        let ms = 1_795_466_405_123;
        assert_eq!(date(ms, -180, Locale::PtBr), "23/11/2026");
        assert_eq!(date_time(ms, -180, Locale::PtBr), "23/11/2026 às 17:40");
        assert_eq!(date(ms, -180, Locale::English), "2026-11-23");
        assert_eq!(date_time(ms, -180, Locale::English), "2026-11-23 at 17:40");
        assert_eq!(iso_local(ms, -180), "2026-11-23T17:40:05-03:00");
        assert_eq!(pdf_date(ms, -180), "D:20261123174005-03'00'");
        assert_eq!(pdf_date(ms, 0), "D:20261123204005Z");
        assert_eq!(offset_label(-180), "UTC\u{2212}03:00");
        assert_eq!(offset_label(330), "UTC+05:30");
        assert_eq!(
            date(ms, 240, Locale::PtBr),
            "24/11/2026",
            "past midnight four hours ahead"
        );
    }

    #[test]
    fn moments_go_and_come_back() {
        for ms in [
            0,
            -1,
            951_782_400_000,
            1_795_466_405_123,
            -62_135_596_800_000,
        ] {
            for offset in [-720, -180, 0, 345, 840] {
                assert_eq!(to_ms(&moment(ms, offset)), ms, "{ms} {offset}");
            }
        }
        let leap = moment(951_782_400_000, 0);
        assert_eq!((leap.year, leap.month, leap.day), (2000, 2, 29));
    }
}
