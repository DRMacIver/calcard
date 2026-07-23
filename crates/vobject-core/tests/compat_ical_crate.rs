//! Cross-implementation writer/parser check: documents serialized by
//! vobject-core must be parsed by the `ical` crate (IcalParser/VcardParser)
//! with the same structure — component names, property names, parameter
//! lists and values.
//!
//! Comparison level (determined empirically from ical 0.11 sources and
//! verified by these tests):
//!
//! - Property values are compared as RAW WIRE TEXT: the `ical` crate does
//!   not unescape TEXT values (its `PropertyParser` stores the substring
//!   after ':' untouched), so `\,`, `\;`, `\n` and `\\` sequences must come
//!   back byte-for-byte. An empty value comes back as `None`.
//! - Parameter values are compared after applying our RFC 6868 caret
//!   encoding to the decoded model value: the `ical` crate strips the
//!   surrounding DQUOTEs but does not decode `^^`/`^n`/`^'`, so its
//!   parameter value equals `caret_encode(model value)`.
//! - Parameter names are compared case-insensitively (their parser
//!   uppercases them).
//! - vCard groups are not split by the `ical` crate: `item1.EMAIL` comes
//!   back as the property NAME "item1.EMAIL".
//!
//! Known deviations (generator constraints, each justified):
//!
//! 1. No physical line may end in whitespace: the `ical` crate's
//!    `LineReader` calls `trim_end()` on every physical line, deleting
//!    trailing spaces/tabs that RFC 5545/6350 treat as significant value
//!    octets. Values are generated without trailing whitespace and any
//!    document whose folded form puts a space at a physical line end is
//!    skipped (`tc.assume`), since folding may legally split just after a
//!    space.
//! 2. No vCard 2.1 bare parameters (`TEL;HOME:...`): the `ical` crate
//!    requires every parameter to have `=` and errors with
//!    MissingDelimiter otherwise. Bare params are a vCard 2.1 legacy form,
//!    not RFC 6350 syntax, so they are simply not generated here (they are
//!    covered by vobject-core's own round-trip suite).
//! 3. Component names are drawn from the fixed RFC 5545 vocabulary the
//!    `ical` crate models (VCALENDAR/VEVENT/VTODO/VJOURNAL/VFREEBUSY/
//!    VTIMEZONE/VALARM/STANDARD/DAYLIGHT): it rejects any other
//!    subcomponent with InvalidComponent, and its `IcalParser` header check
//!    is case-sensitive, so the exact uppercase spellings are used.
//! 4. At most 10 values per parameter: the `ical` crate's parameter-value
//!    loop is hard-capped at 10 iterations. We generate at most 3, so this
//!    never binds, but it is a real limit of theirs.
//! 5. No empty parameter value in an interior position (`P=0,,0`): the
//!    `ical` crate's parser advances with `trim_start_matches(',')`, which
//!    eats every comma in a run, so any `,,` on the wire collapses and the
//!    interior empty value disappears (`["0", "", "0"]` parses as
//!    `["0", "0"]`). RFC 5545 param-value may be empty, so `,,` is a
//!    legitimate empty value; the generator only keeps empty values in
//!    first or last position, where a single separator survives their
//!    parser.
//! 6. No value starting with ':': the `ical` crate splits off the value
//!    with `trim_start_matches(':')`, which eats the whole leading colon
//!    run, so `A:::x` (value "::x") parses as "x" and `A::` (value ":")
//!    as `None`. A leading ':' in a TEXT value is legal RFC 5545/6350
//!    wire, so the generator simply never produces one.

use hegel::generators;
use vobject_core::escape::{caret_encode, escape_text};
use vobject_core::model::{Component, Param, Property};
use vobject_core::{write_document, WriteOptions};

// ---------------------------------------------------------------------------
// Generators

