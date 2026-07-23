//! Validate RRULE expansion against reference-implementation expectations:
//!
//! - libical's icalrecur_test.txt: 146 cases of RRULE + DTSTART +
//!   full expected INSTANCES list.
//! - ical.js's recur iterator tests (extracted to cases.json): 127 cases
//!   including expected expansions, zero-instance rules, and must-fail
//!   parses.
//!
//! RSCALE cases (non-Gregorian calendars, RFC 7529) are out of scope and
//! not loaded.

use std::fs;
use std::path::{Path, PathBuf};

use vobject_core::rrule::{expand, ExpandLimits};
use vobject_core::value::{DateOrDateTime, Recur};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .canonicalize()
        .unwrap()
}

fn expand_strings(
    rule: &str,
    dtstart: DateOrDateTime,
    limit: usize,
) -> Result<Vec<String>, String> {
    let recur = Recur::parse(rule).map_err(|e| e.to_string())?;
    let iter = expand(&recur, dtstart, ExpandLimits::default()).map_err(|e| e.to_string())?;
    Ok(iter.take(limit).map(|d| d.to_string()).collect())
}

#[test]
fn libical_icalrecur_expectations() {
    let text = fs::read_to_string(fixtures().join("libical/recur/icalrecur_test.txt")).unwrap();

    let mut cases: Vec<(String, String, String, Vec<String>)> = Vec::new(); // (comment, rrule, dtstart, instances)
    let mut comment = String::new();
    let mut rrule = String::new();
    let mut dtstart = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(c) = line.strip_prefix('#') {
            comment = c.trim().to_string();
        } else if let Some(r) = line.strip_prefix("RRULE:") {
            rrule = r.to_string();
        } else if let Some(d) = line.strip_prefix("DTSTART:") {
            dtstart = d.to_string();
        } else if let Some(i) = line.strip_prefix("INSTANCES:") {
            let instances: Vec<String> = if i.trim() == "NONE" || i.trim().is_empty() {
                Vec::new()
            } else {
                i.split(',').map(|s| s.trim().to_string()).collect()
            };
            cases.push((comment.clone(), rrule.clone(), dtstart.clone(), instances));
        }
    }
    assert!(
        cases.len() >= 140,
        "only parsed {} libical cases",
        cases.len()
    );

    let mut failures = Vec::new();
    for (comment, rule, start, expected) in &cases {
        // libical marks combinations it does not itself implement with a
        // literal "*** UNIMPLEMENTED" expectation; nothing to compare.
        if expected.iter().any(|e| e.starts_with("***")) {
            continue;
        }
        let dtstart = match DateOrDateTime::parse(start) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{comment}: bad DTSTART {start}: {e}"));
                continue;
            }
        };
        // libical caps its own expansion; expand a little beyond the
        // expected count and compare prefixes (uncapped rules would run
        // forever otherwise). For finite rules require exact equality.
        let rule_is_finite = rule.to_ascii_uppercase().contains("COUNT=")
            || rule.to_ascii_uppercase().contains("UNTIL=");
        let limit = if rule_is_finite {
            expected.len() + 5
        } else {
            expected.len()
        };
        match expand_strings(rule, dtstart, limit.max(1)) {
            Ok(got) => {
                let matches = if rule_is_finite {
                    &got == expected
                } else {
                    got.len() == expected.len() && &got == expected
                };
                if !matches {
                    let diff_at = got
                        .iter()
                        .zip(expected.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(got.len().min(expected.len()));
                    failures.push(format!(
                        "{comment}\n    RRULE:{rule} DTSTART:{start}\n    got {} instances, expected {}; first diff at {diff_at}: got {:?} expected {:?}",
                        got.len(),
                        expected.len(),
                        got.get(diff_at),
                        expected.get(diff_at),
                    ));
                }
            }
            Err(e) => failures.push(format!("{comment}: RRULE:{rule}: {e}")),
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} libical recurrence cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn libical_rscale_expectations() {
    let dir = fixtures().join("libical/recur");
    let mut cases: Vec<(String, String, String, Vec<String>)> = Vec::new();
    for file in [
        "icalrecur_test_rscale.txt",
        "icalrecur_test_rscale_withicu.txt",
        "icalrecur_test_rscale_withicu_dangi.txt",
    ] {
        let text = fs::read_to_string(dir.join(file)).unwrap();
        let mut comment = String::new();
        let mut rrule = String::new();
        let mut dtstart = String::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(c) = line.strip_prefix('#') {
                comment = format!("{file}: {}", c.trim());
            } else if let Some(r) = line.strip_prefix("RRULE:") {
                rrule = r.to_string();
            } else if let Some(d) = line.strip_prefix("DTSTART:") {
                dtstart = d.to_string();
            } else if let Some(i) = line.strip_prefix("INSTANCES:") {
                let instances: Vec<String> = i.split(',').map(|s| s.trim().to_string()).collect();
                cases.push((comment.clone(), rrule.clone(), dtstart.clone(), instances));
            }
        }
    }
    assert!(
        cases.len() >= 30,
        "only parsed {} RSCALE cases",
        cases.len()
    );

    let mut failures = Vec::new();
    let mut checked = 0;
    for (comment, rule, start, expected) in &cases {
        if expected.iter().any(|e| e.starts_with("***")) {
            // libical's own unimplemented markers (e.g. RSCALE=RUSSIAN must
            // error) — verify we error rather than expand.
            let recur = Recur::parse(rule);
            let bad = recur.is_err()
                || expand(
                    &recur.unwrap(),
                    DateOrDateTime::parse(start).unwrap(),
                    ExpandLimits::default(),
                )
                .is_err();
            if !bad {
                failures.push(format!("{comment}: {rule} should be rejected"));
            }
            continue;
        }
        // Known calendar-backend divergence: the placement of the Chinese
        // leap month 9 a century out depends on astronomical predictions
        // of new moons and solar terms; ICU4C (libical's backend) puts the
        // next one in 2109, ICU4X (ours) in 2139. Both agree on 2014. Only
        // the agreed prefix is compared for this rule.
        let backend_divergent =
            rule == "RSCALE=CHINESE;FREQ=YEARLY;BYMONTHDAY=10;BYMONTH=9L;SKIP=OMIT;COUNT=2";

        let dtstart = DateOrDateTime::parse(start).unwrap();
        let rule_is_finite = rule.to_ascii_uppercase().contains("COUNT=")
            || rule.to_ascii_uppercase().contains("UNTIL=");
        let limit = if rule_is_finite {
            expected.len() + 5
        } else {
            expected.len()
        };
        match expand_strings(rule, dtstart, limit.max(1)) {
            Ok(got) => {
                if backend_divergent {
                    if got.first() != expected.first() {
                        failures.push(format!(
                            "{comment}: first instance disagrees: got {:?} expected {:?}",
                            got.first(),
                            expected.first()
                        ));
                    } else {
                        checked += 1;
                    }
                } else if &got != expected {
                    let diff_at = got
                        .iter()
                        .zip(expected.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(got.len().min(expected.len()));
                    failures.push(format!(
                        "{comment}\n    RRULE:{rule} DTSTART:{start}\n    got {} instances, expected {}; first diff at {diff_at}: got {:?} expected {:?}",
                        got.len(),
                        expected.len(),
                        got.get(diff_at),
                        expected.get(diff_at),
                    ));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{comment}: RRULE:{rule}: {e}")),
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} RSCALE cases failed ({checked} ok):\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn icaljs_recur_expectations() {
    let raw = fs::read_to_string(fixtures().join("icaljs/recur/cases.json")).unwrap();
    let data: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let cases = data["cases"].as_array().unwrap();
    assert!(cases.len() >= 120);

    // ical.js writes times as "2015-04-30T08:00:00" / dates as "2016-01-03".
    fn to_wire(s: &str) -> String {
        s.replace(['-', ':'], "")
    }
    fn from_ours(s: &str) -> String {
        // 20150430T080000[Z] -> 2015-04-30T08:00:00[Z]
        if let Some((d, t)) = s.split_once('T') {
            let (t, z) = match t.strip_suffix('Z') {
                Some(t) => (t, "Z"),
                None => (t, ""),
            };
            format!(
                "{}-{}-{}T{}:{}:{}{z}",
                &d[0..4],
                &d[4..6],
                &d[6..8],
                &t[0..2],
                &t[2..4],
                &t[4..6]
            )
        } else {
            format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8])
        }
    }

    // Cases where ical.js's own expectations deviate from RFC 5545 as
    // implemented by libical (which this library follows). Keyed by source
    // line in test/recur_iterator_test.js:
    //   354            — BYWEEKNO with FREQ=WEEKLY (N/A per the RFC table;
    //                    ical.js applies it as a filter).
    //   1003–1059      — ical.js's week-number computation is offset from
    //                    ISO 8601 (e.g. it puts 2015-06-08 in week 23; ISO
    //                    and libical say week 23 is June 1–7).
    //   1455, 1466     — ical.js force-includes DTSTART as an occurrence
    //                    of BYSETPOS rules it does not match; libical and
    //                    dateutil (and this library) only emit matching
    //                    instances.
    let known_deviations: &[i64] = &[354, 1003, 1013, 1022, 1040, 1059, 1455, 1466];

    let mut failures = Vec::new();
    let mut skipped = 0;
    for case in cases {
        let rule = case["rrule"].as_str().unwrap();
        let line = case["source_line"].as_i64().unwrap_or(0);
        if known_deviations.contains(&line) {
            skipped += 1;
            continue;
        }

        if case["invalid"].as_bool() == Some(true) {
            if Recur::parse(rule).is_ok() {
                failures.push(format!("line {line}: {rule} should fail to parse"));
            }
            continue;
        }

        let dtstart_s = to_wire(case["dtstart"].as_str().unwrap());
        let dtstart = match DateOrDateTime::parse(&dtstart_s) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("line {line}: bad dtstart {dtstart_s}: {e}"));
                continue;
            }
        };

        if case["no_instances"].as_bool() == Some(true) {
            match expand_strings(rule, dtstart, 3) {
                Ok(got) if got.is_empty() => {}
                Ok(got) => failures.push(format!(
                    "line {line}: {rule} expected no instances, got {got:?}"
                )),
                Err(e) => failures.push(format!("line {line}: {rule}: {e}")),
            }
            continue;
        }

        let expected: Vec<String> = case["dates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let finite = case["finite"].as_bool() == Some(true);
        let limit = if finite {
            expected.len() + 5
        } else {
            expected.len()
        };
        match expand_strings(rule, dtstart, limit.max(1)) {
            Ok(got) => {
                let got: Vec<String> = got.iter().map(|s| from_ours(s)).collect();
                if got != expected {
                    let diff_at = got
                        .iter()
                        .zip(expected.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(got.len().min(expected.len()));
                    failures.push(format!(
                        "line {line}: {rule} (dtstart {dtstart_s})\n    got {} instances, expected {}; first diff at {diff_at}: got {:?} expected {:?}",
                        got.len(),
                        expected.len(),
                        got.get(diff_at),
                        expected.get(diff_at),
                    ));
                }
            }
            Err(e) => failures.push(format!("line {line}: {rule}: {e}")),
        }
    }

    // The deviation list must not silently grow.
    assert_eq!(skipped, 8, "known-deviation list out of sync");

    if !failures.is_empty() {
        panic!(
            "{}/{} ical.js recurrence cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}
