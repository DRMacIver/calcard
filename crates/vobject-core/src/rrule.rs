//! RRULE occurrence expansion (RFC 5545 §3.3.10 semantics).
//!
//! The algorithm follows the day-mask design used by python-dateutil and
//! libical: iterate frequency periods from DTSTART, generate the candidate
//! days in each period by intersecting all BY-rule masks (with the RFC's
//! expand/limit semantics arising naturally from DTSTART-derived defaults),
//! cross with the time set, apply BYSETPOS per period, then filter against
//! DTSTART/UNTIL/COUNT globally.
//!
//! Expansion is timezone-naive ("floating"): DST-aware expansion is a
//! higher layer's concern. Instances are strictly increasing; an instance
//! is only produced if it genuinely matches the rule (DTSTART itself is
//! not force-included), matching libical and dateutil.

use crate::value::datetime::{days_in_month, Date, DateTime, Time, Weekday};
use crate::value::recur::{Frequency, Recur, Until};
use crate::value::{DateOrDateTime, ValueError};

/// Hard safety limits for expansion.
#[derive(Debug, Clone, Copy)]
pub struct ExpandLimits {
    /// Stop after this many consecutive periods yield no candidate
    /// (a rule like FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30 never matches).
    pub max_empty_periods: usize,
    /// Absolute cap on years scanned past DTSTART.
    pub max_years: i32,
}

impl Default for ExpandLimits {
    fn default() -> ExpandLimits {
        ExpandLimits {
            max_empty_periods: 200,
            max_years: 2200,
        }
    }
}

#[derive(Debug, Clone)]
struct Config {
    freq: Frequency,
    interval: i64,
    count: Option<u64>,
    until: Option<DateTime>,
    by_month: Vec<u8>,
    by_week_no: Vec<i8>,
    by_year_day: Vec<i16>,
    by_month_day: Vec<i8>,
    /// (ordinal, weekday); ordinal only meaningful for YEARLY/MONTHLY.
    by_day: Vec<(Option<i8>, Weekday)>,
    by_set_pos: Vec<i16>,
    wkst: Weekday,
    times: Vec<Time>,
    by_hour: Vec<u8>,
    by_minute: Vec<u8>,
    by_second: Vec<u8>,
    dtstart: DateTime,
    date_only: bool,
    utc: bool,
}

