//! Date, time, date-time, and UTC-offset value types (RFC 5545 §3.3.4,
//! §3.3.5, §3.3.12, §3.3.14).
//!
//! These are plain calendar values with no timezone database attached: a
//! [`Time`] knows only whether it is UTC-suffixed; interpretation of TZID
//! parameters is a higher layer's job.

use std::fmt;

use super::ValueError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

impl Date {
    pub fn new(year: i32, month: u8, day: u8) -> Result<Date, ValueError> {
        if !(1..=12).contains(&month) {
            return Err(ValueError::new(format!("month {month} out of range")));
        }
        if day < 1 || day > days_in_month(year, month) {
            return Err(ValueError::new(format!(
                "day {day} out of range for {year}-{month:02}"
            )));
        }
        if !(0..=9999).contains(&year) {
            return Err(ValueError::new(format!("year {year} out of range")));
        }
        Ok(Date { year, month, day })
    }

    /// Parse `YYYYMMDD`.
    pub fn parse(s: &str) -> Result<Date, ValueError> {
        if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ValueError::new(format!("invalid DATE {s:?}")));
        }
        Date::new(
            s[0..4].parse().unwrap(),
            s[4..6].parse().unwrap(),
            s[6..8].parse().unwrap(),
        )
    }

    /// Day-of-week, ISO numbering via Zeller-style computation.
    pub fn weekday(&self) -> Weekday {
        // Sakamoto's algorithm; 0 = Sunday.
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let mut y = self.year;
        if self.month < 3 {
            y -= 1;
        }
        let dow =
            (y + y / 4 - y / 100 + y / 400 + T[(self.month - 1) as usize] + self.day as i32) % 7;
        match dow.rem_euclid(7) {
            0 => Weekday::Sunday,
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            _ => Weekday::Saturday,
        }
    }

    /// Days since 0000-03-01 (an internal epoch convenient for calendar
    /// math); supports ordering and day arithmetic.
    pub fn to_ordinal(&self) -> i64 {
        // Standard civil-from-days inverse (Howard Hinnant's algorithm).
        let y = self.year as i64 - if self.month <= 2 { 1 } else { 0 };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (self.month as i64 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe
    }

    pub fn from_ordinal(days: i64) -> Result<Date, ValueError> {
        let era = if days >= 0 { days } else { days - 146096 } / 146097;
        let doe = days - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let year = y + if m <= 2 { 1 } else { 0 };
        Date::new(
            i32::try_from(year).map_err(|_| ValueError::new("date out of range"))?,
            m,
            d,
        )
    }

    pub fn add_days(&self, days: i64) -> Result<Date, ValueError> {
        Date::from_ordinal(self.to_ordinal() + days)
    }

    /// 1-based day of the year.
    pub fn day_of_year(&self) -> u16 {
        let jan1 = Date {
            year: self.year,
            month: 1,
            day: 1,
        };
        (self.to_ordinal() - jan1.to_ordinal() + 1) as u16
    }

    pub fn days_in_year(year: i32) -> u16 {
        if is_leap_year(year) {
            366
        } else {
            365
        }
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}{:02}{:02}", self.year, self.month, self.day)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];

    pub fn abbrev(&self) -> &'static str {
        match self {
            Weekday::Monday => "MO",
            Weekday::Tuesday => "TU",
            Weekday::Wednesday => "WE",
            Weekday::Thursday => "TH",
            Weekday::Friday => "FR",
            Weekday::Saturday => "SA",
            Weekday::Sunday => "SU",
        }
    }

    pub fn parse(s: &str) -> Result<Weekday, ValueError> {
        match s.to_ascii_uppercase().as_str() {
            "MO" => Ok(Weekday::Monday),
            "TU" => Ok(Weekday::Tuesday),
            "WE" => Ok(Weekday::Wednesday),
            "TH" => Ok(Weekday::Thursday),
            "FR" => Ok(Weekday::Friday),
            "SA" => Ok(Weekday::Saturday),
            "SU" => Ok(Weekday::Sunday),
            _ => Err(ValueError::new(format!("invalid weekday {s:?}"))),
        }
    }

    /// Days from `self` to `other` going forward (0-6).
    pub fn days_until(&self, other: Weekday) -> u8 {
        ((other.number0() + 7 - self.number0()) % 7) as u8
    }

    /// Monday = 0 ... Sunday = 6.
    pub fn number0(&self) -> u8 {
        match self {
            Weekday::Monday => 0,
            Weekday::Tuesday => 1,
            Weekday::Wednesday => 2,
            Weekday::Thursday => 3,
            Weekday::Friday => 4,
            Weekday::Saturday => 5,
            Weekday::Sunday => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// True if the value carried the `Z` suffix.
    pub utc: bool,
}

impl Time {
    pub fn new(hour: u8, minute: u8, second: u8, utc: bool) -> Result<Time, ValueError> {
        if hour > 23 || minute > 59 || second > 60 {
            return Err(ValueError::new(format!(
                "time {hour:02}:{minute:02}:{second:02} out of range"
            )));
        }
        Ok(Time {
            hour,
            minute,
            second,
            utc,
        })
    }

    /// Parse `HHMMSS[Z]`.
    pub fn parse(s: &str) -> Result<Time, ValueError> {
        let (digits, utc) = match s.strip_suffix(['Z', 'z']) {
            Some(d) => (d, true),
            None => (s, false),
        };
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ValueError::new(format!("invalid TIME {s:?}")));
        }
        Time::new(
            digits[0..2].parse().unwrap(),
            digits[2..4].parse().unwrap(),
            digits[4..6].parse().unwrap(),
            utc,
        )
    }

    pub fn seconds_of_day(&self) -> u32 {
        self.hour as u32 * 3600 + self.minute as u32 * 60 + self.second as u32
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}{:02}{:02}{}",
            self.hour,
            self.minute,
            self.second,
            if self.utc { "Z" } else { "" }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    pub date: Date,
    pub time: Time,
}