/// A property name: uppercase, never BEGIN/END (structural for both sides).
fn draw_name(tc: &hegel::TestCase) -> String {
    let name = tc.draw(generators::from_regex(r"[A-Z][A-Z0-9-]{0,9}").fullmatch(true));
    if name == "BEGIN" || name == "END" {
        format!("X-{name}")
    } else {
        name
    }
}

/// Decoded text for a property value. Excludes CR and control characters
/// other than '\n' (which TEXT escaping carries as "\n"), and trailing
/// whitespace other than '\n' (deviation 1).
fn draw_text(tc: &hegel::TestCase) -> String {
    let mut s: String = tc
        .draw(generators::text().max_size(80))
        .chars()
        .filter(|c| (!c.is_control() || *c == '\n' || *c == '\t') && *c != '\r')
        .collect();
    while s.ends_with(|c: char| c.is_whitespace() && c != '\n') {
        s.pop();
    }
    s
}

/// A raw property value: either escaped TEXT (exercising `\,` `\;` `\n`
/// `\\` and Unicode) or a simple token like a date-time or version number.
fn draw_value(tc: &hegel::TestCase) -> String {
    let value = if tc.draw(generators::booleans()) {
        escape_text(&draw_text(tc))
    } else {
        tc.draw(generators::sampled_from(vec![
            "2.0",
            "20240101T000000Z",
            "20240101",
            "PT1H30M",
            "-//vobject//compat//EN",
            "mailto:alice@example.com",
            "",
        ]))
        .to_string()
    };
    // No leading ':' (deviation 6).
    value.trim_start_matches(':').to_string()
}

/// A decoded parameter value: no CR (unrepresentable through RFC 6868),
/// no trailing whitespace (deviation 1; the closing '"' protects quoted
/// values, but unquoted ones can land at a fold boundary).
fn draw_param_value(tc: &hegel::TestCase) -> String {
    let mut s: String = tc
        .draw(generators::text().max_size(30))
        .chars()
        .filter(|c| (!c.is_control() || *c == '\n' || *c == '\t') && *c != '\r')
        .collect();
    while s.ends_with(|c: char| c.is_whitespace() && c != '\n') {
        s.pop();
    }
    s
}

fn draw_property(tc: &hegel::TestCase, allow_group: bool) -> Property {
    let mut prop = Property::new(draw_name(tc), draw_value(tc));
    if allow_group && tc.draw(generators::booleans()) {
        prop.group = Some(tc.draw(generators::from_regex(r"[A-Za-z0-9-]{1,6}").fullmatch(true)));
    }
    let n_params = tc.draw(generators::integers::<usize>().max_value(3));
    for _ in 0..n_params {
        // Always >= 1 value: bare params are excluded (deviation 2).
        let n_values = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
        let drawn: Vec<String> = (0..n_values).map(|_| draw_param_value(tc)).collect();
        // No interior empty values (deviation 5).
        let last = drawn.len() - 1;
        let values: Vec<String> = drawn
            .into_iter()
            .enumerate()
            .filter(|(i, v)| !v.is_empty() || *i == 0 || *i == last)
            .map(|(_, v)| v)
            .collect();
        prop.params.push(Param {
            name: draw_name(tc),
            values,
        });
    }
    prop
}

fn draw_properties(tc: &hegel::TestCase, comp: &mut Component, max: usize, groups: bool) {
    let n = tc.draw(generators::integers::<usize>().max_value(max));
    for _ in 0..n {
        comp.push_property(draw_property(tc, groups));
    }
}

