//! Regression tests for panics, aborts, and silent misbehavior reachable
//! from untrusted input, plus the strict-mode grammar holes they exposed.

use vobject_core::rrule::{expand, ExpandLimits};
use vobject_core::value::{DateOrDateTime, Duration, Recur};
use vobject_core::{parse, ParseOptions, RepairKind};

fn expand_all(rule: &str, start: &str, cap: usize) -> Vec<DateOrDateTime> {
    let recur = Recur::parse(rule).unwrap();
    let dtstart = DateOrDateTime::parse(start).unwrap();
    expand(&recur, dtstart, ExpandLimits::default())
        .unwrap()
        .take(cap)
        .collect()
}

// --- RRULE expansion must never panic or hang on hostile intervals ---

#[test]
fn daily_huge_interval_terminates() {
    let out = expand_all("FREQ=DAILY;INTERVAL=10000000;COUNT=5", "20260101T100000", 10);
    // The first instance is DTSTART itself; the next is ~27000 years out of
    // range, so expansion just ends.
    assert_eq!(out.len(), 1);
}

#[test]
fn weekly_huge_interval_terminates() {
    let out = expand_all("FREQ=WEEKLY;INTERVAL=10000000;COUNT=5", "20260101T100000", 10);
    assert_eq!(out.len(), 1);
}

#[test]
fn monthly_u64_max_interval_terminates() {
    let out = expand_all(
        "FREQ=MONTHLY;INTERVAL=18446744073709551615;COUNT=5",
        "20260101T100000",
        10,
    );
    assert_eq!(out.len(), 1);
}

#[test]
fn yearly_u32_max_interval_terminates() {
    let out = expand_all(
        "FREQ=YEARLY;INTERVAL=4294967295;COUNT=5",
        "20260101T100000",
        10,
    );
    assert_eq!(out.len(), 1);
}

#[test]
fn hourly_huge_interval_terminates() {
    let out = expand_all(
        "FREQ=HOURLY;INTERVAL=18446744073709551615;COUNT=5",
        "20260101T100000",
        10,
    );
    assert_eq!(out.len(), 1);
}

#[test]
fn yearly_near_year_range_end_terminates() {
    // Stepping past year 9999 must end the expansion, not panic.
    let out = expand_all("FREQ=YEARLY;COUNT=5", "99980601T120000", 10);
    assert_eq!(out.len(), 2); // 9998 and 9999 only
}

#[test]
fn monthly_near_year_range_end_terminates() {
    let out = expand_all("FREQ=MONTHLY;COUNT=40", "99981115T120000", 50);
    assert_eq!(out.len(), 14); // Nov 9998 .. Dec 9999
}

#[test]
fn weekly_near_year_range_end_terminates() {
    let out = expand_all("FREQ=WEEKLY;COUNT=200", "99991101T120000", 300);
    // Nov 1 .. Dec 27, 9999: nine Mondays-equivalent weekly steps.
    assert_eq!(out.len(), 9);
}

#[test]
fn daily_near_year_range_end_terminates() {
    let out = expand_all("FREQ=DAILY;COUNT=100", "99991220T120000", 200);
    assert_eq!(out.len(), 12); // Dec 20 .. Dec 31, 9999
}

#[test]
fn secondly_at_year_range_end_terminates() {
    // The sub-daily cursor saturates at 9999-12-31; iteration must stop
    // rather than spin forever on filtered duplicates.
    let out = expand_all("FREQ=SECONDLY;COUNT=100", "99991231T235950", 200);
    assert_eq!(out.len(), 10); // 23:59:50 .. 23:59:59
}

// --- Sub-daily BYHOUR/BYMINUTE gaps must fast-forward, not exhaust the
// empty-period budget ---

#[test]
fn secondly_sparse_byhour_finds_instances() {
    let out = expand_all("FREQ=SECONDLY;BYHOUR=3;COUNT=3", "20260101T010000", 10);
    let strings: Vec<String> = out.iter().map(|d| d.to_string()).collect();
    assert_eq!(
        strings,
        vec!["20260101T030000", "20260101T030001", "20260101T030002"]
    );
}

#[test]
fn minutely_sparse_byhour_crosses_days() {
    // 23:30 on Jan 1, BYHOUR=1: instances resume at 01:00 on Jan 2.
    let out = expand_all("FREQ=MINUTELY;BYHOUR=1;COUNT=2", "20260101T233000", 10);
    let strings: Vec<String> = out.iter().map(|d| d.to_string()).collect();
    assert_eq!(strings, vec!["20260102T010000", "20260102T010100"]);
}

#[test]
fn secondly_interval_alignment_preserved_across_fast_forward() {
    // Interval 7 from 01:00:00; instances must stay congruent to DTSTART
    // modulo 7 seconds even after jumping to hour 3.
    let out = expand_all("FREQ=SECONDLY;INTERVAL=7;BYHOUR=3;COUNT=3", "20260101T010000", 10);
    let start = match DateOrDateTime::parse("20260101T010000").unwrap() {
        DateOrDateTime::DateTime(dt) => dt,
        DateOrDateTime::Date(_) => unreachable!(),
    };
    for d in &out {
        if let DateOrDateTime::DateTime(dt) = d {
            let since_start = dt.to_epoch_like() - start.to_epoch_like();
            assert_eq!(since_start % 7, 0, "misaligned instance {dt}");
        }
    }
    assert_eq!(out.len(), 3);
}

