//! RECUR value type (RFC 5545 §3.3.10): parsing and serialization of
//! recurrence rules. Occurrence expansion lives in [`crate::rrule`].

use std::fmt;

use super::datetime::{Date, DateTime, Weekday};
use super::ValueError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Frequency {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Frequency {
    pub fn parse(s: &str) -> Result<Frequency, ValueError> {
        match s.to_ascii_uppercase().as_str() {
            "SECONDLY" => Ok(Frequency::Secondly),
            "MINUTELY" => Ok(Frequency::Minutely),
            "HOURLY" => Ok(Frequency::Hourly),
            "DAILY" => Ok(Frequency::Daily),
            "WEEKLY" => Ok(Frequency::Weekly),
            "MONTHLY" => Ok(Frequency::Monthly),
            "YEARLY" => Ok(Frequency::Yearly),
            _ => Err(ValueError::new(format!("invalid FREQ {s:?}"))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Frequency::Secondly => "SECONDLY",
            Frequency::Minutely => "MINUTELY",
            Frequency::Hourly => "HOURLY",
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        }
    }
}

/// A BYDAY entry: an optional ordinal (e.g. `-1` in `-1SU`, "last Sunday")
/// plus a weekday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WeekdayNum {
    pub ordinal: Option<i8>,
    pub weekday: Weekday,
}

impl WeekdayNum {
    pub fn parse(s: &str) -> Result<WeekdayNum, ValueError> {
        let split = s.len().saturating_sub(2);
        let (num, day) = s.split_at(split);
        let weekday = Weekday::parse(day)?;
        let ordinal = if num.is_empty() {
            None
        } else {
            let n: i8 = num
                .parse()
                .map_err(|_| ValueError::new(format!("invalid BYDAY {s:?}")))?;
            if n == 0 || !(-53..=53).contains(&n) {
                return Err(ValueError::new(format!("BYDAY ordinal out of range in {s:?}")));
            }
            Some(n)
        };
        Ok(WeekdayNum { ordinal, weekday })
    }
}

impl fmt::Display for WeekdayNum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(n) = self.ordinal {
            write!(f, "{n}")?;
        }
        f.write_str(self.weekday.abbrev())
    }
}

/// UNTIL is a DATE or DATE-TIME depending on DTSTART's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Until {
    Date(Date),
    DateTime(DateTime),
}

impl fmt::Display for Until {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Until::Date(d) => d.fmt(f),
            Until::DateTime(dt) => dt.fmt(f),
        }
    }
}

/// A parsed recurrence rule.
///
/// Unrecognized parts (RSCALE and other extensions, X- parts) are preserved
/// verbatim in `extra`, in their original order relative to nothing — they
/// are re-serialized after the known parts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recur {
    pub freq: Option<Frequency>,
    pub until: Option<Until>,
    pub count: Option<u64>,
    pub interval: Option<u64>,
    pub by_second: Vec<u8>,
    pub by_minute: Vec<u8>,
    pub by_hour: Vec<u8>,
    pub by_day: Vec<WeekdayNum>,
    pub by_month_day: Vec<i8>,
    pub by_year_day: Vec<i16>,
    pub by_week_no: Vec<i8>,
    pub by_month: Vec<u8>,
    pub by_set_pos: Vec<i16>,
    pub wkst: Option<Weekday>,
    pub extra: Vec<(String, String)>,
}

fn parse_int_list<T: std::str::FromStr + Copy + PartialOrd>(
    value: &str,
    min: T,
    max: T,
    allow_zero: bool,
    zero: T,
    part: &str,
) -> Result<Vec<T>, ValueError> {
    let mut out = Vec::new();
    for piece in value.split(',') {
        let piece = piece.trim();
        let n: T = piece
            .parse()
            .map_err(|_| ValueError::new(format!("invalid {part} value {piece:?}")))?;
        if n < min || n > max || (!allow_zero && n == zero) {
            return Err(ValueError::new(format!("{part} value {piece:?} out of range")));
        }
        out.push(n);
    }
    Ok(out)
}