impl DateTime {
    /// Parse `YYYYMMDD "T" HHMMSS[Z]`.
    pub fn parse(s: &str) -> Result<DateTime, ValueError> {
        let (d, t) = s
            .split_once(['T', 't'])
            .ok_or_else(|| ValueError::new(format!("invalid DATE-TIME {s:?}")))?;
        Ok(DateTime {
            date: Date::parse(d)?,
            time: Time::parse(t)?,
        })
    }

    pub fn utc(&self) -> bool {
        self.time.utc
    }

    /// Seconds since the internal epoch, ignoring any timezone semantics.
    pub fn to_epoch_like(&self) -> i64 {
        self.date.to_ordinal() * 86400 + self.time.seconds_of_day() as i64
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}

/// A `DATE-TIME` or bare `DATE`, as several properties allow either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateOrDateTime {
    Date(Date),
    DateTime(DateTime),
}

impl DateOrDateTime {
    pub fn parse(s: &str) -> Result<DateOrDateTime, ValueError> {
        if s.contains(['T', 't']) {
            Ok(DateOrDateTime::DateTime(DateTime::parse(s)?))
        } else {
            Ok(DateOrDateTime::Date(Date::parse(s)?))
        }
    }

    pub fn date(&self) -> Date {
        match self {
            DateOrDateTime::Date(d) => *d,
            DateOrDateTime::DateTime(dt) => dt.date,
        }
    }
}

impl fmt::Display for DateOrDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateOrDateTime::Date(d) => d.fmt(f),
            DateOrDateTime::DateTime(dt) => dt.fmt(f),
        }
    }
}

/// A UTC offset: `±HHMM[SS]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcOffset {
    /// Offset in seconds east of UTC. May be negative.
    pub seconds: i32,
}

impl UtcOffset {
    pub fn parse(s: &str) -> Result<UtcOffset, ValueError> {
        let (sign, rest) = match s.as_bytes().first() {
            Some(b'+') => (1, &s[1..]),
            Some(b'-') => (-1, &s[1..]),
            _ => return Err(ValueError::new(format!("invalid UTC-OFFSET {s:?}"))),
        };
        if !(rest.len() == 4 || rest.len() == 6) || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ValueError::new(format!("invalid UTC-OFFSET {s:?}")));
        }
        let hours: i32 = rest[0..2].parse().unwrap();
        let minutes: i32 = rest[2..4].parse().unwrap();
        let seconds: i32 = if rest.len() == 6 {
            rest[4..6].parse().unwrap()
        } else {
            0
        };
        if minutes > 59 || seconds > 59 {
            return Err(ValueError::new(format!("invalid UTC-OFFSET {s:?}")));
        }
        let total = hours * 3600 + minutes * 60 + seconds;
        if sign < 0 && total == 0 {
            // RFC 5545: "-0000" is explicitly forbidden.
            return Err(ValueError::new("-0000 is not a valid UTC-OFFSET"));
        }
        Ok(UtcOffset {
            seconds: sign * total,
        })
    }
}