impl Config {
    fn new(recur: &Recur, start: DateOrDateTime) -> Result<Config, ValueError> {
        let freq = recur
            .freq
            .ok_or_else(|| ValueError::new("RECUR without FREQ"))?;
        let (dtstart, date_only, utc) = match start {
            DateOrDateTime::Date(d) => (
                DateTime {
                    date: d,
                    time: Time {
                        hour: 0,
                        minute: 0,
                        second: 0,
                        utc: false,
                    },
                },
                true,
                false,
            ),
            DateOrDateTime::DateTime(dt) => (dt, false, dt.time.utc),
        };

        let until = recur.until.map(|u| match u {
            Until::Date(d) => DateTime {
                date: d,
                time: Time {
                    // A date-valued UNTIL bounds whole days inclusively.
                    hour: 23,
                    minute: 59,
                    second: 59,
                    utc: false,
                },
            },
            Until::DateTime(dt) => dt,
        });

        // The Gregorian engine has no leap months; RSCALE rules take the
        // rscale expansion path before reaching this Config.
        let mut by_month: Vec<u8> = recur
            .by_month
            .iter()
            .filter(|m| !m.leap && m.month <= 12)
            .map(|m| m.month)
            .collect();
        let mut by_month_day: Vec<i8> = recur.by_month_day.clone();
        let mut by_day: Vec<(Option<i8>, Weekday)> = recur
            .by_day
            .iter()
            .map(|w| (w.ordinal, w.weekday))
            .collect();
        let by_week_no = recur.by_week_no.clone();
        let by_year_day = recur.by_year_day.clone();

        // DTSTART-derived defaults (the RFC's expand/limit table falls out
        // of these).
        match freq {
            Frequency::Yearly => {
                // Both libical and ical.js pin DTSTART's month whenever no
                // month-selecting-or-expanding rule is present, even with an
                // explicit BYMONTHDAY (FREQ=YEARLY;BYMONTHDAY=29 from a
                // Feb 29 start recurs on leap-year Februaries only).
                if by_year_day.is_empty() && by_day.is_empty() && by_week_no.is_empty() {
                    if by_month_day.is_empty() {
                        by_month_day = vec![dtstart.date.day as i8];
                    }
                    if by_month.is_empty() {
                        by_month = vec![dtstart.date.month];
                    }
                }
            }
            Frequency::Monthly => {
                if by_month_day.is_empty() && by_day.is_empty() {
                    by_month_day = vec![dtstart.date.day as i8];
                }
            }
            Frequency::Weekly => {
                if by_day.is_empty() {
                    by_day = vec![(None, dtstart.date.weekday())];
                }
            }
            _ => {}
        }

        // Time-of-day sets, likewise defaulted from DTSTART. For a
        // date-valued DTSTART, time-level BY rules are ignored entirely
        // (libical: "time-related BY* should be ignored if DTSTART is
        // date-only").
        let sub_daily = matches!(
            freq,
            Frequency::Hourly | Frequency::Minutely | Frequency::Secondly
        );
        if date_only {
            let midnight = Time {
                hour: 0,
                minute: 0,
                second: 0,
                utc: false,
            };
            return Ok(Config {
                freq,
                interval: recur.interval() as i64,
                count: recur.count,
                until,
                by_month,
                by_week_no,
                by_year_day,
                by_month_day,
                by_day,
                by_set_pos: recur.by_set_pos.clone(),
                wkst: recur.week_start(),
                times: vec![midnight],
                by_hour: vec![0],
                by_minute: vec![0],
                by_second: vec![0],
                dtstart,
                date_only,
                utc,
            });
        }
        let by_hour = if !recur.by_hour.is_empty() {
            recur.by_hour.clone()
        } else if !sub_daily {
            vec![dtstart.time.hour]
        } else {
            Vec::new()
        };
        let by_minute = if !recur.by_minute.is_empty() {
            recur.by_minute.clone()
        } else if matches!(
            freq,
            Frequency::Yearly
                | Frequency::Monthly
                | Frequency::Weekly
                | Frequency::Daily
                | Frequency::Hourly
        ) {
            vec![dtstart.time.minute]
        } else {
            Vec::new()
        };
        let by_second = if !recur.by_second.is_empty() {
            recur.by_second.clone()
        } else if !matches!(freq, Frequency::Secondly) {
            vec![dtstart.time.second]
        } else {
            Vec::new()
        };

        let mut times = Vec::new();
        if !sub_daily {
            let mut hours = by_hour.clone();
            let mut minutes = by_minute.clone();
            let mut seconds = by_second.clone();
            hours.sort_unstable();
            minutes.sort_unstable();
            seconds.sort_unstable();
            for &h in &hours {
                for &m in &minutes {
                    for &s in &seconds {
                        if let Ok(t) = Time::new(h, m, s.min(60), utc) {
                            times.push(t);
                        }
                    }
                }
            }
        }

        Ok(Config {
            freq,
            interval: recur.interval() as i64,
            count: recur.count,
            until,
            by_month,
            by_week_no,
            by_year_day,
            by_month_day,
            by_day,
            by_set_pos: recur.by_set_pos.clone(),
            wkst: recur.week_start(),
            times,
            by_hour,
            by_minute,
            by_second,
            dtstart,
            date_only,
            utc,
        })
    }

    /// Limit-style day test used by DAILY and sub-daily frequencies, and as
    /// the base mask for WEEKLY.
    fn day_passes_limits(&self, date: Date) -> bool {
        if !self.by_month.is_empty() && !self.by_month.contains(&date.month) {
            return false;
        }
        if !self.by_year_day.is_empty() {
            let doy = date.day_of_year() as i16;
            let n = Date::days_in_year(date.year) as i16;
            if !self
                .by_year_day
                .iter()
                .any(|&yd| yd == doy || (yd < 0 && n + yd + 1 == doy))
            {
                return false;
            }
        }
        if !self.by_month_day.is_empty() {
            let dim = days_in_month(date.year, date.month) as i8;
            let d = date.day as i8;
            if !self
                .by_month_day
                .iter()
                .any(|&md| md == d || (md < 0 && dim + md + 1 == d))
            {
                return false;
            }
        }
        if !self.by_day.is_empty() {
            let wd = date.weekday();
            if !self.by_day.iter().any(|&(_, w)| w == wd) {
                return false;
            }
        }
        true
    }
}

