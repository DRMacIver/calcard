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

use crate::value::datetime::{Date, DateTime, Time, Weekday};
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
        d.year().extended_year(),
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
                    if date.year().extended_year() != ext_year {
                        continue;
                    }
                    slots.push(MonthSlot {
                        month,
                        ordinal: date.month().ordinal,
                        days: date.days_in_month(),
                        first_iso: from_icu_iso(date.to_calendar(Iso))?.to_ordinal(),
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
    fn resolve_month(&self, months: &[MonthSlot], want: RecurMonth) -> Option<MonthSlot> {
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
    fn resolve_year_day(&self, months: &[MonthSlot], days_in_year: i64, day: i16) -> Option<i64> {
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

/// ISO ordinal of the nth (positive from the start, negative from the end)
/// weekday within a contiguous day range, or None if it falls outside.
fn nth_weekday_in_range(first: i64, len: i64, wd: Weekday, n: i8) -> Option<i64> {
    if n > 0 {
        let first_date = Date::from_ordinal(first).ok()?;
        let offset = first_date.weekday().days_until(wd) as i64;
        let d = first + offset + (n as i64 - 1) * 7;
        (d < first + len).then_some(d)
    } else {
        let last = first + len - 1;
        let last_date = Date::from_ordinal(last).ok()?;
        let back = (last_date.weekday().number0() + 7 - wd.number0()) % 7;
        let d = last - back as i64 + (n as i64 + 1) * 7;
        (d >= first).then_some(d)
    }
}

/// Days of one month slot matching BYMONTHDAY (when present) and BYDAY:
/// plain entries match by weekday, ordinal entries via the pre-resolved
/// `ordinal_days` set (month- or year-relative per the caller). Mirrors the
/// Gregorian engine's day masks over recurrence-calendar month geometry.
fn slot_days_filtered(slot: &MonthSlot, recur: &Recur, ordinal_days: &[i64]) -> Vec<i64> {
    let has_ordinal = recur.by_day.iter().any(|w| w.ordinal.is_some());
    let mut out = Vec::new();
    for day in 1..=slot.days {
        let ord = slot.first_iso + day as i64 - 1;
        if !recur.by_month_day.is_empty() {
            let dd = day as i8;
            let dim = slot.days as i8;
            if !recur
                .by_month_day
                .iter()
                .any(|&md| md == dd || (md < 0 && dim + md + 1 == dd))
            {
                continue;
            }
        }
        let Ok(d) = Date::from_ordinal(ord) else {
            continue;
        };
        let wd = d.weekday();
        let plain = recur
            .by_day
            .iter()
            .any(|w| w.ordinal.is_none() && w.weekday == wd);
        let ordinal = has_ordinal && ordinal_days.contains(&ord);
        if plain || ordinal {
            out.push(ord);
        }
    }
    out
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
    let start_epoch = DateTime {
        date: start_date,
        time,
    }
    .to_epoch_like();
    let start_r = to_icu_iso(start_date)?.to_calendar(Ref(&cal));
    let start_year = start_r.year().extended_year();
    let start_month = RecurMonth {
        month: start_r.month().number(),
        leap: start_r.month().to_input().is_leap(),
    };
    let start_day = start_r.day_of_month().0;

    if !recur.by_week_no.is_empty() {
        // Week numbering in an arbitrary recurrence calendar has no
        // agreed-upon semantics (and no ICU4X support); refusing loudly
        // beats silently ignoring the rule part.
        return Err(ValueError::new("BYWEEKNO is not supported with RSCALE"));
    }

    let has_by_day = !recur.by_day.is_empty();

    // Mirror the Gregorian engine's DTSTART-derived defaults: the start
    // month/day pin in only when no day-selecting rule part is present.
    // An empty months_wanted means "all months of the year".
    let months_wanted: Vec<RecurMonth> = if !recur.by_month.is_empty() {
        recur.by_month.clone()
    } else if has_by_day || !recur.by_year_day.is_empty() {
        Vec::new()
    } else {
        vec![start_month]
    };
    let days_wanted: Vec<i8> = if !recur.by_month_day.is_empty() {
        recur.by_month_day.clone()
    } else {
        vec![start_day as i8]
    };

    // Time-of-day sets, defaulted from DTSTART exactly like the Gregorian
    // engine (ignored entirely for a date-valued DTSTART).
    let times: Vec<Time> = if date_only {
        vec![time]
    } else {
        let mut hours = if recur.by_hour.is_empty() {
            vec![time.hour]
        } else {
            recur.by_hour.clone()
        };
        let mut minutes = if recur.by_minute.is_empty() {
            vec![time.minute]
        } else {
            recur.by_minute.clone()
        };
        let mut seconds = if recur.by_second.is_empty() {
            vec![time.second]
        } else {
            recur.by_second.clone()
        };
        // Sorted and deduplicated: the cross product below must not scale
        // with duplicated entries in hostile input.
        hours.sort_unstable();
        hours.dedup();
        minutes.sort_unstable();
        minutes.dedup();
        seconds.sort_unstable();
        seconds.dedup();
        let mut ts = Vec::new();
        for &h in &hours {
            for &m in &minutes {
                for &s in &seconds {
                    if let Ok(t) = Time::new(h, m, s.min(60), time.utc) {
                        ts.push(t);
                    }
                }
            }
        }
        ts
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
    // Mirror the Gregorian engine's clamp at year 9999: stepping past the
    // last recurrence-calendar year that reaches into the representable ISO
    // range must end the expansion, not error away the valid instances.
    let last_representable_year = to_icu_iso(Date {
        year: 9999,
        month: 12,
        day: 31,
    })?
    .to_calendar(Ref(&cal))
    .year()
    .extended_year();
    let max_year = start_year
        .saturating_add(limits.max_years)
        .min(last_representable_year);

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
        if year > max_year || empty_periods > limits.max_empty_periods.max(1) {
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
                // Month- or year-relative BYDAY ordinal targets, mirroring
                // the Gregorian engine: relative to each selected month
                // when BYMONTH is present, to the whole year otherwise.
                let ordinal_days: Vec<i64> = if has_by_day {
                    let mut out_days = Vec::new();
                    if months_wanted.is_empty() {
                        for w in &recur.by_day {
                            if let Some(n) = w.ordinal {
                                out_days.extend(nth_weekday_in_range(
                                    months[0].first_iso,
                                    days_in_year,
                                    w.weekday,
                                    n,
                                ));
                            }
                        }
                    } else {
                        for want in &months_wanted {
                            if let Some(slot) = engine.resolve_month(&months, *want) {
                                for w in &recur.by_day {
                                    if let Some(n) = w.ordinal {
                                        out_days.extend(nth_weekday_in_range(
                                            slot.first_iso,
                                            slot.days as i64,
                                            w.weekday,
                                            n,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    out_days
                } else {
                    Vec::new()
                };

                if !recur.by_year_day.is_empty() {
                    for &yd in &recur.by_year_day {
                        candidates.extend(engine.resolve_year_day(&months, days_in_year, yd));
                    }
                    // BYMONTH and BYDAY limit BYYEARDAY-generated days.
                    if !months_wanted.is_empty() {
                        candidates.retain(|&ord| {
                            months_wanted.iter().any(|want| {
                                engine.resolve_month(&months, *want).is_some_and(|s| {
                                    (s.first_iso..s.first_iso + s.days as i64).contains(&ord)
                                })
                            })
                        });
                    }
                    if has_by_day {
                        candidates.retain(|&ord| {
                            let Ok(d) = Date::from_ordinal(ord) else {
                                return false;
                            };
                            let wd = d.weekday();
                            recur
                                .by_day
                                .iter()
                                .any(|w| w.ordinal.is_none() && w.weekday == wd)
                                || ordinal_days.contains(&ord)
                        });
                    }
                } else if has_by_day {
                    // BYDAY expands within the selected months (or year).
                    let slots: Vec<MonthSlot> = if months_wanted.is_empty() {
                        months.clone()
                    } else {
                        months_wanted
                            .iter()
                            .filter_map(|want| engine.resolve_month(&months, *want))
                            .collect()
                    };
                    for slot in &slots {
                        candidates.extend(slot_days_filtered(slot, recur, &ordinal_days));
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
                        if has_by_day {
                            let mut ordinal_days = Vec::new();
                            for w in &recur.by_day {
                                if let Some(n) = w.ordinal {
                                    ordinal_days.extend(nth_weekday_in_range(
                                        slot.first_iso,
                                        slot.days as i64,
                                        w.weekday,
                                        n,
                                    ));
                                }
                            }
                            candidates.extend(slot_days_filtered(slot, recur, &ordinal_days));
                        } else {
                            for &d in &days_wanted {
                                candidates.extend(engine.resolve_day(*slot, d));
                            }
                        }
                    }
                }
                month_ordinal += interval;
                // Month counts differ per year (12 vs 13 months in
                // lunisolar calendars), so each peeled year must use its
                // own count, not the loop-entry year's.
                let mut months_in_year = months.len() as i64;
                while month_ordinal > months_in_year {
                    month_ordinal -= months_in_year;
                    year += 1;
                    if year > max_year {
                        break 'periods;
                    }
                    months_in_year = engine.months_of_year(year)?.len() as i64;
                }
            }
            other => {
                return Err(ValueError::new(format!(
                    "RSCALE expansion only supports YEARLY and MONTHLY (got {})",
                    other.as_str()
                )))
            }
        }

        candidates.sort_unstable();
        candidates.dedup();

        // Cross candidate days with the time set (both sorted, so the
        // instants come out in instant order), then BYSETPOS over the
        // period's instants — matching the Gregorian engine.
        let mut instants: Vec<(i64, Time)> = Vec::with_capacity(candidates.len() * times.len());
        for &ord in &candidates {
            for &t in &times {
                instants.push((ord, t));
            }
        }
        if !recur.by_set_pos.is_empty() {
            let n = instants.len() as i32;
            let mut idx: Vec<i32> = recur
                .by_set_pos
                .iter()
                .filter_map(|&p| {
                    let i = if p > 0 { p as i32 - 1 } else { n + p as i32 };
                    (0..n).contains(&i).then_some(i)
                })
                .collect();
            idx.sort_unstable();
            idx.dedup();
            instants = idx.into_iter().map(|i| instants[i as usize]).collect();
        }

        if instants.is_empty() {
            empty_periods += 1;
            continue;
        }
        empty_periods = 0;

        for (ord, t) in instants {
            let date = Date::from_ordinal(ord)?;
            let instant = DateTime { date, time: t };
            let epoch = instant.to_epoch_like();
            if epoch < start_epoch {
                continue;
            }
            if let Some(last) = last_emitted {
                if epoch <= last {
                    continue;
                }
            }
            if let Some((until_date, until_instant)) = until_ordinal {
                let past = match until_instant {
                    None => ord > until_date,
                    Some(limit) => epoch > limit,
                };
                if past {
                    break 'periods;
                }
            }
            last_emitted = Some(epoch);
            out.push(if date_only {
                DateOrDateTime::Date(date)
            } else {
                DateOrDateTime::DateTime(instant)
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
            run(
                "RSCALE=GREGORIAN;FREQ=MONTHLY;SKIP=BACKWARD;COUNT=4",
                "20140131",
                10
            ),
            vec!["20140131", "20140228", "20140331", "20140430"]
        );
    }

    #[test]
    fn gregorian_skip_forward_monthly() {
        assert_eq!(
            run(
                "RSCALE=GREGORIAN;FREQ=MONTHLY;SKIP=FORWARD;COUNT=4",
                "20140131",
                10
            ),
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
    fn gregorian_rscale_matches_control_at_year_range_end() {
        // Stepping to year 10000 must end the expansion (like the Gregorian
        // engine's clamp at 9999), not error away the valid instances.
        let start = DateOrDateTime::parse("99980101").unwrap();
        let recur = Recur::parse("RSCALE=GREGORIAN;FREQ=YEARLY;SKIP=FORWARD;COUNT=5").unwrap();
        let got: Vec<String> = expand_rscale(&recur, start, ExpandLimits::default())
            .unwrap()
            .into_iter()
            .map(|d| d.to_string())
            .collect();
        let control_rule = Recur::parse("FREQ=YEARLY;COUNT=5").unwrap();
        let control: Vec<String> =
            crate::rrule::expand(&control_rule, start, ExpandLimits::default())
                .unwrap()
                .map(|d| d.to_string())
                .collect();
        assert_eq!(got, control);
        assert_eq!(got, vec!["99980101", "99990101"]);
    }

    #[test]
    fn chinese_monthly_interval_spanning_leap_years() {
        // INTERVAL month-slot arithmetic across year boundaries must account
        // for each peeled year's own month count (12 vs 13 in lunisolar
        // years): jumping 30 slots at once must land on the same month as
        // counting 30 slots one at a time.
        let by_thirty = run(
            "RSCALE=CHINESE;FREQ=MONTHLY;INTERVAL=30;COUNT=2",
            "20230122",
            10,
        );
        let one_by_one = run("RSCALE=CHINESE;FREQ=MONTHLY;COUNT=31", "20230122", 40);
        assert_eq!(by_thirty[1], one_by_one[30]);
        // 2023 has 13 months (leap 2) and 2024 has 12, so slot 30 is the
        // sixth month of Chinese 2025 (the buggy jump gave 20250527).
        assert_eq!(by_thirty, vec!["20230122", "20250625"]);
    }

    #[test]
    fn unsupported_rscale_errors() {
        let recur = Recur::parse("RSCALE=RUSSIAN;FREQ=YEARLY").unwrap();
        let start = DateOrDateTime::parse("20131025").unwrap();
        assert!(expand_rscale(&recur, start, ExpandLimits::default()).is_err());
    }
}