/// A VCALENDAR shaped to the component vocabulary the ical crate models
/// (deviation 3).
fn draw_calendar(tc: &hegel::TestCase) -> Component {
    let mut cal = Component::new("VCALENDAR");
    draw_properties(tc, &mut cal, 3, false);
    let n_subs = tc.draw(generators::integers::<usize>().max_value(4));
    for _ in 0..n_subs {
        let kind = tc.draw(generators::sampled_from(vec![
            "VEVENT",
            "VTODO",
            "VJOURNAL",
            "VFREEBUSY",
            "VTIMEZONE",
        ]));
        let mut sub = Component::new(kind);
        draw_properties(tc, &mut sub, 3, false);
        match kind {
            "VEVENT" | "VTODO" => {
                let n_alarms = tc.draw(generators::integers::<usize>().max_value(2));
                for _ in 0..n_alarms {
                    let mut alarm = Component::new("VALARM");
                    draw_properties(tc, &mut alarm, 2, false);
                    sub.push_component(alarm);
                }
            }
            "VTIMEZONE" => {
                let n_transitions = tc.draw(generators::integers::<usize>().max_value(2));
                for _ in 0..n_transitions {
                    let name = tc.draw(generators::sampled_from(vec!["STANDARD", "DAYLIGHT"]));
                    let mut tr = Component::new(name);
                    draw_properties(tc, &mut tr, 2, false);
                    sub.push_component(tr);
                }
            }
            _ => {}
        }
        cal.push_component(sub);
    }
    cal
}

/// Serialize, then skip the (rare, legal) documents whose folding puts a
/// whitespace octet at the end of a physical line (deviation 1).
fn write_for_ical_crate(tc: &hegel::TestCase, components: &[Component]) -> String {
    let options = if tc.draw(generators::booleans()) {
        WriteOptions::default()
    } else {
        WriteOptions {
            fold_width: None,
            ..WriteOptions::default()
        }
    };
    let wire = write_document(components, &options);
    let hazard = wire
        .split("\r\n")
        .any(|line| line.ends_with(|c: char| c.is_whitespace()));
    tc.assume(!hazard);
    wire
}

// ---------------------------------------------------------------------------
// Comparison helpers

/// What the ical crate should report for one of our properties.
fn expected_property(
    prop: &Property,
) -> (String, Option<Vec<(String, Vec<String>)>>, Option<String>) {
    let name = match &prop.group {
        Some(g) => format!("{g}.{}", prop.name),
        None => prop.name.clone(),
    };
    let params = if prop.params.is_empty() {
        None
    } else {
        Some(
            prop.params
                .iter()
                .map(|p| {
                    (
                        p.name.to_uppercase(),
                        p.values.iter().map(|v| caret_encode(v)).collect(),
                    )
                })
                .collect(),
        )
    };
    let value = if prop.value.is_empty() {
        None
    } else {
        Some(prop.value.clone())
    };
    (name, params, value)
}

fn check_properties(ours: &Component, theirs: &[ical::property::Property], ctx: &str) {
    let expected: Vec<_> = ours.properties().map(expected_property).collect();
    let got: Vec<_> = theirs
        .iter()
        .map(|p| (p.name.clone(), p.params.clone(), p.value.clone()))
        .collect();
    assert_eq!(got, expected, "properties diverged in {ctx}");
}

// ---------------------------------------------------------------------------
// iCalendar