// --- ExpandLimits must mean what it says ---

#[test]
fn max_empty_periods_is_honored_literally() {
    // FREQ=DAILY;BYMONTH=1 from Feb 1: every period is empty until next
    // January (334 empty daily periods). A limit of 10 must stop expansion
    // before then; the default must let it through.
    let recur = Recur::parse("FREQ=DAILY;BYMONTH=1;COUNT=1").unwrap();
    let dtstart = DateOrDateTime::parse("20260201T000000").unwrap();

    let tight = ExpandLimits {
        max_empty_periods: 10,
        ..ExpandLimits::default()
    };
    let out: Vec<_> = expand(&recur, dtstart, tight).unwrap().take(5).collect();
    assert!(out.is_empty(), "10 empty periods must end expansion, got {out:?}");

    let out: Vec<_> = expand(&recur, dtstart, ExpandLimits::default())
        .unwrap()
        .take(5)
        .collect();
    assert_eq!(out.len(), 1, "default limits must reach the next January");
}

// --- DURATION parsing must reject overflow instead of panicking ---

#[test]
fn duration_overflow_rejected() {
    for bad in [
        "PT100000000000000000H",
        "PT18446744073709551615S",
        "PT9999999999999999999M",
        "P18446744073709551615W",
        "-PT100000000000000000H",
    ] {
        assert!(Duration::parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn duration_total_seconds_never_panics() {
    // Direct construction can exceed what parse allows; total_seconds must
    // saturate rather than overflow.
    let d = Duration {
        negative: false,
        weeks: u64::MAX,
        days: u64::MAX,
        hours: u64::MAX,
        minutes: u64::MAX,
        seconds: u64::MAX,
    };
    assert_eq!(d.total_seconds(), i64::MAX);
    let neg = Duration {
        negative: true,
        ..d
    };
    assert_eq!(neg.total_seconds(), i64::MIN);
}

// --- Strict mode must reject nonconformant BEGIN/END lines; lenient mode
// must record repairs for them ---

#[test]
fn strict_rejects_whitespace_in_delimiters() {
    for doc in [
        "BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n",
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR \r\n",
        "BEGIN:VCALENDAR \r\nEND:VCALENDAR\r\n",
        "BEGIN:\tVCALENDAR\r\nEND:VCALENDAR\r\n",
    ] {
        assert!(
            parse(doc, &ParseOptions::strict()).is_err(),
            "strict must reject {doc:?}"
        );
        let parsed = parse(doc, &ParseOptions::lenient()).unwrap();
        assert!(
            !parsed.repairs.is_empty(),
            "lenient must record a repair for {doc:?}"
        );
        assert_eq!(parsed.components.len(), 1);
        assert_eq!(parsed.components[0].name, "VCALENDAR");
    }
}

#[test]
fn strict_rejects_group_on_delimiters() {
    let doc = "home.BEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\n";
    assert!(parse(doc, &ParseOptions::strict()).is_err());
    let parsed = parse(doc, &ParseOptions::lenient()).unwrap();
    assert!(!parsed.repairs.is_empty());
    assert_eq!(parsed.components[0].name, "VCARD");

    let doc = "BEGIN:VCARD\r\nFN:x\r\nhome.END:VCARD\r\n";
    assert!(parse(doc, &ParseOptions::strict()).is_err());
    let parsed = parse(doc, &ParseOptions::lenient()).unwrap();
    assert!(!parsed.repairs.is_empty());
}

#[test]
fn conformant_delimiters_still_clean() {
    let doc = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
    let strict = parse(doc, &ParseOptions::strict()).unwrap();
    assert_eq!(strict.components.len(), 1);
    let lenient = parse(doc, &ParseOptions::lenient()).unwrap();
    assert!(lenient.repairs.is_empty());
}

// --- xCal reader must cap nesting depth instead of aborting ---

#[test]
fn xcal_depth_bomb_is_an_error() {
    // Balanced 100k-deep nesting: deep enough to overflow the stack in both
    // the recursive tree walk and the recursive node Drop unless a depth cap
    // rejects it first.
    let mut doc = String::from("<?xml version=\"1.0\"?><icalendar xmlns=\"urn:ietf:params:xml:ns:icalendar-2.0\">");
    for _ in 0..100_000 {
        doc.push_str("<vcalendar><components>");
    }
    for _ in 0..100_000 {
        doc.push_str("</components></vcalendar>");
    }
    doc.push_str("</icalendar>");
    let result = vobject_core::xcal::from_xml(&doc);
    assert!(result.is_err());
}

#[test]
fn xcal_moderate_nesting_ok() {
    // Sanity check: the cap must not reject reasonable documents.
    let doc = "<?xml version=\"1.0\"?>\
        <icalendar xmlns=\"urn:ietf:params:xml:ns:icalendar-2.0\">\
        <vcalendar><components><vevent><properties>\
        <summary><text>hi</text></summary>\
        </properties></vevent></components></vcalendar></icalendar>";
    let comps = vobject_core::xcal::from_xml(doc).unwrap();
    assert_eq!(comps.len(), 1);
}

// --- RECUR strictness: RFC 5545 says rule parts must not repeat, and
// INTERVAL is a positive integer ---

#[test]
fn recur_rejects_duplicate_parts() {
    assert!(Recur::parse("FREQ=DAILY;COUNT=5;COUNT=99").is_err());
    assert!(Recur::parse("FREQ=DAILY;INTERVAL=2;INTERVAL=3").is_err());
    assert!(Recur::parse("FREQ=DAILY;BYDAY=MO;BYDAY=TU").is_err());
}

#[test]
fn recur_rejects_zero_interval() {
    assert!(Recur::parse("FREQ=DAILY;INTERVAL=0").is_err());
}

// --- Lenient repairs for the new delimiter fixes keep the zero-repairs ⟺
// strict-valid invariant (spot check; the property test covers it broadly) ---

#[test]
fn delimiter_repairs_round_trip_strict() {
    let doc = "BEGIN: VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
    let parsed = parse(doc, &ParseOptions::lenient()).unwrap();
    assert!(parsed
        .repairs
        .iter()
        .any(|r| matches!(r.kind, RepairKind::NormalizedDelimiter(_))));
    // Re-serializing the repaired model must be strictly valid.
    let wire = vobject_core::write_document(&parsed.components, &Default::default());
    assert!(parse(&wire, &ParseOptions::strict()).is_ok());
}

// --- xCal asymmetries ---

#[test]
fn xcal_mixed_component_kinds_rejected() {
    use vobject_core::Component;
    let cal = Component::new("VCALENDAR");
    let card = Component::new("VCARD");
    // One XML document has one root; a mixed stream cannot be represented
    // faithfully and must be an error, not a silently mislabeled document.
    assert!(vobject_core::xcal::to_xml(&[cal, card]).is_err());
}

#[test]
fn xcal_icalendar_group_round_trips() {
    use vobject_core::{parse, ParseOptions};
    // An iCalendar property with a (nonstandard but parseable) group.
    let doc = "BEGIN:VCALENDAR\r\nITEM1.X-EMAIL:a@example.com\r\nEND:VCALENDAR\r\n";
    let parsed = parse(doc, &ParseOptions::lenient()).unwrap();
    let xml = vobject_core::xcal::to_xml(&parsed.components).unwrap();
    let back = vobject_core::xcal::from_xml(&xml).unwrap();
    let prop = back[0].properties().next().unwrap();
    assert!(
        prop.group.as_deref().is_some_and(|g| g.eq_ignore_ascii_case("ITEM1")),
        "group lost: {prop:?}"
    );
    assert!(prop.name.eq_ignore_ascii_case("X-EMAIL"), "{prop:?}");
}

// --- Byte input: UTF-8 is the only strict encoding; the lenient Latin-1
// fallback must be recorded as a repair, not silent ---

#[test]
fn parse_bytes_records_non_utf8_decode() {
    use vobject_core::{parse_bytes, ErrorKind};
    let doc = b"BEGIN:VCARD\r\nFN:Caf\xe9\r\nEND:VCARD\r\n";

    match parse_bytes(doc, &ParseOptions::strict()) {
        Err(e) => {
            assert_eq!(e.kind, ErrorKind::InvalidUtf8);
            assert_eq!(e.location.line, 2);
        }
        Ok(_) => panic!("strict must reject non-UTF-8 bytes"),
    }

    let parsed = parse_bytes(doc, &ParseOptions::lenient()).unwrap();
    assert!(parsed
        .repairs
        .iter()
        .any(|r| matches!(r.kind, RepairKind::DecodedNonUtf8AsLatin1)));
    let fn_prop = parsed.components[0].properties().next().unwrap();
    assert_eq!(fn_prop.value, "Café", "Latin-1 data must be preserved");
}

#[test]
fn parse_bytes_clean_utf8_has_no_repairs() {
    use vobject_core::parse_bytes;
    let doc = "BEGIN:VCARD\r\nFN:Café\r\nEND:VCARD\r\n".as_bytes();
    let parsed = parse_bytes(doc, &ParseOptions::lenient()).unwrap();
    assert!(parsed.repairs.is_empty());
    assert!(parse_bytes(doc, &ParseOptions::strict()).is_ok());
}

#[test]
fn parse_bytes_strips_bom() {
    use vobject_core::parse_bytes;
    let doc = b"\xef\xbb\xbfBEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\n";
    let parsed = parse_bytes(doc, &ParseOptions::lenient()).unwrap();
    assert_eq!(parsed.components.len(), 1);
    assert!(parsed.repairs.is_empty());
}
