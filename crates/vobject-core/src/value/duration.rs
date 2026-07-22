//! DURATION (RFC 5545 §3.3.6) and PERIOD (§3.3.9) value types.

use std::fmt;

use super::datetime::DateTime;
use super::ValueError;

/// An RFC 5545 duration. The RFC's grammar allows either weeks alone or a
/// day/time combination; real-world data freely mixes them, so the model
/// stores all fields and the strict grammar is enforced only at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Duration {
    pub negative: bool,
    pub weeks: u64,
    pub days: u64,
    pub hours: u64,
    pub minutes: u64,
    pub seconds: u64,
}

impl Duration {
    pub fn from_seconds(total: i64) -> Duration {
        let negative = total < 0;
        let mut rest = total.unsigned_abs();
        let seconds = rest % 60;
        rest /= 60;
        let minutes = rest % 60;
        rest /= 60;
        let hours = rest % 24;
        rest /= 24;
        Duration {
            negative,
            weeks: 0,
            days: rest,
            hours,
            minutes,
            seconds,
        }
    }

    /// Total seconds, or `None` if the magnitude overflows `i64`. `parse`
    /// guarantees this is `Some` for any parsed duration.
    pub fn checked_total_seconds(&self) -> Option<i64> {
        let magnitude = self
            .weeks
            .checked_mul(7 * 86400)?
            .checked_add(self.days.checked_mul(86400)?)?
            .checked_add(self.hours.checked_mul(3600)?)?
            .checked_add(self.minutes.checked_mul(60)?)?
            .checked_add(self.seconds)?;
        let magnitude = i64::try_from(magnitude).ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }

    /// Total seconds, saturating at `i64::MIN`/`i64::MAX` for directly
    /// constructed values whose magnitude overflows (parsed durations never
    /// do — see [`Duration::parse`]).
    pub fn total_seconds(&self) -> i64 {
        self.checked_total_seconds().unwrap_or(if self.negative {
            i64::MIN
        } else {
            i64::MAX
        })
    }

    /// Parse `[+/-] "P" (weeks / date-time parts)`.
    pub fn parse(s: &str) -> Result<Duration, ValueError> {
        let err = || ValueError::new(format!("invalid DURATION {s:?}"));
        let mut rest = s;
        let negative = match rest.as_bytes().first() {
            Some(b'-') => {
                rest = &rest[1..];
                true
            }
            Some(b'+') => {
                rest = &rest[1..];
                false
            }
            _ => false,
        };
        rest = rest.strip_prefix(['P', 'p']).ok_or_else(err)?;

        let mut duration = Duration {
            negative,
            ..Duration::default()
        };
        let mut in_time = false;
        let mut saw_component = false;
        let mut time_components = 0u32;
        let mut number = String::new();
        for c in rest.chars() {
            match c {
                '0'..='9' => number.push(c),
                'T' | 't' => {
                    if in_time || !number.is_empty() {
                        return Err(err());
                    }
                    in_time = true;
                }
                _ => {
                    let value: u64 = number.parse().map_err(|_| err())?;
                    number.clear();
                    saw_component = true;
                    match (c.to_ascii_uppercase(), in_time) {
                        ('W', false) => duration.weeks = value,
                        ('D', false) => duration.days = value,
                        ('H', true) => duration.hours = value,
                        ('M', true) => duration.minutes = value,
                        ('S', true) => duration.seconds = value,
                        _ => return Err(err()),
                    }
                    if in_time {
                        time_components += 1;
                    }
                }
            }
        }
        if !number.is_empty() || !saw_component || (in_time && time_components == 0) {
            return Err(err());
        }
        // Reject anything whose total magnitude can't be represented, and
        // keep the coarse fields inside u32 for interop.
        if duration.checked_total_seconds().is_none()
            || duration.weeks > u32::MAX as u64
            || duration.days > u32::MAX as u64
        {
            return Err(err());
        }
        Ok(duration)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str("P")?;
        if self.weeks != 0 {
            write!(f, "{}W", self.weeks)?;
        }
        if self.days != 0 {
            write!(f, "{}D", self.days)?;
        }
        let has_time = self.hours != 0 || self.minutes != 0 || self.seconds != 0;
        if has_time {
            f.write_str("T")?;
            if self.hours != 0 {
                write!(f, "{}H", self.hours)?;
            }
            if self.minutes != 0 {
                write!(f, "{}M", self.minutes)?;
            }
            if self.seconds != 0 {
                write!(f, "{}S", self.seconds)?;
            }
        } else if self.weeks == 0 && self.days == 0 {
            // Zero duration: canonical form.
            f.write_str("T0S")?;
        }
        Ok(())
    }
}