/// Start of "week 1" of `year`: the first WKST-started week with at least
/// four days in the year.
fn week1_start(year: i32, wkst: Weekday) -> Date {
    let jan1 = Date {
        year,
        month: 1,
        day: 1,
    };
    let since_wkst = (jan1.weekday().number0() + 7 - wkst.number0()) % 7;
    let candidate = jan1.add_days(-(since_wkst as i64)).unwrap();
    if 7 - since_wkst >= 4 {
        candidate
    } else {
        candidate.add_days(7).unwrap()
    }
}

fn weeks_in_year(year: i32, wkst: Weekday) -> i64 {
    (week1_start(year + 1, wkst).to_ordinal() - week1_start(year, wkst).to_ordinal()) / 7
}

/// The week-based year a date belongs to (its ISO year, generalized to an
/// arbitrary week start).
fn week_year(date: Date, wkst: Weekday) -> i32 {
    let y = date.year;
    if date.to_ordinal() >= week1_start(y + 1, wkst).to_ordinal() {
        y + 1
    } else if date.to_ordinal() < week1_start(y, wkst).to_ordinal() {
        y - 1
    } else {
        y
    }
}

/// Candidate days within one YEARLY period.
fn yearly_days(cfg: &Config, year: i32) -> Vec<Date> {
    let mut days: Vec<Date> = Vec::new();

    if !cfg.by_week_no.is_empty() {
        // Selected weeks (whole weeks; may spill into adjacent years).
        let nweeks = weeks_in_year(year, cfg.wkst);
        let w1 = week1_start(year, cfg.wkst);
        let mut selected: Vec<i64> = Vec::new();
        for &wn in &cfg.by_week_no {
            let w = if wn > 0 {
                wn as i64
            } else {
                nweeks + wn as i64 + 1
            };
            if (1..=nweeks).contains(&w) {
                selected.push(w);
            }
        }
        selected.sort_unstable();
        selected.dedup();
        for w in selected {
            let start = w1.add_days((w - 1) * 7).unwrap();
            for i in 0..7 {
                let d = start.add_days(i).unwrap();
                // Within selected weeks, BYDAY limits by weekday; other
                // BY-day rules and BYMONTH also limit.
                if !cfg.by_day.is_empty()
                    && !cfg.by_day.iter().any(|&(_, wd)| wd == d.weekday())
                {
                    continue;
                }
                if cfg.by_day.is_empty() && d.weekday() != cfg.dtstart.date.weekday() {
                    // No BYDAY: the RFC pins the day-of-week to DTSTART's.
                    continue;
                }
                if !cfg.by_month.is_empty() && !cfg.by_month.contains(&d.month) {
                    continue;
                }
                days.push(d);
            }
        }
        return days;
    }

    // Ordinal BYDAY entries expand relative to the year, or to each
    // selected month when BYMONTH is present.
    let months: Vec<u8> = if cfg.by_month.is_empty() {
        (1..=12).collect()
    } else {
        cfg.by_month.clone()
    };

    let has_ordinal_byday = cfg.by_day.iter().any(|(o, _)| o.is_some());
    let mut ordinal_days: Vec<Date> = Vec::new();
    if has_ordinal_byday {
        if cfg.by_month.is_empty() {
            for &(ord, wd) in &cfg.by_day {
                match ord {
                    Some(n) => ordinal_days.extend(nth_weekday_of_year(year, wd, n)),
                    None => {}
                }
            }
        } else {
            for &m in &months {
                for &(ord, wd) in &cfg.by_day {
                    if let Some(n) = ord {
                        ordinal_days.extend(nth_weekday_of_month(year, m, wd, n));
                    }
                }
            }
        }
    }

    let jan1 = Date {
        year,
        month: 1,
        day: 1,
    };
    let n_days = Date::days_in_year(year) as i64;
    for i in 0..n_days {
        let d = jan1.add_days(i).unwrap();
        if !cfg.by_month.is_empty() && !cfg.by_month.contains(&d.month) {
            continue;
        }
        if !cfg.by_year_day.is_empty() {
            let doy = (i + 1) as i16;
            let n = n_days as i16;
            if !cfg
                .by_year_day
                .iter()
                .any(|&yd| yd == doy || (yd < 0 && n + yd + 1 == doy))
            {
                continue;
            }
        }
        if !cfg.by_month_day.is_empty() {
            let dim = days_in_month(d.year, d.month) as i8;
            let dd = d.day as i8;
            if !cfg
                .by_month_day
                .iter()
                .any(|&md| md == dd || (md < 0 && dim + md + 1 == dd))
            {
                continue;
            }
        }
        if !cfg.by_day.is_empty() {
            let wd = d.weekday();
            let plain_match = cfg
                .by_day
                .iter()
                .any(|&(ord, w)| ord.is_none() && w == wd);
            let ordinal_match = has_ordinal_byday && ordinal_days.contains(&d);
            if !plain_match && !ordinal_match {
                continue;
            }
        }
        days.push(d);
    }
    days
}

