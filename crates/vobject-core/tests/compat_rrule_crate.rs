//! Cross-implementation recurrence oracle: vobject-core's RRULE expansion
//! compared against the `rrule` crate (a dateutil/rrule.js-lineage
//! implementation) on generated rules.
//!
//! Method: generate an RFC 5545-valid RRULE plus a UTC DTSTART, expand with
//! both engines, and require the instant sequences to be identical. The
//! `rrule` crate validates far more strictly than our parser (which is
//! deliberately lenient), so a parse/validation rejection on their side is
//! not a failure by itself — it is counted, and the cumulative rejection
//! rate is asserted to stay bounded so the comparison keeps real coverage.
//!
//! Known deviations (generator constraints, each justified):
//!
//! 1. FREQ=YEARLY with BYMONTHDAY but no BYMONTH: vobject-core pins
//!    DTSTART's month (libical and ical.js behaviour, exercised by the
//!    conformance suites); the rrule crate follows python-dateutil and
//!    expands over all twelve months. Both readings of RFC 5545 §3.3.10
//!    exist in the wild; ours is the libical one by design. The generator
//!    therefore always adds BYMONTH to such rules.
//! 2. FREQ=WEEKLY with BYMONTHDAY is forbidden by RFC 5545 ("MUST NOT be
//!    specified when the FREQ rule part is set to WEEKLY") and the rrule
//!    crate rejects it, so the generator never produces it.
//! 3. BYDAY ordinals (2SU, -1FR) are only generated for MONTHLY/YEARLY:
//!    RFC 5545 only gives them meaning there ("MUST NOT be specified with
//!    a numeric value when the FREQ rule part is not set to MONTHLY or
//!    YEARLY").
//! 4. BYSETPOS is only generated alongside another BY-part, per RFC 5545
//!    ("MUST only be used in conjunction with another BYxxx rule part") —
//!    the rrule crate enforces this.
//! 5. DTSTART/UNTIL are always UTC (`Z`): the rrule crate resolves a
//!    floating DTSTART in the machine's local timezone, which would make
//!    the comparison nondeterministic.
//! 6. FREQ=YEARLY with BYWEEKNO/BYYEARDAY is out of scope here (neither is
//!    in this suite's generation mandate); those parts are covered against
//!    libical/ical.js expectations in rrule_conformance.rs.
//! 7. FREQ=SECONDLY/MINUTELY combined with BYHOUR *and* a day-level part
//!    (BYDAY, BYMONTH or BYMONTHDAY) is excluded: the rrule crate silently
//!    returns no instances for such rules (e.g.
//!    FREQ=SECONDLY;BYMONTHDAY=5;BYHOUR=1 from 19700101T000000Z yields
//!    nothing, with `limited: false`, where 19700105T010000Z is correct —
//!    each combination works in isolation). Its per-RRULE iterator gives up
//!    at MAX_ITER_LOOP but `RRuleSetIter::generate` drops the inner
//!    iterator's `was_limited` flag, so the truncation is unreportable.
//!    Pinned by `subdaily_byhour_with_day_filter_is_a_known_deviation`.
//! 8. FREQ=WEEKLY with BYSETPOS: when DTSTART is not on WKST, vobject-core
//!    (like libical) takes the BYSETPOS interval to be the full WKST-aligned
//!    week containing each instance, while the rrule crate (like dateutil)
//!    truncates the first interval to start at DTSTART, which changes which
//!    candidate is "position 1" in that week and shifts every COUNT-bounded
//!    instance after it. RFC 5545 defines the weekly interval via WKST, so
//!    we keep the libical reading. Pinned by
//!    `weekly_bysetpos_first_partial_week_is_a_known_deviation`.
//! 9. BYDAY lists mixing plain and ordinal entries (e.g. BYDAY=MO,1MO) are
//!    excluded: RFC 5545's BYDAY list denotes the union of its entries
//!    (BYDAY=MO,1MO is every Monday; dateutil and libical agree), but the
//!    rrule crate implements plain entries and ordinal entries as two
//!    independent AND-ed day filters (`iter/filters.rs`), producing their
//!    intersection (first Monday only). The generator emits all-plain or
//!    all-ordinal lists. Pinned by
//!    `mixed_plain_and_ordinal_byday_is_a_known_deviation`.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use hegel::generators;
use vobject_core::rrule::{expand, ExpandLimits};
use vobject_core::value::datetime::days_in_month;
use vobject_core::value::{DateOrDateTime, Recur};

static ACCEPTED: AtomicU64 = AtomicU64::new(0);
static REJECTED: AtomicU64 = AtomicU64::new(0);