#[hegel::test(test_cases = 200)]
fn ical_crate_parses_our_icalendar_output(tc: hegel::TestCase) {
    let n_cals = tc.draw(generators::integers::<usize>().min_value(1).max_value(2));
    let calendars: Vec<Component> = (0..n_cals).map(|_| draw_calendar(&tc)).collect();
    let wire = write_for_ical_crate(&tc, &calendars);

    let parsed: Vec<_> = ical::IcalParser::new(wire.as_bytes()).collect();
    assert_eq!(
        parsed.len(),
        calendars.len(),
        "calendar count diverged\nwire:\n{wire}"
    );

    for (ours, theirs) in calendars.iter().zip(parsed) {
        let theirs = theirs.unwrap_or_else(|e| panic!("ical crate failed: {e}\nwire:\n{wire}"));
        check_properties(ours, &theirs.properties, "VCALENDAR");

        let events: Vec<&Component> = ours.comps("VEVENT").collect();
        let todos: Vec<&Component> = ours.comps("VTODO").collect();
        let journals: Vec<&Component> = ours.comps("VJOURNAL").collect();
        let free_busys: Vec<&Component> = ours.comps("VFREEBUSY").collect();
        let timezones: Vec<&Component> = ours.comps("VTIMEZONE").collect();

        assert_eq!(theirs.events.len(), events.len(), "event count");
        assert_eq!(theirs.todos.len(), todos.len(), "todo count");
        assert_eq!(theirs.journals.len(), journals.len(), "journal count");
        assert_eq!(theirs.free_busys.len(), free_busys.len(), "freebusy count");
        assert_eq!(theirs.timezones.len(), timezones.len(), "timezone count");

        for (ev, their_ev) in events.iter().zip(&theirs.events) {
            check_properties(ev, &their_ev.properties, "VEVENT");
            let alarms: Vec<&Component> = ev.comps("VALARM").collect();
            assert_eq!(their_ev.alarms.len(), alarms.len(), "alarm count");
            for (al, their_al) in alarms.iter().zip(&their_ev.alarms) {
                check_properties(al, &their_al.properties, "VALARM");
            }
        }
        for (todo, their_todo) in todos.iter().zip(&theirs.todos) {
            check_properties(todo, &their_todo.properties, "VTODO");
            let alarms: Vec<&Component> = todo.comps("VALARM").collect();
            assert_eq!(their_todo.alarms.len(), alarms.len(), "todo alarm count");
            for (al, their_al) in alarms.iter().zip(&their_todo.alarms) {
                check_properties(al, &their_al.properties, "VALARM");
            }
        }
        for (j, their_j) in journals.iter().zip(&theirs.journals) {
            check_properties(j, &their_j.properties, "VJOURNAL");
        }
        for (fb, their_fb) in free_busys.iter().zip(&theirs.free_busys) {
            check_properties(fb, &their_fb.properties, "VFREEBUSY");
        }
        for (tz, their_tz) in timezones.iter().zip(&theirs.timezones) {
            check_properties(tz, &their_tz.properties, "VTIMEZONE");
            let transitions: Vec<&Component> = tz
                .components()
                .filter(|c| c.is("STANDARD") || c.is("DAYLIGHT"))
                .collect();
            assert_eq!(
                their_tz.transitions.len(),
                transitions.len(),
                "transition count"
            );
            for (tr, their_tr) in transitions.iter().zip(&their_tz.transitions) {
                use ical::parser::ical::component::IcalTimeZoneTransitionType as T;
                let expected_standard = tr.is("STANDARD");
                let is_standard = matches!(their_tr.transition, T::STANDARD);
                assert_eq!(is_standard, expected_standard, "transition kind");
                check_properties(tr, &their_tr.properties, "VTIMEZONE transition");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// vCard

#[hegel::test(test_cases = 200)]
fn ical_crate_parses_our_vcard_output(tc: hegel::TestCase) {
    let n_cards = tc.draw(generators::integers::<usize>().min_value(1).max_value(2));
    let cards: Vec<Component> = (0..n_cards)
        .map(|_| {
            let mut card = Component::new("VCARD");
            // At least one property: an empty VCARD parses fine, but keep
            // documents representative.
            card.push_property(Property::new("VERSION", "4.0"));
            draw_properties(&tc, &mut card, 5, true);
            card
        })
        .collect();
    let wire = write_for_ical_crate(&tc, &cards);

    let parsed: Vec<_> = ical::VcardParser::new(wire.as_bytes()).collect();
    assert_eq!(
        parsed.len(),
        cards.len(),
        "card count diverged\nwire:\n{wire}"
    );

    for (ours, theirs) in cards.iter().zip(parsed) {
        let theirs = theirs.unwrap_or_else(|e| panic!("ical crate failed: {e}\nwire:\n{wire}"));
        check_properties(ours, &theirs.properties, "VCARD");
    }
}
