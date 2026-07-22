//! RSCALE recurrence expansion (RFC 7529): recurrence evaluated in a
//! non-Gregorian calendar, with SKIP handling for instances that are
//! invalid in that calendar (nonexistent days, absent leap months).
//!
//! Calendar systems come from ICU4X (`icu_calendar`), which is pure Rust
//! with compiled-in data. Supported RSCALE values map onto CLDR calendar
//! identifiers; YEARLY and MONTHLY frequencies are evaluated natively in
//! the recurrence calendar (the only frequencies whose periods are
//! calendar-dependent — the caller routes other frequencies to the
//! Gregorian engine, whose day/week arithmetic is calendar-independent).
//!
//! SKIP semantics, validated against libical's RSCALE conformance data:
//! an overflowing day slides FORWARD to the first day after the month's
//! (or year's) end, or BACKWARD to its last day; an underflowing negative
//! day slides BACKWARD to the last day before the month start, or FORWARD
//! to its first day; an absent leap month falls BACKWARD to its regular
//! counterpart or FORWARD to the following month.

use icu_calendar::types::Month as IcuMonth;
use icu_calendar::{AnyCalendar, AnyCalendarKind, Date as IcuDate, Iso, Ref};

use crate::value::datetime::{Date, DateTime, Time};
use crate::value::recur::{Frequency, Recur, RecurMonth, Skip, Until};
use crate::value::{DateOrDateTime, ValueError};

/// The calendar for an RSCALE identifier, if supported.
pub fn calendar_for(rscale: &str) -> Option<AnyCalendar> {
    let kind = match rscale.to_ascii_uppercase().as_str() {
        "GREGORIAN" | "GREGORY" => AnyCalendarKind::Gregorian,
        "CHINESE" => AnyCalendarKind::Chinese,
        "DANGI" => AnyCalendarKind::Dangi,
        "HEBREW" => AnyCalendarKind::Hebrew,
        "ETHIOPIC" => AnyCalendarKind::Ethiopian,
        "ETHIOPIC-AMETE-ALEM" | "ETHIOAA" => AnyCalendarKind::EthiopianAmeteAlem,
        "ISLAMIC-CIVIL" | "ISLAMICC" | "ISLAMIC" => AnyCalendarKind::HijriTabularTypeIIFriday,
        "ISLAMIC-TBLA" => AnyCalendarKind::HijriTabularTypeIIThursday,
        "ISLAMIC-UMALQURA" => AnyCalendarKind::HijriUmmAlQura,
        "COPTIC" => AnyCalendarKind::Coptic,
        "INDIAN" => AnyCalendarKind::Indian,
        "PERSIAN" => AnyCalendarKind::Persian,
        "BUDDHIST" => AnyCalendarKind::Buddhist,
        "JAPANESE" => AnyCalendarKind::Japanese,
        "ROC" => AnyCalendarKind::Roc,
        _ => return None,
    };
    Some(AnyCalendar::new(kind))
}

fn to_icu_iso(d: Date) -> Result<IcuDate<Iso>, ValueError> {
    IcuDate::try_new_iso(d.year, d.month, d.day)
        .map_err(|e| ValueError::new(format!("date out of calendar range: {e}")))
}

fn from_icu_iso(d: IcuDate<Iso>) -> Result<Date, ValueError> {
    Date::new(
        d.extended_year(),
        d.month().ordinal,
        d.day_of_month().0,
    )
}

/// One month of a recurrence-calendar year.
#[derive(Debug, Clone, Copy)]
struct MonthSlot {
    month: IcuMonth,
    ordinal: u8,
    days: u8,
    /// ISO date of this month's first day.
    first_iso: i64, // Date::to_ordinal form
}

struct Engine<'c> {
    cal: Ref<'c, AnyCalendar>,
    skip: Skip,
}