/// The end of a PERIOD: an explicit end instant or a duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeriodEnd {
    End(DateTime),
    Duration(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Period {
    pub start: DateTime,
    pub end: PeriodEnd,
}

impl Period {
    pub fn parse(s: &str) -> Result<Period, ValueError> {
        let (start, end) = s
            .split_once('/')
            .ok_or_else(|| ValueError::new(format!("invalid PERIOD {s:?}")))?;
        let start = DateTime::parse(start)?;
        let end = if end.starts_with(['P', 'p', '+', '-']) {
            PeriodEnd::Duration(Duration::parse(end)?)
        } else {
            PeriodEnd::End(DateTime::parse(end)?)
        };
        Ok(Period { start, end })
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            PeriodEnd::End(e) => write!(f, "{}/{}", self.start, e),
            PeriodEnd::Duration(d) => write!(f, "{}/{}", self.start, d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_forms() {
        assert_eq!(
            Duration::parse("P15DT5H0M20S").unwrap(),
            Duration {
                negative: false,
                weeks: 0,
                days: 15,
                hours: 5,
                minutes: 0,
                seconds: 20,
            }
        );
        assert_eq!(Duration::parse("P7W").unwrap().weeks, 7);
        assert_eq!(Duration::parse("PT2H").unwrap().hours, 2);
        assert_eq!(Duration::parse("-PT30M").unwrap().total_seconds(), -1800);
        assert_eq!(Duration::parse("+P1D").unwrap().total_seconds(), 86400);
        assert_eq!(Duration::parse("PT0S").unwrap().total_seconds(), 0);
    }

    #[test]
    fn parse_rejects_garbage() {
        for bad in ["", "P", "PT", "15D", "P15X", "PT5D", "P1DT", "P-1D", "PT1H30", "Q1D"] {
            assert!(Duration::parse(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn mixed_weeks_and_days_accepted() {
        // Outside the strict RFC grammar but seen in the wild; the parser
        // accepts it and serialization is canonical.
        let d = Duration::parse("P1W2D").unwrap();
        assert_eq!(d.total_seconds(), (7 + 2) * 86400);
    }

    #[test]
    fn display_round_trip() {
        for s in ["P15DT5H20S", "P7W", "PT2H", "-PT30M", "P1D", "PT0S", "P1W2DT3H4M5S"] {
            let d = Duration::parse(s).unwrap();
            assert_eq!(Duration::parse(&d.to_string()).unwrap(), d, "{s}");
        }
        assert_eq!(Duration::default().to_string(), "PT0S");
        assert_eq!(Duration::from_seconds(-1800).to_string(), "-PT30M");
        assert_eq!(Duration::from_seconds(90061).to_string(), "P1DT1H1M1S");
    }

    #[test]
    fn period_forms() {
        let p = Period::parse("19970101T180000Z/19970102T070000Z").unwrap();
        assert!(matches!(p.end, PeriodEnd::End(_)));
        assert_eq!(p.to_string(), "19970101T180000Z/19970102T070000Z");

        let p = Period::parse("19970101T180000Z/PT5H30M").unwrap();
        assert!(matches!(p.end, PeriodEnd::Duration(_)));
        assert_eq!(p.to_string(), "19970101T180000Z/PT5H30M");

        assert!(Period::parse("19970101T180000Z").is_err());
        assert!(Period::parse("19970101/19970102T070000Z").is_err());
    }
}
