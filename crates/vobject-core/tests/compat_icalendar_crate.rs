//! Cross-implementation parser/writer check: calendars built and serialized
//! with the `icalendar` crate must parse with vobject-core in lenient mode
//! with ZERO repairs (their output is RFC 5545-conformant), the semantic
//! values must survive (checked via `Property::typed_value` and raw
//! values), and our re-serialization of the parsed model must round-trip.
//!
//! Known deviations (generator constraints, each justified):
//!
//! 1. Property values never contain CR or control characters other than
//!    '\n': the icalendar crate only escapes `\` `,` `;` and LF, so any
//!    other control character would be written raw onto the content line,
//!    which is invalid RFC 5545 (and correctly triggers a vobject-core
//!    repair). '\n' is generated freely for TEXT-typed properties
//!    (SUMMARY/DESCRIPTION/LOCATION), where their writer escapes it.
//! 2. X- properties are written TEXT-escaped by the icalendar crate (its
//!    `ValueType::by_name` maps every `X-` name to TEXT). vobject-core's
//!    registry treats unknown/X- properties as raw (RFC 5545 gives X-
//!    properties no default value type), so the X-COMPAT comparison is
//!    done by TEXT-unescaping our raw value — not a generator constraint,
//!    just the comparison level.
//! 3. Parameter values never contain ',' or '"' or newlines: the icalendar
//!    crate quotes parameter values containing ':' or ';' but not ','
//!    (RFC 5545 requires quoting for all three), does not support RFC 6868
//!    caret encoding, and has no way to carry a DQUOTE or newline in a
//!    parameter at all.
//! 4. Parameter values never contain '^': the icalendar crate writes '^'
//!    raw (no RFC 6868 support); a raw "^n"/"^^"/"^'" sequence would be
//!    decoded by vobject-core's RFC 6868-aware parameter parser and the
//!    values would legitimately differ.
//! 5. Parameter values do not simultaneously start and end with '"':
//!    their `quote_if_contains_colon` passes such values through
//!    unquoted-as-written, assuming they are pre-quoted.

use chrono::{NaiveDate, TimeZone, Utc};
use hegel::generators;
use icalendar::{Calendar, Component as _, Event, EventLike as _};
use vobject_core::model::Component;
use vobject_core::value::{DateOrDateTime, Dialect, Value};
use vobject_core::{parse, write_document, ParseOptions, WriteOptions};

// ---------------------------------------------------------------------------
// Generators

/// Text for a TEXT-typed property (SUMMARY/DESCRIPTION/LOCATION):
/// no control characters except '\n' (deviation 1), full Unicode otherwise.
fn draw_text(tc: &hegel::TestCase) -> String {
    tc.draw(generators::text().max_size(120))
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}

/// A value for an arbitrary (X-) property: their writer TEXT-escapes these
/// (deviation 2 note), so the same domain as TEXT applies.
fn draw_raw_value(tc: &hegel::TestCase) -> String {
    draw_text(tc)
}