/// Expand with vobject-core, formatting instants as `YYYYMMDDTHHMMSSZ`.
fn expand_ours(rule: &str, dtstart: &str, limit: usize) -> Vec<String> {
    let recur = Recur::parse(rule).expect("generated rule must parse");
    let start = DateOrDateTime::parse(dtstart).unwrap();
    expand(&recur, start, ExpandLimits::default())
        .expect("generated rule must expand")
        .take(limit)
        .map(|d| d.to_string())
        .collect()
}

/// Expand with the rrule crate. `Err` means their validator rejected a rule
/// our parser accepts; `Ok((instances, limited))` mirrors `RRuleSet::all`.
fn expand_theirs(rule: &str, dtstart: &str, limit: u16) -> Result<(Vec<String>, bool), String> {
    let input = format!("DTSTART:{dtstart}\nRRULE:{rule}");
    let set = rrule::RRuleSet::from_str(&input).map_err(|e| e.to_string())?;
    let result = set.all(limit);
    let dates = result
        .dates
        .iter()
        .map(|d| d.format("%Y%m%dT%H%M%SZ").to_string())
        .collect();
    Ok((dates, result.limited))
}

struct GeneratedRule {
    rule: String,
    /// COUNT if the rule is COUNT-bounded, None for UNTIL-bounded.
    count: Option<u64>,
}

fn draw_rule(tc: &hegel::TestCase, dtstart: &str) -> GeneratedRule {
    let freq = tc.draw(generators::sampled_from(vec![
        "SECONDLY", "MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "YEARLY",
    ]));
    let mut rule = format!("FREQ={freq}");

    if tc.draw(generators::booleans()) {
        let interval = tc.draw(generators::integers::<u8>().min_value(1).max_value(4));
        rule.push_str(&format!(";INTERVAL={interval}"));
    }

    let mut has_by_part = false;
    let mut has_day_level_part = false;

    // BYDAY, with ordinals only where RFC 5545 allows them (deviation 3)
    // and never mixing plain and ordinal entries (deviation 9).
    if tc.draw(generators::booleans()) {
        let ordinals_ok = matches!(freq, "MONTHLY" | "YEARLY");
        let use_ordinals = ordinals_ok && tc.draw(generators::booleans());
        let n = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
        let mut days = Vec::new();
        for _ in 0..n {
            let day = tc.draw(generators::sampled_from(vec![
                "MO", "TU", "WE", "TH", "FR", "SA", "SU",
            ]));
            if use_ordinals {
                let ord = tc.draw(generators::integers::<i8>().min_value(-5).max_value(5));
                if ord != 0 {
                    days.push(format!("{ord}{day}"));
                    continue;
                }
            }
            days.push(day.to_string());
        }
        // `use_ordinals` can still leave a plain entry behind when the drawn
        // ordinal is 0; keep the list homogeneous (deviation 9).
        let any_ordinal = days
            .iter()
            .any(|d| !d.chars().next().unwrap().is_ascii_alphabetic());
        if any_ordinal {
            days.retain(|d| !d.chars().next().unwrap().is_ascii_alphabetic());
        }
        rule.push_str(&format!(";BYDAY={}", days.join(",")));
        has_by_part = true;
        has_day_level_part = true;
    }

    let mut has_by_month = false;
    if tc.draw(generators::booleans()) {
        let n = tc.draw(generators::integers::<usize>().min_value(1).max_value(2));
        let months: Vec<String> = (0..n)
            .map(|_| {
                tc.draw(generators::integers::<u8>().min_value(1).max_value(12))
                    .to_string()
            })
            .collect();
        rule.push_str(&format!(";BYMONTH={}", months.join(",")));
        has_by_month = true;
        has_by_part = true;
        has_day_level_part = true;
    }

    // BYMONTHDAY: never for WEEKLY (deviation 2); for YEARLY only together
    // with BYMONTH (deviation 1).
    if freq != "WEEKLY" && tc.draw(generators::booleans()) {
        let md = tc.draw(generators::integers::<i8>().min_value(-31).max_value(31));
        if md != 0 {
            if freq == "YEARLY" && !has_by_month {
                let m = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
                rule.push_str(&format!(";BYMONTH={m}"));
            }
            rule.push_str(&format!(";BYMONTHDAY={md}"));
            has_by_part = true;
            has_day_level_part = true;
        }
    }

    // BYHOUR, except on sub-daily rules that also carry a day-level part
    // (deviation 7: the rrule crate silently drops such rules' instances).
    let subdaily = matches!(freq, "SECONDLY" | "MINUTELY");
    if !(subdaily && has_day_level_part) && tc.draw(generators::booleans()) {
        let h = tc.draw(generators::integers::<u8>().max_value(23));
        rule.push_str(&format!(";BYHOUR={h}"));
        has_by_part = true;
    }

    // BYSETPOS only alongside another BY-part (deviation 4), and never for
    // WEEKLY (deviation 8).
    if has_by_part && freq != "WEEKLY" && tc.draw(generators::booleans()) {
        let sp = tc.draw(generators::integers::<i8>().min_value(-6).max_value(6));
        if sp != 0 {
            rule.push_str(&format!(";BYSETPOS={sp}"));
        }
    }

    // Bound the rule: usually COUNT, sometimes UNTIL (never both, per RFC).
    if tc.draw(generators::integers::<u8>().max_value(3)) == 0 {
        // UNTIL strictly after DTSTART's date so the rrule crate's
        // "UNTIL >= DTSTART" validation can never reject on time-of-day.
        let start = DateOrDateTime::parse(dtstart).unwrap();
        let days = tc.draw(generators::integers::<i64>().min_value(1).max_value(1200));
        let until_date = start.date().add_days(days).unwrap();
        let h = tc.draw(generators::integers::<u8>().max_value(23));
        let mi = tc.draw(generators::integers::<u8>().max_value(59));
        let s = tc.draw(generators::integers::<u8>().max_value(59));
        rule.push_str(&format!(";UNTIL={until_date}T{h:02}{mi:02}{s:02}Z"));
        GeneratedRule { rule, count: None }
    } else {
        let count = tc.draw(generators::integers::<u64>().min_value(1).max_value(12));
        rule.push_str(&format!(";COUNT={count}"));
        GeneratedRule {
            rule,
            count: Some(count),
        }
    }
}