impl<'c> Engine<'c> {
    /// All months of the recurrence-calendar year with the given extended
    /// year number, in order.
    fn months_of_year(&self, ext_year: i32) -> Result<Vec<MonthSlot>, ValueError> {
        let mut slots = Vec::new();
        for number in 1..=14u8 {
            for leap in [false, true] {
                let month = if leap {
                    IcuMonth::leap(number)
                } else {
                    IcuMonth::new(number)
                };
                if let Ok(date) = IcuDate::try_new(ext_year.into(), month, 1, self.cal) {
                    if date.extended_year() != ext_year {
                        continue;
                    }
                    slots.push(MonthSlot {
                        month,
                        ordinal: date.month().ordinal,
                        days: date.days_in_month(),
                        first_iso: from_icu_iso(date.to_iso())?.to_ordinal(),
                    });
                }
            }
        }
        slots.sort_by_key(|s| s.ordinal);
        if slots.is_empty() {
            return Err(ValueError::new(format!(
                "no months found in recurrence-calendar year {ext_year}"
            )));
        }
        Ok(slots)
    }

    /// Resolve a requested month within a year, applying SKIP for months
    /// the year does not contain. Returns the chosen slot, or None (OMIT).
    fn resolve_month(
        &self,
        months: &[MonthSlot],
        want: RecurMonth,
    ) -> Option<MonthSlot> {
        let found = months
            .iter()
            .find(|s| s.month.number() == want.month && s.month.is_leap() == want.leap);
        if let Some(slot) = found {
            return Some(*slot);
        }
        match self.skip {
            Skip::Omit => None,
            Skip::Backward => {
                if want.leap {
                    // The regular counterpart of an absent leap month.
                    months
                        .iter()
                        .find(|s| s.month.number() == want.month && !s.month.is_leap())
                        .copied()
                } else {
                    // A month number past the end of the year: its last month.
                    months.last().copied()
                }
            }
            Skip::Forward => {
                if want.leap {
                    // The month following where the leap month would sit.
                    months
                        .iter()
                        .find(|s| s.month.number() > want.month)
                        .copied()
                } else {
                    months.last().copied()
                }
            }
        }
    }

    /// Resolve a (possibly negative, possibly out-of-range) day within a
    /// month, applying SKIP. Returns the ISO ordinal of the instance day.
    fn resolve_day(&self, slot: MonthSlot, day: i8) -> Option<i64> {
        let target = if day > 0 {
            day as i64
        } else {
            slot.days as i64 + day as i64 + 1
        };
        if (1..=slot.days as i64).contains(&target) {
            return Some(slot.first_iso + target - 1);
        }
        match self.skip {
            Skip::Omit => None,
            Skip::Backward => {
                if target < 1 {
                    // Last day before the month starts.
                    Some(slot.first_iso - 1)
                } else {
                    // Last day of the month.
                    Some(slot.first_iso + slot.days as i64 - 1)
                }
            }
            Skip::Forward => {
                if target < 1 {
                    // First day of the month.
                    Some(slot.first_iso)
                } else {
                    // First day after the month ends.
                    Some(slot.first_iso + slot.days as i64)
                }
            }
        }
    }

    /// Resolve a (possibly negative) day-of-year with SKIP.
    fn resolve_year_day(
        &self,
        months: &[MonthSlot],
        days_in_year: i64,
        day: i16,
    ) -> Option<i64> {
        let year_start = months[0].first_iso;
        let target = if day > 0 {
            day as i64
        } else {
            days_in_year + day as i64 + 1
        };
        if (1..=days_in_year).contains(&target) {
            return Some(year_start + target - 1);
        }
        match self.skip {
            Skip::Omit => None,
            Skip::Backward => {
                if target < 1 {
                    Some(year_start - 1)
                } else {
                    Some(year_start + days_in_year - 1)
                }
            }
            Skip::Forward => {
                if target < 1 {
                    Some(year_start)
                } else {
                    Some(year_start + days_in_year)
                }
            }
        }
    }
}

