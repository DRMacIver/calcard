//! jCal/jCard reading (`from_jcal`) tests: example round-trips, a corpus
//! sweep, malformed-input errors, and depth-bomb resistance.
//!
//! The corpus contract mirrors xcal_conformance.rs: converting a parsed
//! model to jCal and reading it back either reproduces the model exactly,
//! or — where the writer's documented dialect degradations lose model
//! detail (consumed VALUE params, canonicalized representations, degraded
//! unparseable values) — reaches a fixed point: writing the re-read model
//! produces the identical JSON.

use std::fs;
use std::path::{Path, PathBuf};

use hegel::generators;
use serde_json::{json, Value as Json};
use vobject_core::jcal::{from_jcal, from_jcal_value, to_jcal};
use vobject_core::model::{Component, Param, Property};
use vobject_core::{parse, ParseOptions};

fn first(input: &str) -> Component {
    parse(input, &ParseOptions::lenient())
        .unwrap()
        .components
        .remove(0)
}

// ---------------------------------------------------------------------------
// Example round-trips (model equality)

#[test]
fn simple_calendar_round_trips_to_identical_model() {
    let comp = first(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Tea \\, biscuits\r\nDTSTART;TZID=Europe/London:20260722T160000\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    );
    let j = to_jcal(&comp);
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back, vec![comp]);
}

#[test]
fn vcard_round_trips_to_identical_model() {
    let comp = first(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Example\r\nN:Example;Alice;;;\r\nBDAY:--0203\r\nitem1.EMAIL;TYPE=work:a@b.c\r\nEND:VCARD\r\n",
    );
    let j = to_jcal(&comp);
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back, vec![comp]);
}

#[test]
fn icalendar_group_prefix_round_trips() {
    let comp = first("BEGIN:VCALENDAR\r\nitem1.X-THING:hello\r\nEND:VCALENDAR\r\n");
    let j = to_jcal(&comp);
    assert_eq!(j[1][0][0], json!("item1.x-thing"));
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back[0].properties().next().unwrap().group.as_deref(), Some("item1"));
    assert_eq!(back[0].properties().next().unwrap().name, "X-THING");
}

#[test]
fn typed_values_round_trip() {
    let comp = first(
        "BEGIN:VCALENDAR\r\nDTSTART;VALUE=DATE:20260722\r\nGEO:37.386013;-122.082932\r\nREQUEST-STATUS:2.0;Success\r\nCATEGORIES:one,two\\,half\r\nFREEBUSY:19970101T180000Z/PT1H,19970102T180000Z/19970102T190000Z\r\nDURATION:PT1H30M\r\nTZOFFSETFROM:-0500\r\nPRIORITY:5\r\nX-B;VALUE=BOOLEAN:TRUE\r\nRRULE:FREQ=MONTHLY;UNTIL=20270101T000000Z;BYDAY=MO,-1FR\r\nEND:VCALENDAR\r\n",
    );
    let j = to_jcal(&comp);
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back, vec![comp]);
}

#[test]
fn value_date_param_survives() {
    let comp = first("BEGIN:VCALENDAR\r\nDTSTART;VALUE=DATE:20260722\r\nEND:VCALENDAR\r\n");
    let j = to_jcal(&comp);
    let back = from_jcal(&j.to_string()).unwrap();
    let p = back[0].prop("DTSTART").unwrap();
    assert_eq!(p.param_value("VALUE"), Some("DATE"));
    assert_eq!(p.value, "20260722");
}

#[test]
fn multivalued_params_round_trip() {
    let comp = first(
        "BEGIN:VCALENDAR\r\nATTENDEE;MEMBER=\"mailto:a@b\",\"mailto:c@d\";CN=Alice:mailto:x@y\r\nEND:VCALENDAR\r\n",
    );
    let j = to_jcal(&comp);
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back, vec![comp]);
}

#[test]
fn vcard3_dialect_detected_from_version() {
    let comp = first(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nTEL;TYPE=HOME:+441234\r\nBDAY:1996-04-15\r\nEND:VCARD\r\n",
    );
    let j = to_jcal(&comp);
    // vCard 3 TEL is phone-number, and its date passes through verbatim.
    assert_eq!(j[1][1][2], json!("phone-number"));
    let back = from_jcal(&j.to_string()).unwrap();
    assert_eq!(back, vec![comp]);
}

// ---------------------------------------------------------------------------
// Accepted document shapes