impl Recur {
    /// The effective interval (default 1).
    pub fn interval(&self) -> u64 {
        self.interval.unwrap_or(1).max(1)
    }

    /// The effective week start (default Monday).
    pub fn week_start(&self) -> Weekday {
        self.wkst.unwrap_or(Weekday::Monday)
    }

    pub fn parse(s: &str) -> Result<Recur, ValueError> {
        let mut recur = Recur::default();
        let mut seen_freq = false;
        // Tolerate a trailing ';' (seen in the wild; icalendar does too).
        let trimmed = s.strip_suffix(';').unwrap_or(s);
        if trimmed.is_empty() {
            return Err(ValueError::new("empty RECUR value"));
        }
        for part in trimmed.split(';') {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| ValueError::new(format!("RECUR part {part:?} has no '='")))?;
            let name_upper = name.trim().to_ascii_uppercase();
            let value = value.trim();
            match name_upper.as_str() {
                "FREQ" => {
                    if seen_freq {
                        return Err(ValueError::new("duplicate FREQ"));
                    }
                    seen_freq = true;
                    recur.freq = Some(Frequency::parse(value)?);
                }
                "UNTIL" => {
                    recur.until = Some(if value.contains(['T', 't']) {
                        Until::DateTime(DateTime::parse(value)?)
                    } else {
                        Until::Date(Date::parse(value)?)
                    });
                }
                "COUNT" => {
                    recur.count = Some(
                        value
                            .parse()
                            .map_err(|_| ValueError::new(format!("invalid COUNT {value:?}")))?,
                    );
                }
                "INTERVAL" => {
                    recur.interval = Some(
                        value
                            .parse()
                            .map_err(|_| ValueError::new(format!("invalid INTERVAL {value:?}")))?,
                    );
                }
                "BYSECOND" => recur.by_second = parse_int_list(value, 0, 60, true, 0, "BYSECOND")?,
                "BYMINUTE" => recur.by_minute = parse_int_list(value, 0, 59, true, 0, "BYMINUTE")?,
                "BYHOUR" => recur.by_hour = parse_int_list(value, 0, 23, true, 0, "BYHOUR")?,
                "BYDAY" => {
                    recur.by_day = value
                        .split(',')
                        .map(|p| WeekdayNum::parse(p.trim()))
                        .collect::<Result<_, _>>()?;
                }
                "BYMONTHDAY" => {
                    recur.by_month_day = parse_int_list(value, -31, 31, false, 0, "BYMONTHDAY")?
                }
                "BYYEARDAY" => {
                    recur.by_year_day = parse_int_list(value, -366, 366, false, 0, "BYYEARDAY")?
                }
                "BYWEEKNO" => recur.by_week_no = parse_int_list(value, -53, 53, false, 0, "BYWEEKNO")?,
                "BYMONTH" => recur.by_month = parse_int_list(value, 1, 12, true, 0, "BYMONTH")?,
                "BYSETPOS" => {
                    recur.by_set_pos = parse_int_list(value, -366, 366, false, 0, "BYSETPOS")?
                }
                "WKST" => recur.wkst = Some(Weekday::parse(value)?),
                _ => recur.extra.push((name.trim().to_string(), value.to_string())),
            }
        }
        if !seen_freq {
            return Err(ValueError::new("RECUR without FREQ"));
        }
        if recur.until.is_some() && recur.count.is_some() {
            return Err(ValueError::new("RECUR with both UNTIL and COUNT"));
        }
        Ok(recur)
    }
}

fn write_list<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    name: &str,
    values: &[T],
) -> fmt::Result {
    if values.is_empty() {
        return Ok(());
    }
    write!(f, ";{name}=")?;
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            f.write_str(",")?;
        }
        write!(f, "{v}")?;
    }
    Ok(())
}