/// A parameter value the icalendar crate can faithfully carry
/// (deviations 3-5).
fn draw_param_value(tc: &hegel::TestCase) -> String {
    let s: String = tc
        .draw(generators::text().max_size(30))
        .chars()
        .filter(|c| !c.is_control() && *c != ',' && *c != '"' && *c != '^')
        .collect();
    s
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum When {
    AllDay(i32, u8, u8),
    Floating(i32, u8, u8, u8, u8, u8),
    Utc(i32, u8, u8, u8, u8, u8),
}

fn draw_when(tc: &hegel::TestCase) -> When {
    let year = tc.draw(generators::integers::<i32>().min_value(1).max_value(9999));
    let month = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
    let day = tc.draw(
        generators::integers::<u8>()
            .min_value(1)
            .max_value(vobject_core::value::datetime::days_in_month(year, month)),
    );
    match tc.draw(generators::integers::<u8>().max_value(2)) {
        0 => When::AllDay(year, month, day),
        n => {
            let h = tc.draw(generators::integers::<u8>().max_value(23));
            let mi = tc.draw(generators::integers::<u8>().max_value(59));
            let s = tc.draw(generators::integers::<u8>().max_value(59));
            if n == 1 {
                When::Floating(year, month, day, h, mi, s)
            } else {
                When::Utc(year, month, day, h, mi, s)
            }
        }
    }
}

fn apply_when(event: &mut Event, when: When, end: bool) {
    let set = |event: &mut Event, dt: icalendar::DatePerhapsTime| {
        if end {
            event.ends(dt);
        } else {
            event.starts(dt);
        }
    };
    match when {
        When::AllDay(y, m, d) => {
            let date = NaiveDate::from_ymd_opt(y, m.into(), d.into()).unwrap();
            set(event, date.into());
        }
        When::Floating(y, m, d, h, mi, s) => {
            let dt = NaiveDate::from_ymd_opt(y, m.into(), d.into())
                .unwrap()
                .and_hms_opt(h.into(), mi.into(), s.into())
                .unwrap();
            set(event, dt.into());
        }
        When::Utc(y, m, d, h, mi, s) => {
            let dt = NaiveDate::from_ymd_opt(y, m.into(), d.into())
                .unwrap()
                .and_hms_opt(h.into(), mi.into(), s.into())
                .unwrap()
                .and_utc();
            set(event, dt.into());
        }
    }
}

/// Assert that the parsed property matches the `When` that was put in.
fn check_when(prop: &vobject_core::model::Property, when: When, ctx: &str) {
    let value = prop
        .typed_value(Dialect::ICalendar)
        .unwrap_or_else(|e| panic!("{ctx}: typed_value failed: {e} on {:?}", prop.value));
    match when {
        When::AllDay(y, m, d) => {
            assert_eq!(prop.param_value("VALUE"), Some("DATE"), "{ctx}: VALUE=DATE");
            match value {
                Value::Date(dates) => {
                    assert_eq!(dates.len(), 1, "{ctx}");
                    assert_eq!(
                        (dates[0].year, dates[0].month, dates[0].day),
                        (y, m, d),
                        "{ctx}"
                    );
                }
                other => panic!("{ctx}: expected Date, got {other:?}"),
            }
        }
        When::Floating(y, m, d, h, mi, s) | When::Utc(y, m, d, h, mi, s) => {
            let expect_utc = matches!(when, When::Utc(..));
            match value {
                Value::DateTime(dts) => {
                    assert_eq!(dts.len(), 1, "{ctx}");
                    match dts[0] {
                        DateOrDateTime::DateTime(dt) => {
                            assert_eq!(
                                (
                                    dt.date.year,
                                    dt.date.month,
                                    dt.date.day,
                                    dt.time.hour,
                                    dt.time.minute,
                                    dt.time.second,
                                    dt.time.utc,
                                ),
                                (y, m, d, h, mi, s, expect_utc),
                                "{ctx}"
                            );
                        }
                        other => panic!("{ctx}: expected DateTime, got {other:?}"),
                    }
                }
                other => panic!("{ctx}: expected DateTime, got {other:?}"),
            }
        }
    }
}

/// One generated event and everything we expect to read back out.
#[derive(Debug)]
struct EventSpec {
    uid: String,
    stamp: (i32, u8, u8, u8, u8, u8),
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    starts: Option<When>,
    ends: Option<When>,
    priority: Option<u32>,
    x_value: Option<String>,
    x_param: Option<String>,
}

fn draw_event_spec(tc: &hegel::TestCase) -> EventSpec {
    let stamp_when = match draw_when(tc) {
        When::AllDay(y, m, d) => (y, m, d, 0, 0, 0),
        When::Floating(y, m, d, h, mi, s) | When::Utc(y, m, d, h, mi, s) => (y, m, d, h, mi, s),
    };
    EventSpec {
        uid: tc.draw(generators::from_regex(r"[a-z0-9-]{1,16}").fullmatch(true)),
        stamp: stamp_when,
        summary: tc.draw(generators::booleans()).then(|| draw_text(tc)),
        description: tc.draw(generators::booleans()).then(|| draw_text(tc)),
        location: tc.draw(generators::booleans()).then(|| draw_text(tc)),
        starts: tc.draw(generators::booleans()).then(|| draw_when(tc)),
        ends: tc.draw(generators::booleans()).then(|| draw_when(tc)),
        priority: tc
            .draw(generators::booleans())
            .then(|| tc.draw(generators::integers::<u32>().max_value(9))),
        x_value: tc.draw(generators::booleans()).then(|| draw_raw_value(tc)),
        x_param: tc
            .draw(generators::booleans())
            .then(|| draw_param_value(tc)),
    }
}

fn build_event(spec: &EventSpec) -> Event {
    let mut event = Event::new();
    event.uid(&spec.uid);
    let (y, m, d, h, mi, s) = spec.stamp;
    event.timestamp(
        Utc.with_ymd_and_hms(y, m.into(), d.into(), h.into(), mi.into(), s.into())
            .unwrap(),
    );
    if let Some(summary) = &spec.summary {
        event.summary(summary);
    }
    if let Some(description) = &spec.description {
        event.description(description);
    }
    if let Some(location) = &spec.location {
        event.location(location);
    }
    if let Some(starts) = spec.starts {
        apply_when(&mut event, starts, false);
    }
    if let Some(ends) = spec.ends {
        apply_when(&mut event, ends, true);
    }
    if let Some(priority) = spec.priority {
        event.priority(priority);
    }
    if let Some(x_value) = &spec.x_value {
        let mut prop = icalendar::Property::new("X-COMPAT", x_value);
        if let Some(x_param) = &spec.x_param {
            prop.add_parameter("X-PARAM", x_param);
        }
        event.append_property(prop.done());
    }
    event.done()
}

/// A single-valued TEXT property must decode back to exactly the input.
fn check_text(comp: &Component, name: &str, expected: &str) {
    let prop = comp
        .prop(name)
        .unwrap_or_else(|| panic!("{name} missing from parsed event"));
    match prop.typed_value(Dialect::ICalendar) {
        Ok(Value::Text(items)) => {
            assert_eq!(items.len(), 1, "{name} multiplicity");
            assert_eq!(items[0], expected, "{name} text round-trip");
        }
        other => panic!("{name}: expected Text, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The property

#[hegel::test(test_cases = 200)]
fn our_parser_reads_icalendar_crate_output(tc: hegel::TestCase) {
    let n_events = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
    let specs: Vec<EventSpec> = (0..n_events).map(|_| draw_event_spec(&tc)).collect();

    let mut calendar = Calendar::new();
    for spec in &specs {
        calendar.push(build_event(spec));
    }
    let wire = calendar.to_string();

    // Lenient parse of RFC-conformant output must record zero repairs.
    let parsed = parse(&wire, &ParseOptions::lenient()).unwrap();
    assert_eq!(
        parsed.repairs,
        vec![],
        "icalendar-crate output needed repairs\nwire:\n{wire}"
    );
    assert_eq!(parsed.components.len(), 1);
    let cal = &parsed.components[0];
    assert!(cal.is("VCALENDAR"));

    // Calendar::new()'s default properties survive.
    assert_eq!(cal.prop("VERSION").map(|p| p.value.as_str()), Some("2.0"));
    assert!(cal.prop("PRODID").is_some());
    assert_eq!(
        cal.prop("CALSCALE").map(|p| p.value.as_str()),
        Some("GREGORIAN")
    );

    let events: Vec<&Component> = cal.comps("VEVENT").collect();
    assert_eq!(events.len(), specs.len(), "event count\nwire:\n{wire}");

    for (spec, event) in specs.iter().zip(events) {
        let ctx = format!("event {}", spec.uid);

        assert_eq!(
            event.prop("UID").map(|p| p.value.as_str()),
            Some(spec.uid.as_str()),
            "{ctx}: UID"
        );

        // DTSTAMP is a UTC timestamp.
        let (y, m, d, h, mi, s) = spec.stamp;
        match event
            .prop("DTSTAMP")
            .expect("DTSTAMP missing")
            .typed_value(Dialect::ICalendar)
        {
            Ok(Value::DateTime(dts)) => match dts[..] {
                [DateOrDateTime::DateTime(dt)] => {
                    assert_eq!(
                        (
                            dt.date.year,
                            dt.date.month,
                            dt.date.day,
                            dt.time.hour,
                            dt.time.minute,
                            dt.time.second,
                            dt.time.utc,
                        ),
                        (y, m, d, h, mi, s, true),
                        "{ctx}: DTSTAMP"
                    );
                }
                ref other => panic!("{ctx}: DTSTAMP parsed as {other:?}"),
            },
            other => panic!("{ctx}: DTSTAMP parsed as {other:?}"),
        }

        for (name, expected) in [
            ("SUMMARY", &spec.summary),
            ("DESCRIPTION", &spec.description),
            ("LOCATION", &spec.location),
        ] {
            match expected {
                Some(text) => check_text(event, name, text),
                None => assert!(event.prop(name).is_none(), "{ctx}: unexpected {name}"),
            }
        }

        match spec.starts {
            Some(when) => check_when(
                event.prop("DTSTART").expect("DTSTART missing"),
                when,
                &format!("{ctx}: DTSTART"),
            ),
            None => assert!(event.prop("DTSTART").is_none(), "{ctx}: unexpected DTSTART"),
        }
        match spec.ends {
            Some(when) => check_when(
                event.prop("DTEND").expect("DTEND missing"),
                when,
                &format!("{ctx}: DTEND"),
            ),
            None => assert!(event.prop("DTEND").is_none(), "{ctx}: unexpected DTEND"),
        }

        match spec.priority {
            Some(p) => match event
                .prop("PRIORITY")
                .expect("PRIORITY missing")
                .typed_value(Dialect::ICalendar)
            {
                Ok(Value::Integer(items)) => {
                    assert_eq!(items, vec![i64::from(p)], "{ctx}: PRIORITY")
                }
                other => panic!("{ctx}: PRIORITY parsed as {other:?}"),
            },
            None => assert!(
                event.prop("PRIORITY").is_none(),
                "{ctx}: unexpected PRIORITY"
            ),
        }

        // The icalendar crate TEXT-escapes X- properties; our registry
        // leaves them raw, so unescape before comparing (deviation 2 note).
        match &spec.x_value {
            Some(x_value) => {
                let prop = event.prop("X-COMPAT").expect("X-COMPAT missing");
                let decoded = vobject_core::escape::unescape_text(&prop.value, None, 0)
                    .expect("lenient unescape is total");
                assert_eq!(&decoded, x_value, "{ctx}: X-COMPAT text value");
                match &spec.x_param {
                    Some(x_param) => {
                        assert_eq!(
                            prop.param_value("X-PARAM"),
                            Some(x_param.as_str()),
                            "{ctx}: X-PARAM decoded value"
                        );
                    }
                    None => assert!(prop.param("X-PARAM").is_none()),
                }
            }
            None => assert!(
                event.prop("X-COMPAT").is_none(),
                "{ctx}: unexpected X-COMPAT"
            ),
        }
    }

    // Round-trip: our serialization of the parsed model, reparsed strictly,
    // reproduces the model exactly.
    let ours = write_document(&parsed.components, &WriteOptions::default());
    let reparsed = parse(&ours, &ParseOptions::strict())
        .unwrap_or_else(|e| panic!("our rewrite failed strict parse: {e}\n{ours}"));
    assert_eq!(reparsed.components, parsed.components);
    assert!(reparsed.repairs.is_empty());
}