#[test]
fn jcard_two_element_form_is_accepted() {
    let back = from_jcal(r#"["vcard", [["version", {}, "text", "4.0"], ["fn", {}, "text", "Alice"]]]"#)
        .unwrap();
    assert_eq!(back.len(), 1);
    assert!(back[0].is("VCARD"));
    assert_eq!(back[0].prop("FN").unwrap().value, "Alice");
}

#[test]
fn top_level_array_of_documents_is_accepted() {
    let comps = parse(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nFN:A\r\nEND:VCARD\r\n",
        &ParseOptions::lenient(),
    )
    .unwrap()
    .components;
    let multi = Json::Array(comps.iter().map(to_jcal).collect());
    let back = from_jcal(&multi.to_string()).unwrap();
    assert_eq!(back, comps);
}

// ---------------------------------------------------------------------------
// Errors

#[test]
fn rejects_garbage_and_malformed_documents() {
    // Not JSON at all.
    assert!(from_jcal("").is_err());
    assert!(from_jcal("BEGIN:VCALENDAR").is_err());
    // JSON, but not a jCal array.
    assert!(from_jcal("{}").is_err());
    assert!(from_jcal("42").is_err());
    assert!(from_jcal("\"vcalendar\"").is_err());
    assert!(from_jcal("null").is_err());
    // Wrong root shapes.
    assert!(from_jcal("[]").is_err());
    assert!(from_jcal("[42, [], []]").is_err());
    assert!(from_jcal("[\"vcalendar\"]").is_err());
    assert!(from_jcal("[\"vcalendar\", {}, []]").is_err());
    assert!(from_jcal("[\"vcalendar\", [], {}]").is_err());
    assert!(from_jcal("[\"vcalendar\", [], [], [], []]").is_err());
    // Mixed multi-document arrays with a bad element.
    assert!(from_jcal("[[\"vcalendar\", [], []], 7]").is_err());
}

#[test]
fn rejects_bad_property_shapes() {
    let doc = |props: &str| format!("[\"vcalendar\", [{props}], []]");
    assert!(from_jcal(&doc("42")).is_err());
    assert!(from_jcal(&doc("\"summary\"")).is_err());
    assert!(from_jcal(&doc("[]")).is_err());
    assert!(from_jcal(&doc("[\"summary\"]")).is_err());
    assert!(from_jcal(&doc("[\"summary\", {}]")).is_err());
    // Type present but no value slots.
    assert!(from_jcal(&doc("[\"summary\", {}, \"text\"]")).is_err());
    // Non-string name / non-object params / non-string type.
    assert!(from_jcal(&doc("[7, {}, \"text\", \"x\"]")).is_err());
    assert!(from_jcal(&doc("[\"summary\", [], \"text\", \"x\"]")).is_err());
    assert!(from_jcal(&doc("[\"summary\", {}, 9, \"x\"]")).is_err());
}

// ---------------------------------------------------------------------------
// Depth bombs

#[test]
fn deeply_nested_json_text_errors_cleanly() {
    // 100k-deep array nesting: must return an error, not overflow the stack.
    let bomb = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(from_jcal(&bomb).is_err());
}

#[test]
fn deeply_nested_component_value_errors_cleanly() {
    // Build a 600-deep component tree as a Json value directly (the string
    // path is capped earlier by serde_json's own recursion limit).
    let mut v = json!(["vevent", [], []]);
    for _ in 0..600 {
        v = json!(["vcalendar", [], [v]]);
    }
    let err = from_jcal_value(&v).unwrap_err();
    assert!(err.message.contains("deep"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// Corpus sweep

fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let ext = path.extension().map(|e| e.to_string_lossy().to_string());
                if matches!(ext.as_deref(), Some("ics") | Some("vcf"))
                    || (path.parent().unwrap().file_name().unwrap() == "fuzz"
                        && name != "LICENSE")
                {
                    files.push(path);
                }
            }
        }
    }
    assert!(files.len() >= 500);
    files
}

#[test]
fn corpus_round_trips_through_jcal() {
    let mut converted = 0;
    let mut model_equal = 0;
    for path in corpus() {
        let text = String::from_utf8_lossy(&fs::read(&path).unwrap()).to_string();
        let parsed = parse(&text, &ParseOptions::lenient()).unwrap();
        for comp in &parsed.components {
            let j1 = to_jcal(comp);
            // Extremely deep fuzz models exceed serde_json's string-input
            // recursion limit; the value API supports the full 512 cap.
            let back = from_jcal(&j1.to_string())
                .or_else(|_| from_jcal_value(&j1))
                .unwrap_or_else(|e| {
                    panic!("from_jcal failed on {}: {e}\n{j1}", path.display())
                });
            assert_eq!(back.len(), 1, "{}", path.display());
            if back[0] == *comp {
                model_equal += 1;
            } else {
                let j2 = to_jcal(&back[0]);
                assert_eq!(
                    j2,
                    j1,
                    "jCal fixed point not reached for {}",
                    path.display()
                );
            }
            converted += 1;
        }
    }
    assert!(converted >= 500, "only {converted} components converted");
    eprintln!("{model_equal}/{converted} components round-tripped model-equal");
    // Degradation must be the exception: most real-world components
    // round-trip to the identical model.
    assert!(
        model_equal * 10 >= converted * 8,
        "only {model_equal}/{converted} components round-tripped model-equal"
    );
}