/// Dates that are the nth weekday of a year (n may be negative).
fn nth_weekday_of_year(year: i32, wd: Weekday, n: i8) -> Option<Date> {
    let jan1 = Date {
        year,
        month: 1,
        day: 1,
    };
    let n_days = Date::days_in_year(year) as i64;
    if n > 0 {
        let first = jan1.add_days(jan1.weekday().days_until(wd) as i64).unwrap();
        let d = first.add_days((n as i64 - 1) * 7).unwrap();
        (d.year == year).then_some(d)
    } else {
        let dec31 = Date {
            year,
            month: 12,
            day: 31,
        };
        let back = (dec31.weekday().number0() + 7 - wd.number0()) % 7;
        let last = dec31.add_days(-(back as i64)).unwrap();
        let d = last.add_days((n as i64 + 1) * 7);
        match d {
            Ok(d) if d.year == year && (jan1.to_ordinal()..jan1.to_ordinal() + n_days).contains(&d.to_ordinal()) => Some(d),
            _ => None,
        }
    }
}

/// Dates that are the nth weekday of a month (n may be negative).
fn nth_weekday_of_month(year: i32, month: u8, wd: Weekday, n: i8) -> Option<Date> {
    let dim = days_in_month(year, month);
    if n > 0 {
        let first = Date {
            year,
            month,
            day: 1,
        };
        let day = 1 + first.weekday().days_until(wd) as i64 + (n as i64 - 1) * 7;
        (day <= dim as i64).then(|| Date {
            year,
            month,
            day: day as u8,
        })
    } else {
        let last = Date {
            year,
            month,
            day: dim,
        };
        let back = (last.weekday().number0() + 7 - wd.number0()) % 7;
        let day = dim as i64 - back as i64 + (n as i64 + 1) * 7;
        (day >= 1).then(|| Date {
            year,
            month,
            day: day as u8,
        })
    }
}

/// Candidate days within one MONTHLY period.
fn monthly_days(cfg: &Config, year: i32, month: u8) -> Vec<Date> {
    if !cfg.by_month.is_empty() && !cfg.by_month.contains(&month) {
        return Vec::new();
    }
    let mut days = Vec::new();
    let dim = days_in_month(year, month);

    let has_ordinal = cfg.by_day.iter().any(|(o, _)| o.is_some());
    let mut ordinal_days: Vec<Date> = Vec::new();
    if has_ordinal {
        for &(ord, wd) in &cfg.by_day {
            if let Some(n) = ord {
                ordinal_days.extend(nth_weekday_of_month(year, month, wd, n));
            }
        }
    }

    for day in 1..=dim {
        let d = Date { year, month, day };
        if !cfg.by_month_day.is_empty() {
            let dd = day as i8;
            if !cfg
                .by_month_day
                .iter()
                .any(|&md| md == dd || (md < 0 && dim as i8 + md + 1 == dd))
            {
                continue;
            }
        }
        if !cfg.by_day.is_empty() {
            let wd = d.weekday();
            let plain = cfg.by_day.iter().any(|&(o, w)| o.is_none() && w == wd);
            let ordinal = has_ordinal && ordinal_days.contains(&d);
            if !plain && !ordinal {
                continue;
            }
        }
        days.push(d);
    }
    days
}