impl fmt::Display for UtcOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.seconds.unsigned_abs();
        let sign = if self.seconds < 0 { '-' } else { '+' };
        let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
        if s != 0 {
            write!(f, "{sign}{h:02}{m:02}{s:02}")
        } else {
            write!(f, "{sign}{h:02}{m:02}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_parse_and_display() {
        let d = Date::parse("20260722").unwrap();
        assert_eq!(d, Date::new(2026, 7, 22).unwrap());
        assert_eq!(d.to_string(), "20260722");
        assert!(Date::parse("2026072").is_err());
        assert!(Date::parse("20261322").is_err());
        assert!(Date::parse("20260230").is_err());
        assert!(Date::parse("2026072a").is_err());
    }

    #[test]
    fn leap_years() {
        assert!(Date::parse("20240229").is_ok());
        assert!(Date::parse("20250229").is_err());
        assert!(Date::parse("20000229").is_ok());
        assert!(Date::parse("19000229").is_err());
    }

    #[test]
    fn weekday_computation() {
        assert_eq!(Date::parse("20260722").unwrap().weekday(), Weekday::Wednesday);
        assert_eq!(Date::parse("20000101").unwrap().weekday(), Weekday::Saturday);
        assert_eq!(Date::parse("19700101").unwrap().weekday(), Weekday::Thursday);
        assert_eq!(Date::parse("20240229").unwrap().weekday(), Weekday::Thursday);
    }

    #[test]
    fn ordinal_round_trip() {
        for s in ["00010101", "19700101", "20000229", "20260722", "99991231"] {
            let d = Date::parse(s).unwrap();
            assert_eq!(Date::from_ordinal(d.to_ordinal()).unwrap(), d, "{s}");
        }
        let d = Date::parse("20261231").unwrap();
        assert_eq!(d.add_days(1).unwrap(), Date::parse("20270101").unwrap());
        assert_eq!(d.add_days(-365).unwrap(), Date::parse("20251231").unwrap());
    }

    #[test]
    fn day_of_year() {
        assert_eq!(Date::parse("20260101").unwrap().day_of_year(), 1);
        assert_eq!(Date::parse("20261231").unwrap().day_of_year(), 365);
        assert_eq!(Date::parse("20241231").unwrap().day_of_year(), 366);
    }

    #[test]
    fn time_parse() {
        assert_eq!(
            Time::parse("235960").unwrap(),
            Time::new(23, 59, 60, false).unwrap()
        );
        let t = Time::parse("120000Z").unwrap();
        assert!(t.utc);
        assert_eq!(t.to_string(), "120000Z");
        assert!(Time::parse("240000").is_err());
        assert!(Time::parse("12000").is_err());
        assert!(Time::parse("1200000").is_err());
    }

    #[test]
    fn datetime_parse() {
        let dt = DateTime::parse("20260722T160000Z").unwrap();
        assert!(dt.utc());
        assert_eq!(dt.to_string(), "20260722T160000Z");
        assert!(DateTime::parse("20260722").is_err());
        assert!(DateTime::parse("20260722T").is_err());
    }

    #[test]
    fn date_or_datetime() {
        assert!(matches!(
            DateOrDateTime::parse("20260722").unwrap(),
            DateOrDateTime::Date(_)
        ));
        assert!(matches!(
            DateOrDateTime::parse("20260722T120000").unwrap(),
            DateOrDateTime::DateTime(_)
        ));
    }

    #[test]
    fn utc_offset() {
        assert_eq!(UtcOffset::parse("+0100").unwrap().seconds, 3600);
        assert_eq!(UtcOffset::parse("-0500").unwrap().seconds, -18000);
        assert_eq!(UtcOffset::parse("+093030").unwrap().seconds, 34230);
        assert_eq!(UtcOffset::parse("+0000").unwrap().seconds, 0);
        assert!(UtcOffset::parse("-0000").is_err());
        assert!(UtcOffset::parse("0100").is_err());
        assert!(UtcOffset::parse("+01").is_err());
        assert!(UtcOffset::parse("+0160").is_err());
        assert_eq!(UtcOffset { seconds: 34230 }.to_string(), "+093030");
        assert_eq!(UtcOffset { seconds: -18000 }.to_string(), "-0500");
    }
}