// ---------------------------------------------------------------------------
// Property tests (hegel): fixed point on random models and document-ish text

fn draw_name(tc: &hegel::TestCase) -> String {
    tc.draw(generators::from_regex(r"[A-Za-z0-9-]{1,12}").fullmatch(true))
}

fn draw_prop_name(tc: &hegel::TestCase) -> String {
    let name = draw_name(tc);
    if name.eq_ignore_ascii_case("BEGIN") || name.eq_ignore_ascii_case("END") {
        format!("{name}X")
    } else {
        name
    }
}

fn draw_value(tc: &hegel::TestCase) -> String {
    tc.draw(generators::text().max_size(60))
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect()
}

fn draw_property(tc: &hegel::TestCase) -> Property {
    let mut prop = Property::new(draw_prop_name(tc), draw_value(tc));
    if tc.draw(generators::booleans()) {
        prop.group = Some(draw_name(tc));
    }
    let n_params = tc.draw(generators::integers::<usize>().max_value(3));
    for _ in 0..n_params {
        let n_values = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
        prop.params.push(Param {
            name: draw_name(tc),
            values: (0..n_values)
                .map(|_| {
                    tc.draw(generators::text().max_size(20))
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t' || *c == '\n')
                        .collect()
                })
                .collect(),
        });
    }
    prop
}

fn draw_component(tc: &hegel::TestCase, depth: u32) -> Component {
    let mut comp = Component::new(draw_name(tc));
    let n_children = tc.draw(generators::integers::<usize>().max_value(5));
    for _ in 0..n_children {
        if depth > 0 && tc.draw(generators::booleans()) {
            comp.push_component(draw_component(tc, depth - 1));
        } else {
            comp.push_property(draw_property(tc));
        }
    }
    comp
}

/// Document-ish text biased toward typed properties, to exercise the
/// writer's degradation paths (unparseable dates, booleans, floats…).
fn draw_typed_documentish(tc: &hegel::TestCase) -> String {
    let n_lines = tc.draw(generators::integers::<usize>().max_value(15));
    let mut out = String::new();
    for _ in 0..n_lines {
        let kind = tc.draw(generators::integers::<u8>().max_value(14));
        match kind {
            0 => out.push_str("BEGIN:VCALENDAR"),
            1 => out.push_str("END:VCALENDAR"),
            2 => out.push_str("BEGIN:VCARD"),
            3 => out.push_str("END:VCARD"),
            4 => out.push_str("VERSION:2.0"),
            5 => out.push_str(&format!("DTSTART:{}", draw_value(tc))),
            6 => out.push_str("DTSTART:12:34"),
            7 => out.push_str("DTSTART;VALUE=DATE-TIME:20260722"),
            8 => out.push_str(&format!("EXDATE:{}", draw_value(tc))),
            9 => out.push_str(&format!("GEO:{}", draw_value(tc))),
            10 => out.push_str(&format!("RRULE:{}", draw_value(tc))),
            11 => out.push_str(&format!("TZOFFSETFROM:{}", draw_value(tc))),
            12 => out.push_str(&format!("PRIORITY:{}", draw_value(tc))),
            13 => out.push_str(&format!("FREEBUSY:{}", draw_value(tc))),
            _ => out.push_str(&tc.draw(generators::text().max_size(30))),
        }
        out.push_str("\r\n");
    }
    out
}

#[hegel::test(test_cases = 500)]
fn jcal_fixed_point_on_random_models(tc: hegel::TestCase) {
    let comp = draw_component(&tc, 3);
    let j1 = to_jcal(&comp);
    let back = from_jcal(&j1.to_string())
        .unwrap_or_else(|e| panic!("from_jcal failed: {e}\nmodel: {comp:?}\n{j1}"));
    assert_eq!(back.len(), 1);
    let j2 = to_jcal(&back[0]);
    assert_eq!(j2, j1, "model: {comp:?}");
}

#[hegel::test(test_cases = 500)]
fn jcal_fixed_point_on_parsed_documentish_text(tc: hegel::TestCase) {
    let input = draw_typed_documentish(&tc);
    let parsed = parse(&input, &ParseOptions::lenient()).unwrap();
    for comp in &parsed.components {
        let j1 = to_jcal(comp);
        let back = from_jcal(&j1.to_string())
            .unwrap_or_else(|e| panic!("from_jcal failed: {e}\ninput: {input:?}\n{j1}"));
        assert_eq!(back.len(), 1);
        let j2 = to_jcal(&back[0]);
        assert_eq!(j2, j1, "input: {input:?}");
    }
}