/// The expansion iterator.
pub struct RRuleIter {
    cfg: Config,
    limits: ExpandLimits,
    /// Buffered candidates for the current period (already BYSETPOS'd),
    /// in reverse order for cheap pop.
    buffer: Vec<DateTime>,
    /// Period cursor.
    cursor: PeriodCursor,
    emitted: u64,
    last: Option<DateTime>,
    empty_periods: usize,
    done: bool,
}

enum PeriodCursor {
    Yearly { year: i32 },
    Monthly { year: i32, month0: i64 },
    Weekly { week_start: Date },
    Daily { day: Date },
    SubDaily { current: DateTime },
}

impl RRuleIter {
    fn fill_from_days(&mut self, days: Vec<Date>) {
        let mut candidates: Vec<DateTime> = Vec::with_capacity(days.len() * self.cfg.times.len());
        for d in days {
            for t in &self.cfg.times {
                candidates.push(DateTime { date: d, time: *t });
            }
        }
        candidates.sort_by_key(|dt| dt.to_epoch_like());
        candidates.dedup();
        self.apply_setpos_and_buffer(candidates);
    }

    fn apply_setpos_and_buffer(&mut self, candidates: Vec<DateTime>) {
        let selected: Vec<DateTime> = if self.cfg.by_set_pos.is_empty() {
            candidates
        } else {
            let n = candidates.len() as i32;
            let mut idx: Vec<i32> = self
                .cfg
                .by_set_pos
                .iter()
                .filter_map(|&p| {
                    let i = if p > 0 { p as i32 - 1 } else { n + p as i32 };
                    (0..n).contains(&i).then_some(i)
                })
                .collect();
            idx.sort_unstable();
            idx.dedup();
            idx.into_iter()
                .map(|i| candidates[i as usize])
                .collect()
        };
        self.buffer = selected;
        self.buffer.reverse();
    }