impl fmt::Display for Recur {
    /// Canonical serialization: FREQ first (Mac iCal requires it), then the
    /// RFC's own ordering.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(freq) = self.freq {
            write!(f, "FREQ={}", freq.as_str())?;
        }
        if let Some(u) = &self.until {
            write!(f, ";UNTIL={u}")?;
        }
        if let Some(c) = self.count {
            write!(f, ";COUNT={c}")?;
        }
        if let Some(i) = self.interval {
            write!(f, ";INTERVAL={i}")?;
        }
        write_list(f, "BYSECOND", &self.by_second)?;
        write_list(f, "BYMINUTE", &self.by_minute)?;
        write_list(f, "BYHOUR", &self.by_hour)?;
        write_list(f, "BYDAY", &self.by_day)?;
        write_list(f, "BYMONTHDAY", &self.by_month_day)?;
        write_list(f, "BYYEARDAY", &self.by_year_day)?;
        write_list(f, "BYWEEKNO", &self.by_week_no)?;
        write_list(f, "BYMONTH", &self.by_month)?;
        write_list(f, "BYSETPOS", &self.by_set_pos)?;
        if let Some(w) = self.wkst {
            write!(f, ";WKST={}", w.abbrev())?;
        }
        for (name, value) in &self.extra {
            write!(f, ";{name}={value}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let r = Recur::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR;UNTIL=19971224T000000Z").unwrap();
        assert_eq!(r.freq, Some(Frequency::Weekly));
        assert_eq!(r.interval(), 2);
        assert_eq!(r.by_day.len(), 3);
        assert!(matches!(r.until, Some(Until::DateTime(_))));
    }

    #[test]
    fn parse_ordinal_byday() {
        let r = Recur::parse("FREQ=MONTHLY;BYDAY=-1SU,2MO").unwrap();
        assert_eq!(
            r.by_day[0],
            WeekdayNum {
                ordinal: Some(-1),
                weekday: Weekday::Sunday
            }
        );
        assert_eq!(
            r.by_day[1],
            WeekdayNum {
                ordinal: Some(2),
                weekday: Weekday::Monday
            }
        );
    }

    #[test]
    fn parse_rejects_invalid() {
        for bad in [
            "",
            "COUNT=3",                       // no FREQ
            "FREQ=NEVER",
            "FREQ=DAILY;COUNT=x",
            "FREQ=DAILY;BYDAY=0MO",          // zero ordinal
            "FREQ=DAILY;BYMONTHDAY=0",
            "FREQ=DAILY;BYMONTH=13",
            "FREQ=DAILY;BYHOUR=24",
            "FREQ=DAILY;UNTIL=2020",         // bad date
            "FREQ=DAILY;COUNT=1;UNTIL=20200101", // both terminators
            "FREQ=DAILY;FREQ=WEEKLY",        // duplicate FREQ
            "FREQ=DAILY;NOEQUALS",
        ] {
            assert!(Recur::parse(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn until_date_form() {
        let r = Recur::parse("FREQ=DAILY;UNTIL=19971224").unwrap();
        assert!(matches!(r.until, Some(Until::Date(_))));
    }

    #[test]
    fn trailing_semicolon_tolerated() {
        assert!(Recur::parse("FREQ=DAILY;COUNT=10;").is_ok());
    }

    #[test]
    fn unknown_parts_preserved() {
        let r = Recur::parse("FREQ=MONTHLY;RSCALE=CHINESE;X-FOO=bar").unwrap();
        assert_eq!(
            r.extra,
            vec![
                ("RSCALE".to_string(), "CHINESE".to_string()),
                ("X-FOO".to_string(), "bar".to_string())
            ]
        );
        let out = r.to_string();
        assert!(out.contains("RSCALE=CHINESE"));
        assert!(out.contains("X-FOO=bar"));
    }

    #[test]
    fn display_round_trip() {
        for s in [
            "FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "FREQ=WEEKLY;COUNT=10;WKST=SU;BYDAY=TU,TH",
            "FREQ=MONTHLY;BYMONTHDAY=-3",
            "FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO",
            "FREQ=DAILY;INTERVAL=2",
            "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
        ] {
            let r = Recur::parse(s).unwrap();
            let round = Recur::parse(&r.to_string()).unwrap();
            assert_eq!(round, r, "{s} -> {r}");
        }
    }
}