fn draw_dtstart(tc: &hegel::TestCase) -> String {
    let year = tc.draw(
        generators::integers::<i32>()
            .min_value(1970)
            .max_value(2060),
    );
    let month = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
    let day = tc.draw(
        generators::integers::<u8>()
            .min_value(1)
            .max_value(days_in_month(year, month)),
    );
    let hour = tc.draw(generators::integers::<u8>().max_value(23));
    let minute = tc.draw(generators::integers::<u8>().max_value(59));
    let second = tc.draw(generators::integers::<u8>().max_value(59));
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

#[hegel::test(test_cases = 400)]
fn expansion_matches_rrule_crate(tc: hegel::TestCase) {
    let dtstart = draw_dtstart(&tc);
    let generated = draw_rule(&tc, &dtstart);
    let rule = &generated.rule;
    tc.note(&format!("RRULE:{rule} DTSTART:{dtstart}"));

    let theirs = match expand_theirs(rule, &dtstart, 60) {
        Ok(t) => t,
        Err(e) => {
            // The rrule crate rejected a rule we accept. Not a failure by
            // itself, but the cumulative rate must stay bounded or the
            // comparison is testing nothing.
            let rejected = REJECTED.fetch_add(1, Ordering::Relaxed) + 1;
            let accepted = ACCEPTED.load(Ordering::Relaxed);
            let total = rejected + accepted;
            assert!(
                total < 100 || rejected * 2 < total,
                "rrule crate rejection rate too high: {rejected}/{total} \
                 (last: RRULE:{rule} DTSTART:{dtstart}: {e})"
            );
            return;
        }
    };
    ACCEPTED.fetch_add(1, Ordering::Relaxed);

    let accepted = ACCEPTED.load(Ordering::Relaxed);
    let rejected = REJECTED.load(Ordering::Relaxed);
    if (accepted + rejected).is_multiple_of(100) {
        // Silent under normal `cargo test`; visible with `-- --nocapture`.
        eprintln!("rrule-crate comparison: {accepted} accepted, {rejected} rejected");
    }

    match generated.count {
        Some(count) => {
            let (their_dates, limited) = theirs;
            assert!(
                !limited,
                "COUNT={count} rule hit the rrule crate's limit: RRULE:{rule}"
            );
            let ours = expand_ours(rule, &dtstart, count as usize + 5);
            assert_eq!(
                ours, their_dates,
                "COUNT rule diverged: RRULE:{rule} DTSTART:{dtstart}"
            );
        }
        None => {
            let (their_dates, limited) = theirs;
            if limited {
                // Their expansion was truncated at 60: compare prefixes.
                let ours = expand_ours(rule, &dtstart, their_dates.len());
                assert_eq!(
                    ours, their_dates,
                    "UNTIL rule prefix diverged: RRULE:{rule} DTSTART:{dtstart}"
                );
            } else {
                // Their expansion is complete: ours must be identical, with
                // nothing extra.
                let ours = expand_ours(rule, &dtstart, their_dates.len() + 3);
                assert_eq!(
                    ours, their_dates,
                    "UNTIL rule diverged: RRULE:{rule} DTSTART:{dtstart}"
                );
            }
        }
    }
}

/// Directed regression: the documented YEARLY/BYMONTHDAY deviation really is
/// a deviation (this pins the behaviour of both sides so a change in either
/// is noticed, and documents why the generator excludes the combination).
#[test]
fn yearly_bymonthday_without_bymonth_is_a_known_deviation() {
    let rule = "FREQ=YEARLY;COUNT=3;BYMONTHDAY=15";
    let dtstart = "20240315T090000Z";

    // vobject-core (libical/ical.js semantics): March is pinned.
    let ours = expand_ours(rule, dtstart, 5);
    assert_eq!(
        ours,
        vec!["20240315T090000Z", "20250315T090000Z", "20260315T090000Z"]
    );

    // rrule crate (dateutil semantics): every month matches, so COUNT=3 is
    // exhausted within 2024.
    let (theirs, _) = expand_theirs(rule, dtstart, 5).unwrap();
    assert_eq!(
        theirs,
        vec!["20240315T090000Z", "20240415T090000Z", "20240515T090000Z"]
    );
}

/// Directed pin of deviation 7: a sub-daily rule combining BYHOUR with a
/// day-level part is correct in vobject-core but silently empty in the
/// rrule crate. If this test ever fails on the `theirs` half, the rrule
/// crate has been fixed and the generator constraint can be dropped.
#[test]
fn subdaily_byhour_with_day_filter_is_a_known_deviation() {
    let rule = "FREQ=SECONDLY;BYMONTHDAY=5;BYHOUR=1;COUNT=1";
    let dtstart = "19700101T000000Z";

    // RFC 5545: BYMONTHDAY and BYHOUR both limit a SECONDLY rule; the first
    // matching second is 01:00:00 on January 5.
    let ours = expand_ours(rule, dtstart, 2);
    assert_eq!(ours, vec!["19700105T010000Z"]);

    let (theirs, limited) = expand_theirs(rule, dtstart, 2).unwrap();
    assert_eq!(theirs, Vec::<String>::new());
    assert!(!limited, "truncation is not even reported");
}

/// Directed pin of deviation 8: WEEKLY + BYSETPOS with DTSTART mid-week.
/// 1970-01-01 is a Thursday; with WKST=MO the week's candidate set is
/// {Mon Dec 29, Thu Jan 1}, whose position 1 (Mon) precedes DTSTART, so the
/// libical reading starts on Mon Jan 5. The rrule crate truncates the first
/// week at DTSTART, making Thu Jan 1 its position 1.
#[test]
fn weekly_bysetpos_first_partial_week_is_a_known_deviation() {
    let rule = "FREQ=WEEKLY;BYDAY=MO,TH;BYSETPOS=1;COUNT=3";
    let dtstart = "19700101T000000Z";

    let ours = expand_ours(rule, dtstart, 5);
    assert_eq!(
        ours,
        vec!["19700105T000000Z", "19700112T000000Z", "19700119T000000Z"]
    );

    let (theirs, _) = expand_theirs(rule, dtstart, 5).unwrap();
    assert_eq!(
        theirs,
        vec!["19700101T000000Z", "19700105T000000Z", "19700112T000000Z"]
    );
}

/// Directed pin of deviation 9: BYDAY=MO,1MO means every Monday (the list
/// is a union per RFC 5545 §3.3.10; dateutil and libical both agree), but
/// the rrule crate intersects the plain and ordinal entries, keeping only
/// the first Monday of each month.
#[test]
fn mixed_plain_and_ordinal_byday_is_a_known_deviation() {
    let rule = "FREQ=MONTHLY;BYDAY=MO,1MO;COUNT=3";
    let dtstart = "19700101T000000Z";

    let ours = expand_ours(rule, dtstart, 5);
    assert_eq!(
        ours,
        vec!["19700105T000000Z", "19700112T000000Z", "19700119T000000Z"]
    );

    let (theirs, _) = expand_theirs(rule, dtstart, 5).unwrap();
    assert_eq!(
        theirs,
        vec!["19700105T000000Z", "19700202T000000Z", "19700302T000000Z"]
    );
}