    /// Generate the next period's candidates into the buffer. Returns false
    /// when iteration must stop.
    fn advance_period(&mut self) -> bool {
        let max_year = self.cfg.dtstart.date.year + self.limits.max_years;
        match &mut self.cursor {
            PeriodCursor::Yearly { year } => {
                if *year > max_year {
                    return false;
                }
                let days = yearly_days(&self.cfg, *year);
                *year += self.cfg.interval as i32;
                self.fill_from_days(days);
            }
            PeriodCursor::Monthly { year, month0 } => {
                let y = *year + (*month0 / 12) as i32;
                if y > max_year {
                    return false;
                }
                let m = (*month0 % 12) as u8 + 1;
                let days = monthly_days(&self.cfg, y, m);
                *month0 += self.cfg.interval;
                self.fill_from_days(days);
            }
            PeriodCursor::Weekly { week_start } => {
                if week_start.year > max_year {
                    return false;
                }
                let mut days = Vec::new();
                for i in 0..7 {
                    let d = week_start.add_days(i).unwrap();
                    if !self.cfg.by_day.iter().any(|&(_, w)| w == d.weekday()) {
                        continue;
                    }
                    if !self.cfg.by_month.is_empty() && !self.cfg.by_month.contains(&d.month) {
                        continue;
                    }
                    days.push(d);
                }
                *week_start = week_start.add_days(7 * self.cfg.interval).unwrap();
                self.fill_from_days(days);
            }
            PeriodCursor::Daily { day } => {
                if day.year > max_year {
                    return false;
                }
                let days = if self.cfg.day_passes_limits(*day) {
                    vec![*day]
                } else {
                    Vec::new()
                };
                *day = day.add_days(self.cfg.interval).unwrap();
                self.fill_from_days(days);
            }
            PeriodCursor::SubDaily { current } => {
                if current.date.year > max_year {
                    return false;
                }
                let cfg = &self.cfg;
                let cur = *current;
                let step_seconds = match cfg.freq {
                    Frequency::Hourly => 3600,
                    Frequency::Minutely => 60,
                    _ => 1,
                } * cfg.interval;

                let mut out: Vec<DateTime> = Vec::new();
                if cfg.day_passes_limits(cur.date) {
                    let h = cur.time.hour;
                    let m = cur.time.minute;
                    let s = cur.time.second;
                    let hour_ok = cfg.by_hour.is_empty() || cfg.by_hour.contains(&h);
                    match cfg.freq {
                        Frequency::Hourly => {
                            if hour_ok {
                                let mut minutes = cfg.by_minute.clone();
                                let mut seconds = cfg.by_second.clone();
                                minutes.sort_unstable();
                                seconds.sort_unstable();
                                for &mm in &minutes {
                                    for &ss in &seconds {
                                        if let Ok(t) = Time::new(h, mm, ss.min(60), cfg.utc) {
                                            out.push(DateTime {
                                                date: cur.date,
                                                time: t,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        Frequency::Minutely => {
                            let minute_ok =
                                cfg.by_minute.is_empty() || cfg.by_minute.contains(&m);
                            if hour_ok && minute_ok {
                                let mut seconds = cfg.by_second.clone();
                                seconds.sort_unstable();
                                for &ss in &seconds {
                                    if let Ok(t) = Time::new(h, m, ss.min(60), cfg.utc) {
                                        out.push(DateTime {
                                            date: cur.date,
                                            time: t,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {
                            let minute_ok =
                                cfg.by_minute.is_empty() || cfg.by_minute.contains(&m);
                            let second_ok =
                                cfg.by_second.is_empty() || cfg.by_second.contains(&s);
                            if hour_ok && minute_ok && second_ok {
                                out.push(cur);
                            }
                        }
                    }
                } else {
                    // Fast-forward to the first aligned step on a later day.
                    let day_end = DateTime {
                        date: cur.date,
                        time: Time {
                            hour: 23,
                            minute: 59,
                            second: 59,
                            utc: cfg.utc,
                        },
                    };
                    let gap = day_end.to_epoch_like() - cur.to_epoch_like() + 1;
                    let steps = (gap + step_seconds - 1) / step_seconds;
                    let target = cur.to_epoch_like() + (steps - 1) * step_seconds;
                    *current = epoch_like_to_datetime(target, cfg.utc);
                }

                let next = current.to_epoch_like() + step_seconds;
                *current = epoch_like_to_datetime(next, cfg.utc);
                self.apply_setpos_and_buffer(out);
            }
        }
        true
    }
}

fn epoch_like_to_datetime(v: i64, utc: bool) -> DateTime {
    let days = v.div_euclid(86400);
    let secs = v.rem_euclid(86400);
    DateTime {
        date: Date::from_ordinal(days).unwrap_or(Date {
            year: 9999,
            month: 12,
            day: 31,
        }),
        time: Time {
            hour: (secs / 3600) as u8,
            minute: ((secs / 60) % 60) as u8,
            second: (secs % 60) as u8,
            utc,
        },
    }
}

impl Iterator for RRuleIter {
    type Item = DateOrDateTime;

    fn next(&mut self) -> Option<DateOrDateTime> {
        if self.done {
            return None;
        }
        if let Some(count) = self.cfg.count {
            if self.emitted >= count {
                self.done = true;
                return None;
            }
        }
        loop {
            match self.buffer.pop() {
                Some(dt) => {
                    self.empty_periods = 0;
                    // Global filters: >= DTSTART, strictly increasing,
                    // <= UNTIL.
                    if dt.to_epoch_like() < self.cfg.dtstart.to_epoch_like() {
                        continue;
                    }
                    if let Some(last) = self.last {
                        if dt.to_epoch_like() <= last.to_epoch_like() {
                            continue;
                        }
                    }
                    if let Some(until) = self.cfg.until {
                        if dt.to_epoch_like() > until.to_epoch_like() {
                            self.done = true;
                            return None;
                        }
                    }
                    self.last = Some(dt);
                    self.emitted += 1;
                    return Some(if self.cfg.date_only {
                        DateOrDateTime::Date(dt.date)
                    } else {
                        DateOrDateTime::DateTime(dt)
                    });
                }
                None => {
                    self.empty_periods += 1;
                    if self.empty_periods > self.limits.max_empty_periods.max(1) * 400 {
                        self.done = true;
                        return None;
                    }
                    if !self.advance_period() {
                        self.done = true;
                        return None;
                    }
                    if !self.buffer.is_empty() {
                        self.empty_periods = 0;
                    }
                }
            }
        }
    }
}

/// The result of [`expand`]: either the streaming Gregorian iterator or a
/// materialized RSCALE expansion.
pub enum Expansion {
    Gregorian(Box<RRuleIter>),
    Rscale(std::vec::IntoIter<DateOrDateTime>),
}

impl Iterator for Expansion {
    type Item = DateOrDateTime;

    fn next(&mut self) -> Option<DateOrDateTime> {
        match self {
            Expansion::Gregorian(iter) => iter.next(),
            Expansion::Rscale(iter) => iter.next(),
        }
    }
}

/// Expand a recurrence rule from a start instant.
///
/// RSCALE rules (RFC 7529) with YEARLY or MONTHLY frequency are evaluated
/// in the recurrence calendar; other frequencies use the Gregorian engine
/// directly, since day- and week-based arithmetic is calendar-independent
/// (BYMONTH on such rules is then interpreted in the Gregorian calendar —
/// a documented approximation).
pub fn expand(
    recur: &Recur,
    dtstart: DateOrDateTime,
    limits: ExpandLimits,
) -> Result<Expansion, ValueError> {
    if let Some(rscale) = &recur.rscale {
        let gregorian = matches!(
            rscale.to_ascii_uppercase().as_str(),
            "GREGORIAN" | "GREGORY"
        );
        let skip_active = !matches!(
            recur.skip.unwrap_or_default(),
            crate::value::recur::Skip::Omit
        );
        let calendar_dependent = matches!(
            recur.freq,
            Some(Frequency::Yearly) | Some(Frequency::Monthly)
        );
        if calendar_dependent && (!gregorian || skip_active) {
            let instances = crate::rscale::expand_rscale(recur, dtstart, limits)?;
            return Ok(Expansion::Rscale(instances.into_iter()));
        }
        if !gregorian && crate::rscale::calendar_for(rscale).is_none() {
            return Err(ValueError::new(format!("unsupported RSCALE {rscale:?}")));
        }
    }
    Ok(Expansion::Gregorian(Box::new(expand_gregorian(
        recur, dtstart, limits,
    )?)))
}

/// The Gregorian expansion engine (no RSCALE handling).
fn expand_gregorian(
    recur: &Recur,
    dtstart: DateOrDateTime,
    limits: ExpandLimits,
) -> Result<RRuleIter, ValueError> {
    let cfg = Config::new(recur, dtstart)?;
    let cursor = match cfg.freq {
        // With BYWEEKNO the yearly period sequence is anchored on the
        // week-based year of DTSTART when DTSTART itself satisfies the
        // week-number and weekday predicate (its week is part of the
        // recurrence), and on DTSTART's calendar year otherwise. This
        // matches libical's behavior across the github issue1223/1230
        // cases in its conformance data.
        Frequency::Yearly => PeriodCursor::Yearly {
            year: if cfg.by_week_no.is_empty() {
                cfg.dtstart.date.year
            } else {
                let wy = week_year(cfg.dtstart.date, cfg.wkst);
                let week = (cfg.dtstart.date.to_ordinal()
                    - week1_start(wy, cfg.wkst).to_ordinal())
                    / 7
                    + 1;
                let nweeks = weeks_in_year(wy, cfg.wkst);
                let week_match = cfg.by_week_no.iter().any(|&wn| {
                    let resolved = if wn > 0 {
                        wn as i64
                    } else {
                        nweeks + wn as i64 + 1
                    };
                    resolved == week
                });
                let day_match = cfg.by_day.is_empty()
                    || cfg
                        .by_day
                        .iter()
                        .any(|&(_, d)| d == cfg.dtstart.date.weekday());
                if week_match && day_match {
                    wy
                } else {
                    cfg.dtstart.date.year
                }
            },
        },
        Frequency::Monthly => PeriodCursor::Monthly {
            year: cfg.dtstart.date.year,
            month0: cfg.dtstart.date.month as i64 - 1,
        },
        Frequency::Weekly => {
            let since_wkst =
                (cfg.dtstart.date.weekday().number0() + 7 - cfg.wkst.number0()) % 7;
            PeriodCursor::Weekly {
                week_start: cfg.dtstart.date.add_days(-(since_wkst as i64)).unwrap(),
            }
        }
        Frequency::Daily => PeriodCursor::Daily {
            day: cfg.dtstart.date,
        },
        Frequency::Hourly | Frequency::Minutely | Frequency::Secondly => {
            PeriodCursor::SubDaily {
                current: cfg.dtstart,
            }
        }
    };
    Ok(RRuleIter {
        cfg,
        limits,
        buffer: Vec::new(),
        cursor,
        emitted: 0,
        last: None,
        empty_periods: 0,
        done: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rule: &str, dtstart: &str, n: usize) -> Vec<String> {
        let recur = Recur::parse(rule).unwrap();
        let start = DateOrDateTime::parse(dtstart).unwrap();
        expand(&recur, start, ExpandLimits::default())
            .unwrap()
            .take(n)
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn daily_count() {
        assert_eq!(
            run("FREQ=DAILY;COUNT=3", "19970902T090000", 10),
            vec!["19970902T090000", "19970903T090000", "19970904T090000"]
        );
    }

    #[test]
    fn weekly_interval_byday() {
        assert_eq!(
            run("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH;COUNT=4", "19970902T090000", 10),
            vec![
                "19970902T090000",
                "19970904T090000",
                "19970916T090000",
                "19970918T090000"
            ]
        );
    }

    #[test]
    fn monthly_first_friday() {
        assert_eq!(
            run("FREQ=MONTHLY;COUNT=3;BYDAY=1FR", "19970905T090000", 10),
            vec!["19970905T090000", "19971003T090000", "19971107T090000"]
        );
    }

    #[test]
    fn monthly_last_day() {
        assert_eq!(
            run("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3", "19970930T090000", 10),
            vec!["19970930T090000", "19971031T090000", "19971130T090000"]
        );
    }

    #[test]
    fn yearly_default_from_dtstart() {
        assert_eq!(
            run("FREQ=YEARLY;COUNT=3", "19970610T090000", 10),
            vec!["19970610T090000", "19980610T090000", "19990610T090000"]
        );
    }

    #[test]
    fn bysetpos_last_weekday() {
        // Last weekday of the month — the RFC 5545 example: from a DTSTART
        // of Mon 1997-09-29 the first instance is Tue 1997-09-30.
        assert_eq!(
            run(
                "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=3",
                "19970929T090000",
                10
            ),
            vec!["19970930T090000", "19971031T090000", "19971128T090000"]
        );
    }

    #[test]
    fn until_is_inclusive() {
        assert_eq!(
            run(
                "FREQ=DAILY;UNTIL=19970904T090000Z",
                "19970902T090000",
                10
            ),
            vec!["19970902T090000", "19970903T090000", "19970904T090000"]
        );
    }

    #[test]
    fn dtstart_not_matching_is_skipped() {
        // DTSTART is a Monday; the rule selects Tuesdays only.
        assert_eq!(
            run("FREQ=WEEKLY;BYDAY=TU;COUNT=2", "19970901T090000", 10),
            vec!["19970902T090000", "19970909T090000"]
        );
    }

    #[test]
    fn feb29_yearly() {
        assert_eq!(
            run("FREQ=YEARLY;COUNT=3", "19960229T090000", 10),
            vec!["19960229T090000", "20000229T090000", "20040229T090000"]
        );
    }

    #[test]
    fn never_matching_rule_terminates() {
        let out = run("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30", "20200101T000000", 10);
        assert!(out.is_empty());
    }

    #[test]
    fn date_only_expansion() {
        assert_eq!(
            run("FREQ=WEEKLY;COUNT=3", "20260722", 10),
            vec!["20260722", "20260729", "20260805"]
        );
    }

    #[test]
    fn hourly_with_interval() {
        assert_eq!(
            run("FREQ=HOURLY;INTERVAL=3;COUNT=3", "19970902T090000", 10),
            vec!["19970902T090000", "19970902T120000", "19970902T150000"]
        );
    }

    #[test]
    fn secondly_interval() {
        assert_eq!(
            run("FREQ=SECONDLY;INTERVAL=3;COUNT=3", "20150430T080000", 10),
            vec!["20150430T080000", "20150430T080003", "20150430T080006"]
        );
    }
}