/// Expand an RSCALE rule with YEARLY or MONTHLY frequency.
pub fn expand_rscale(
    recur: &Recur,
    dtstart: DateOrDateTime,
    limits: crate::rrule::ExpandLimits,
) -> Result<Vec<DateOrDateTime>, ValueError> {
    let rscale = recur
        .rscale
        .as_deref()
        .ok_or_else(|| ValueError::new("expand_rscale requires RSCALE"))?;
    let cal = calendar_for(rscale)
        .ok_or_else(|| ValueError::new(format!("unsupported RSCALE {rscale:?}")))?;
    let engine = Engine {
        cal: Ref(&cal),
        skip: recur.skip.unwrap_or_default(),
    };
    let freq = recur
        .freq
        .ok_or_else(|| ValueError::new("RECUR without FREQ"))?;

    let (start_date, time, date_only) = match dtstart {
        DateOrDateTime::Date(d) => (
            d,
            Time {
                hour: 0,
                minute: 0,
                second: 0,
                utc: false,
            },
            true,
        ),
        DateOrDateTime::DateTime(dt) => (dt.date, dt.time, false),
    };
    let start_ordinal = start_date.to_ordinal();
    let start_r = to_icu_iso(start_date)?.to_calendar(Ref(&cal));
    let start_year = start_r.extended_year();
    let start_month = RecurMonth {
        month: start_r.month().number(),
        leap: start_r.month().to_input().is_leap(),
    };
    let start_day = start_r.day_of_month().0;

    let months_wanted: Vec<RecurMonth> = if recur.by_month.is_empty() {
        vec![start_month]
    } else {
        recur.by_month.clone()
    };
    let days_wanted: Vec<i8> = if recur.by_month_day.is_empty() {
        vec![start_day as i8]
    } else {
        recur.by_month_day.clone()
    };

    let until_ordinal: Option<(i64, Option<i64>)> = recur.until.map(|u| match u {
        Until::Date(d) => (d.to_ordinal(), None),
        Until::DateTime(dt) => (dt.date.to_ordinal(), Some(dt.to_epoch_like())),
    });

    let interval = recur.interval() as i64;
    let mut out: Vec<DateOrDateTime> = Vec::new();
    let mut last_emitted: Option<i64> = None;
    let mut empty_periods = 0usize;
    let count_target = recur.count;
    let max_year = start_year + limits.max_years;

    // Period cursor.
    let mut year = start_year;
    let mut month_ordinal: i64 = {
        let months = engine.months_of_year(year)?;
        months
            .iter()
            .find(|s| {
                s.month.number() == start_month.month && s.month.is_leap() == start_month.leap
            })
            .map(|s| s.ordinal as i64)
            .unwrap_or(1)
    };

    'periods: loop {
        if year > max_year || empty_periods > limits.max_empty_periods.max(1) * 40 {
            break;
        }
        if let Some(count) = count_target {
            if out.len() as u64 >= count {
                break;
            }
        }

        let months = engine.months_of_year(year)?;
        let days_in_year: i64 = months.iter().map(|s| s.days as i64).sum();

        // Candidate ISO ordinals for this period.
        let mut candidates: Vec<i64> = Vec::new();
        match freq {
            Frequency::Yearly => {
                if !recur.by_year_day.is_empty() {
                    for &yd in &recur.by_year_day {
                        candidates.extend(engine.resolve_year_day(&months, days_in_year, yd));
                    }
                } else {
                    for want in &months_wanted {
                        if let Some(slot) = engine.resolve_month(&months, *want) {
                            for &d in &days_wanted {
                                candidates.extend(engine.resolve_day(slot, d));
                            }
                        }
                    }
                }
                year += interval as i32;
            }
            Frequency::Monthly => {
                let slot = months.iter().find(|s| s.ordinal as i64 == month_ordinal);
                if let Some(slot) = slot {
                    let month_matches = recur.by_month.is_empty()
                        || recur.by_month.iter().any(|m| {
                            m.month == slot.month.number() && m.leap == slot.month.is_leap()
                        });
                    if month_matches {
                        for &d in &days_wanted {
                            candidates.extend(engine.resolve_day(*slot, d));
                        }
                    }
                }
                month_ordinal += interval;
                while month_ordinal > months.len() as i64 {
                    month_ordinal -= months.len() as i64;
                    year += 1;
                    // Month counts differ per year; re-check against the
                    // new year on the next loop iteration.
                    if year > max_year {
                        break 'periods;
                    }
                }
            }
            other => {
                return Err(ValueError::new(format!(
                    "RSCALE expansion only supports YEARLY and MONTHLY (got {})",
                    other.as_str()
                )))
            }
        }

        // BYDAY as a plain weekday limit (ordinals are not defined for
        // RSCALE expansion here).
        if !recur.by_day.is_empty() {
            candidates.retain(|&ord| {
                Date::from_ordinal(ord)
                    .map(|d| recur.by_day.iter().any(|w| w.weekday == d.weekday()))
                    .unwrap_or(false)
            });
        }

        candidates.sort_unstable();
        candidates.dedup();

        // BYSETPOS within the period.
        if !recur.by_set_pos.is_empty() {
            let n = candidates.len() as i32;
            let mut selected: Vec<i64> = recur
                .by_set_pos
                .iter()
                .filter_map(|&p| {
                    let i = if p > 0 { p as i32 - 1 } else { n + p as i32 };
                    (0..n).contains(&i).then(|| candidates[i as usize])
                })
                .collect();
            selected.sort_unstable();
            selected.dedup();
            candidates = selected;
        }

        if candidates.is_empty() {
            empty_periods += 1;
            continue;
        }
        empty_periods = 0;

        for ord in candidates {
            if ord < start_ordinal {
                continue;
            }
            if let Some(last) = last_emitted {
                if ord <= last {
                    continue;
                }
            }
            let date = Date::from_ordinal(ord)?;
            if let Some((until_date, until_instant)) = until_ordinal {
                let past = match until_instant {
                    None => ord > until_date,
                    Some(instant) => {
                        DateTime { date, time }.to_epoch_like() > instant
                    }
                };
                if past {
                    break 'periods;
                }
            }
            last_emitted = Some(ord);
            out.push(if date_only {
                DateOrDateTime::Date(date)
            } else {
                DateOrDateTime::DateTime(DateTime { date, time })
            });
            if let Some(count) = count_target {
                if out.len() as u64 >= count {
                    break 'periods;
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rrule::ExpandLimits;
    use crate::value::Recur;

    fn run(rule: &str, dtstart: &str, n: usize) -> Vec<String> {
        let recur = Recur::parse(rule).unwrap();
        let start = DateOrDateTime::parse(dtstart).unwrap();
        expand_rscale(&recur, start, ExpandLimits::default())
            .unwrap()
            .into_iter()
            .take(n)
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn gregorian_skip_backward_monthly() {
        assert_eq!(
            run("RSCALE=GREGORIAN;FREQ=MONTHLY;SKIP=BACKWARD;COUNT=4", "20140131", 10),
            vec!["20140131", "20140228", "20140331", "20140430"]
        );
    }

    #[test]
    fn gregorian_skip_forward_monthly() {
        assert_eq!(
            run("RSCALE=GREGORIAN;FREQ=MONTHLY;SKIP=FORWARD;COUNT=4", "20140131", 10),
            vec!["20140131", "20140301", "20140331", "20140501"]
        );
    }

    #[test]
    fn gregorian_leap_day_omit() {
        assert_eq!(
            run("RSCALE=GREGORIAN;FREQ=YEARLY;COUNT=4", "20120229", 10),
            vec!["20120229", "20160229", "20200229", "20240229"]
        );
    }

    #[test]
    fn chinese_new_year() {
        assert_eq!(
            run("RSCALE=CHINESE;FREQ=YEARLY;UNTIL=20180101", "20130210", 10),
            vec!["20130210", "20140131", "20150219", "20160208", "20170128"]
        );
    }

    #[test]
    fn unsupported_rscale_errors() {
        let recur = Recur::parse("RSCALE=RUSSIAN;FREQ=YEARLY").unwrap();
        let start = DateOrDateTime::parse("20131025").unwrap();
        assert!(expand_rscale(&recur, start, ExpandLimits::default()).is_err());
    }
}
