//! Property-based tests (hegel) for the syntax layer.
//!
//! The key properties, each grounded in a documented contract:
//!
//! 1. Lenient parsing is total: it never panics and never errors, on any
//!    string whatsoever.
//! 2. If strict parsing succeeds, lenient parsing produces the identical
//!    document with zero repairs.
//! 3. If lenient parsing records zero repairs, strict parsing succeeds and
//!    produces the identical document ("zero repairs ⟺ strictly clean").
//! 4. Serializing any well-formed model and strict-parsing it back is the
//!    identity (model → wire → model).
//! 5. Serialization is faithful to the parsed model: leniently reparsing
//!    the writer's output reproduces the model (wire → model → wire → model).
//! 6. fold/unfold are inverses, and folded output respects the width limit.
//! 7. TEXT escaping, RFC 6868 caret encoding, and escaped-list splitting
//!    are invertible.

use hegel::generators;
use vobject_core::escape::{
    caret_decode, caret_encode, escape_text, split_unescaped, unescape_text,
};
use vobject_core::model::{Child, Component, Param, Property};
use vobject_core::write::property_line;
use vobject_core::{parse, write_document, ParseOptions, WriteOptions};

// ---------------------------------------------------------------------------
// Generators

/// A strict-grammar name: 1*(ALPHA / DIGIT / "-").
fn draw_name(tc: &hegel::TestCase) -> String {
    tc.draw(generators::from_regex(r"[A-Za-z0-9-]{1,12}").fullmatch(true))
}

/// A strict-grammar property name that is not BEGIN or END (those are
/// structural and cannot appear as ordinary properties in the model).
fn draw_prop_name(tc: &hegel::TestCase) -> String {
    let name = draw_name(tc);
    if name.eq_ignore_ascii_case("BEGIN") || name.eq_ignore_ascii_case("END") {
        format!("{name}X")
    } else {
        name
    }
}

/// Arbitrary text usable as a raw property value on a strict content line:
/// no control characters other than TAB (values live on one logical line,
/// so they can never contain newlines).
fn draw_value(tc: &hegel::TestCase) -> String {
    tc.draw(generators::text().max_size(60))
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect()
}

/// Arbitrary text usable as a decoded parameter value. Newlines are allowed
/// (RFC 6868 carries them via ^n); a lone CR is not representable (it
/// normalizes to LF through caret encoding) so it is excluded.
fn draw_param_value(tc: &hegel::TestCase) -> String {
    tc.draw(generators::text().max_size(40))
        .chars()
        .filter(|c| !c.is_control() || *c == '\t' || *c == '\n')
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
            values: (0..n_values).map(|_| draw_param_value(tc)).collect(),
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

/// Text that looks vaguely like a vobject document: random structural
/// fragments mixed with junk, using both CRLF and bare LF. Pure random text
/// almost never exercises the tree-building code, so robustness and
/// idempotence tests mix this in.
fn draw_documentish(tc: &hegel::TestCase) -> String {
    let n_lines = tc.draw(generators::integers::<usize>().max_value(15));
    let mut out = String::new();
    for _ in 0..n_lines {
        let kind = tc.draw(generators::integers::<u8>().max_value(9));
        match kind {
            0 => out.push_str("BEGIN:VCARD"),
            1 => out.push_str("END:VCARD"),
            2 => out.push_str("BEGIN:VCALENDAR"),
            3 => out.push_str("END:VCALENDAR"),
            4 => out.push_str(&format!(" {}", draw_value(tc))),
            5 => out.push_str("SUMMARY;TZID=\"a,b\";X=1,2:hello\\n world"),
            6 => out.push_str("NOTE;ENCODING=QUOTED-PRINTABLE:soft break="),
            7 => out.push_str("TEL;HOME;VOICE:+441234"),
            _ => out.push_str(&tc.draw(generators::text().max_size(30))),
        }
        out.push_str(if tc.draw(generators::booleans()) {
            "\r\n"
        } else {
            "\n"
        });
    }
    out
}

/// Either completely arbitrary text or document-ish text.
fn draw_input(tc: &hegel::TestCase) -> String {
    if tc.draw(generators::booleans()) {
        tc.draw(generators::text().max_size(400))
    } else {
        draw_documentish(tc)
    }
}

fn all_properties(comp: &Component) -> Vec<&Property> {
    let mut out = Vec::new();
    let mut stack = vec![comp];
    while let Some(c) = stack.pop() {
        for child in &c.children {
            match child {
                Child::Property(p) => out.push(p),
                Child::Component(k) => stack.push(k),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Totality

#[hegel::test(test_cases = 500)]
fn lenient_parse_is_total(tc: hegel::TestCase) {
    let input = draw_input(&tc);
    let parsed = parse(&input, &ParseOptions::lenient());
    assert!(
        parsed.is_ok(),
        "lenient parse failed on {input:?}: {:?}",
        parsed.unwrap_err()
    );
}

#[hegel::test(test_cases = 500)]
fn strict_parse_never_panics(tc: hegel::TestCase) {
    let input = draw_input(&tc);
    // Err is fine; panicking is not.
    let _ = parse(&input, &ParseOptions::strict());
}

// ---------------------------------------------------------------------------
// 2 & 3. Strict and lenient agree on the boundary

#[hegel::test(test_cases = 500)]
fn strict_success_implies_lenient_identical_with_no_repairs(tc: hegel::TestCase) {
    let input = draw_input(&tc);
    if let Ok(strict) = parse(&input, &ParseOptions::strict()) {
        let lenient = parse(&input, &ParseOptions::lenient()).unwrap();
        assert_eq!(strict.components, lenient.components);
        assert_eq!(
            lenient.repairs,
            vec![],
            "strictly-valid input produced repairs"
        );
    }
}

#[hegel::test(test_cases = 500)]
fn zero_repairs_implies_strict_success(tc: hegel::TestCase) {
    let input = draw_input(&tc);
    let lenient = parse(&input, &ParseOptions::lenient()).unwrap();
    if lenient.repairs.is_empty() {
        let strict = parse(&input, &ParseOptions::strict());
        match strict {
            Ok(strict) => assert_eq!(strict.components, lenient.components),
            Err(e) => {
                panic!("lenient parse of {input:?} had no repairs but strict parse failed: {e}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Model round-trip through the wire

#[hegel::test(test_cases = 300)]
fn model_survives_write_then_strict_parse(tc: hegel::TestCase) {
    let n_roots = tc.draw(generators::integers::<usize>().min_value(1).max_value(3));
    let components: Vec<Component> = (0..n_roots).map(|_| draw_component(&tc, 3)).collect();

    let wire = write_document(&components, &WriteOptions::default());
    let parsed = parse(&wire, &ParseOptions::strict())
        .unwrap_or_else(|e| panic!("writer output failed strict parse: {e}\n{wire}"));
    assert_eq!(parsed.components, components, "wire form:\n{wire}");
    assert!(parsed.repairs.is_empty());

    // Folded physical lines respect the 75-octet limit.
    for line in wire.split("\r\n") {
        assert!(line.len() <= 75, "line too long ({}): {line:?}", line.len());
    }
}

// ---------------------------------------------------------------------------
// 5. Anything parseable serializes faithfully

#[hegel::test(test_cases = 500)]
fn parsed_model_survives_write_then_lenient_reparse(tc: hegel::TestCase) {
    let input = draw_input(&tc);
    let first = parse(&input, &ParseOptions::lenient()).unwrap();

    // The vCard 2.1 quoted-printable soft-break heuristic makes the wire
    // format genuinely ambiguous: a property whose parameter section
    // mentions QUOTED-PRINTABLE and whose value ends in '=' will re-join
    // with the following line on reparse. Exclude that corner.
    let qp_hazard = first.components.iter().any(|c| {
        all_properties(c).into_iter().any(|p| {
            p.value.contains('=')
                && property_line(p)
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .contains("QUOTED-PRINTABLE")
        })
    });
    tc.assume(!qp_hazard);

    let wire = write_document(&first.components, &WriteOptions::default());
    let second = parse(&wire, &ParseOptions::lenient()).unwrap();
    assert_eq!(
        second.components, first.components,
        "input: {input:?}\nwire: {wire:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. Folding

#[hegel::test(test_cases = 500)]
fn fold_unfold_round_trip(tc: hegel::TestCase) {
    // A logical line: starts with a name-ish character, no newlines.
    let body: String = tc
        .draw(generators::text().max_size(300))
        .chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .collect();
    let line = format!("X:{body}");

    let folded = vobject_core::fold::fold(&line);
    for physical in folded.split("\r\n") {
        assert!(physical.len() <= 75);
    }
    let unfolded = vobject_core::lines::unfold(&folded, None).unwrap();
    assert_eq!(unfolded.len(), 1);
    assert_eq!(unfolded[0].text, line);
}

// ---------------------------------------------------------------------------
// 7. Escaping inverses

#[hegel::test(test_cases = 500)]
fn text_escape_round_trip(tc: hegel::TestCase) {
    // A lone CR is not representable in TEXT (it normalizes to \n), so it
    // is excluded from the input domain.
    let s: String = tc
        .draw(generators::text().max_size(200))
        .chars()
        .filter(|c| *c != '\r')
        .collect();
    let escaped = escape_text(&s);
    // Escaped text is safe to put on a content line: no raw newlines.
    assert!(!escaped.contains('\n') && !escaped.contains('\r'));
    let back = unescape_text(&escaped, None, 1).unwrap();
    assert_eq!(back, s);
}

#[hegel::test(test_cases = 500)]
fn unescape_is_total_in_lenient_mode(tc: hegel::TestCase) {
    let s = tc.draw(generators::text().max_size(200));
    let mut repairs = Vec::new();
    // Must never fail or panic, whatever the escapes look like.
    unescape_text(&s, Some(&mut repairs), 1).unwrap();
}

#[hegel::test(test_cases = 500)]
fn caret_round_trip(tc: hegel::TestCase) {
    let s: String = tc
        .draw(generators::text().max_size(200))
        .chars()
        .filter(|c| *c != '\r')
        .collect();
    let encoded = caret_encode(&s);
    assert!(!encoded.contains('\n') && !encoded.contains('"'));
    assert_eq!(caret_decode(&encoded), s);
}

#[hegel::test(test_cases = 500)]
fn caret_decode_is_total(tc: hegel::TestCase) {
    let s = tc.draw(generators::text().max_size(200));
    let _ = caret_decode(&s);
}

// ---------------------------------------------------------------------------
// 8. Value layer and recurrence

/// Value round-trip: parse a raw value as a type, serialize it back, parse
/// again — must be identical (serialize∘parse is idempotent on its image).
#[hegel::test(test_cases = 500)]
fn typed_value_reparse_is_identity(tc: hegel::TestCase) {
    use vobject_core::value::{parse_value, serialize_value, Multiplicity, TypeInfo, ValueType};
    let vtype = tc.draw(generators::sampled_from(vec![
        ValueType::Text,
        ValueType::Date,
        ValueType::DateTime,
        ValueType::Duration,
        ValueType::Period,
        ValueType::Integer,
        ValueType::Boolean,
        ValueType::UtcOffset,
        ValueType::Binary,
    ]));
    let multiplicity = if tc.draw(generators::booleans()) {
        Multiplicity::Single
    } else {
        Multiplicity::CommaList
    };
    let info = TypeInfo {
        vtype,
        multiplicity,
    };
    // Values in the model come from unfolded content lines and can never
    // contain a raw CR (it is a line terminator); a lone CR is documented
    // as not representable in TEXT escaping.
    let raw: String = tc
        .draw(generators::text().max_size(60))
        .chars()
        .filter(|c| *c != '\r')
        .collect();
    if let Ok(value) = parse_value(&raw, info) {
        let serialized = serialize_value(&value);
        let reparsed = parse_value(&serialized, info).unwrap_or_else(|e| {
            panic!("serialized form {serialized:?} of {value:?} failed to reparse: {e}")
        });
        assert_eq!(reparsed, value, "via {serialized:?}");
    }
}

/// Every instance an RRULE expansion emits is on/after DTSTART, strictly
/// increasing, bounded by COUNT, and never panics — for arbitrary rules.
#[hegel::test(test_cases = 300)]
fn rrule_expansion_is_sane(tc: hegel::TestCase) {
    use vobject_core::rrule::{expand, ExpandLimits};
    use vobject_core::value::{DateOrDateTime, Recur};

    // Build a random-but-parseable rule.
    let freq = tc.draw(generators::sampled_from(vec![
        "SECONDLY", "MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "YEARLY",
    ]));
    let mut rule = format!("FREQ={freq}");
    let count = tc.draw(generators::integers::<u8>().min_value(1).max_value(20));
    rule.push_str(&format!(";COUNT={count}"));
    if tc.draw(generators::booleans()) {
        // Full-range intervals: hostile magnitudes (u64::MAX once wrapped a
        // cast negative and panicked the stepper) must terminate cleanly,
        // not just the polite 1..=5 ones.
        let interval: u64 = if tc.draw(generators::booleans()) {
            tc.draw(generators::integers::<u8>().min_value(1).max_value(5)) as u64
        } else {
            tc.draw(generators::integers::<u64>().min_value(1))
        };
        rule.push_str(&format!(";INTERVAL={interval}"));
    }
    if tc.draw(generators::booleans()) {
        let day = tc.draw(generators::sampled_from(vec![
            "MO", "TU", "WE", "TH", "FR", "SA", "SU", "1MO", "-1FR", "2TU", "-2SA", "53MO", "-53FR",
        ]));
        rule.push_str(&format!(";BYDAY={day}"));
    }
    if tc.draw(generators::booleans()) {
        let m = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
        rule.push_str(&format!(";BYMONTH={m}"));
    }
    if tc.draw(generators::booleans()) {
        let md = tc.draw(generators::integers::<i8>().min_value(-31).max_value(31));
        if md != 0 {
            rule.push_str(&format!(";BYMONTHDAY={md}"));
        }
    }
    if tc.draw(generators::booleans()) {
        let wn = tc.draw(generators::integers::<i8>().min_value(-53).max_value(53));
        if wn != 0 {
            rule.push_str(&format!(";BYWEEKNO={wn}"));
        }
    }
    if tc.draw(generators::booleans()) {
        let yd = tc.draw(generators::integers::<i16>().min_value(-366).max_value(366));
        if yd != 0 {
            rule.push_str(&format!(";BYYEARDAY={yd}"));
        }
    }
    if tc.draw(generators::booleans()) {
        let h = tc.draw(generators::integers::<u8>().max_value(23));
        rule.push_str(&format!(";BYHOUR={h}"));
    }
    if tc.draw(generators::booleans()) {
        let m = tc.draw(generators::integers::<u8>().max_value(59));
        rule.push_str(&format!(";BYMINUTE={m}"));
    }
    if tc.draw(generators::booleans()) {
        let sp = tc.draw(generators::integers::<i8>().min_value(-5).max_value(5));
        if sp != 0 {
            rule.push_str(&format!(";BYSETPOS={sp}"));
        }
    }

    // The full representable year range, weighted toward the modern era but
    // deliberately including both edges (year-0 week math underflowed and
    // year-9999 stepping overflowed before the hardening fixes).
    let year = if tc.draw(generators::booleans()) {
        tc.draw(
            generators::integers::<i32>()
                .min_value(1970)
                .max_value(2100),
        )
    } else {
        tc.draw(generators::integers::<i32>().min_value(0).max_value(9999))
    };
    let month = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
    let day = tc.draw(generators::integers::<u8>().min_value(1).max_value(28));
    let hour = tc.draw(generators::integers::<u8>().max_value(23));
    let dtstart_s = format!("{year:04}{month:02}{day:02}T{hour:02}0000");
    let dtstart = DateOrDateTime::parse(&dtstart_s).unwrap();

    let recur = Recur::parse(&rule).unwrap();
    let instances: Vec<_> = expand(&recur, dtstart, ExpandLimits::default())
        .unwrap()
        .take(count as usize + 5)
        .collect();

    assert!(
        instances.len() <= count as usize,
        "{rule} from {dtstart_s}: COUNT exceeded ({})",
        instances.len()
    );
    let epochs: Vec<i64> = instances
        .iter()
        .map(|d| match d {
            DateOrDateTime::Date(d) => d.to_ordinal() * 86400,
            DateOrDateTime::DateTime(dt) => dt.to_epoch_like(),
        })
        .collect();
    let start_epoch = match dtstart {
        DateOrDateTime::Date(d) => d.to_ordinal() * 86400,
        DateOrDateTime::DateTime(dt) => dt.to_epoch_like(),
    };
    for window in epochs.windows(2) {
        assert!(window[0] < window[1], "{rule}: instances not increasing");
    }
    if let Some(first) = epochs.first() {
        assert!(*first >= start_epoch, "{rule}: instance before DTSTART");
    }
}

#[hegel::test(test_cases = 500)]
fn split_unescaped_inverts_escaped_join(tc: hegel::TestCase) {
    let n = tc.draw(generators::integers::<usize>().min_value(1).max_value(6));
    let pieces: Vec<String> = (0..n)
        .map(|_| {
            tc.draw(generators::text().max_size(30))
                .chars()
                .filter(|c| *c != '\r')
                .collect()
        })
        .collect();
    let escaped: Vec<String> = pieces.iter().map(|p| escape_text(p)).collect();
    let joined = escaped.join(",");
    let split: Vec<&str> = split_unescaped(&joined, ',');
    assert_eq!(split, escaped);
    // And each piece unescapes to the original.
    for (piece, original) in split.iter().zip(&pieces) {
        assert_eq!(&unescape_text(piece, None, 1).unwrap(), original);
    }
}

/// DURATION parsing is total (never panics, on any string), and every
/// accepted duration has a representable total_seconds that survives the
/// display round trip.
#[hegel::test(test_cases = 500)]
fn duration_parse_is_total(tc: hegel::TestCase) {
    use vobject_core::value::Duration;

    // Half adversarial-but-shaped (digit runs that stress magnitude
    // handling), half arbitrary text.
    let s = if tc.draw(generators::booleans()) {
        let sign = tc.draw(generators::sampled_from(vec!["", "-", "+"]));
        let n = tc.draw(generators::integers::<u128>());
        let unit = tc.draw(generators::sampled_from(vec!["W", "D", "H", "M", "S"]));
        let t = if matches!(unit, "H" | "M" | "S") {
            "T"
        } else {
            ""
        };
        format!("{sign}P{t}{n}{unit}")
    } else {
        tc.draw(generators::text().max_size(30))
    };
    if let Ok(d) = Duration::parse(&s) {
        let total = d
            .checked_total_seconds()
            .expect("parsed duration overflows");
        let redisplayed = Duration::parse(&d.to_string()).unwrap();
        assert_eq!(redisplayed.total_seconds(), total, "{s:?}");
    }
}

/// RSCALE=GREGORIAN with SKIP=OMIT is RFC 5545 semantics by definition
/// (RFC 7529 §4.1), so the RSCALE engine must agree with the Gregorian
/// engine on arbitrary YEARLY/MONTHLY rules. This is the oracle that
/// catches any BY-part the RSCALE path drops or mishandles.
#[hegel::test(test_cases = 300)]
fn rscale_gregorian_matches_gregorian_engine(tc: hegel::TestCase) {
    use vobject_core::rrule::{expand, ExpandLimits, Expansion};
    use vobject_core::rscale::expand_rscale;
    use vobject_core::value::{DateOrDateTime, Recur};

    let freq = tc.draw(generators::sampled_from(vec!["MONTHLY", "YEARLY"]));
    let mut rule = format!("FREQ={freq}");
    let count = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
    rule.push_str(&format!(";COUNT={count}"));
    if tc.draw(generators::booleans()) {
        let interval = tc.draw(generators::integers::<u8>().min_value(1).max_value(4));
        rule.push_str(&format!(";INTERVAL={interval}"));
    }
    if tc.draw(generators::booleans()) {
        let day = tc.draw(generators::sampled_from(vec![
            "MO", "TU", "FR", "SU", "1MO", "2SU", "3WE", "4TH", "-1FR", "-2SA",
        ]));
        rule.push_str(&format!(";BYDAY={day}"));
    }
    if tc.draw(generators::booleans()) {
        let m = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
        rule.push_str(&format!(";BYMONTH={m}"));
    }
    if tc.draw(generators::booleans()) {
        let md = tc.draw(generators::integers::<i8>().min_value(-28).max_value(28));
        if md != 0 {
            rule.push_str(&format!(";BYMONTHDAY={md}"));
        }
    }
    if tc.draw(generators::booleans()) {
        let h = tc.draw(generators::integers::<u8>().max_value(23));
        rule.push_str(&format!(";BYHOUR={h}"));
    }
    if tc.draw(generators::booleans()) {
        let sp = tc.draw(generators::integers::<i8>().min_value(-3).max_value(3));
        if sp != 0 {
            rule.push_str(&format!(";BYSETPOS={sp}"));
        }
    }

    let year = tc.draw(
        generators::integers::<i32>()
            .min_value(1990)
            .max_value(2080),
    );
    let month = tc.draw(generators::integers::<u8>().min_value(1).max_value(12));
    let day = tc.draw(generators::integers::<u8>().min_value(1).max_value(28));
    let dtstart_s = format!("{year:04}{month:02}{day:02}T060000");
    let dtstart = DateOrDateTime::parse(&dtstart_s).unwrap();

    let gregorian_rule = Recur::parse(&rule).unwrap();
    let rscale_rule = Recur::parse(&format!("RSCALE=GREGORIAN;SKIP=OMIT;{rule}")).unwrap();

    let via_gregorian: Vec<String> = match expand(&gregorian_rule, dtstart, ExpandLimits::default())
        .unwrap()
    {
        Expansion::Gregorian(iter) => iter.take(count as usize).map(|d| d.to_string()).collect(),
        Expansion::Rscale(_) => panic!("plain rule must use the Gregorian engine"),
    };
    let via_rscale: Vec<String> = expand_rscale(&rscale_rule, dtstart, ExpandLimits::default())
        .unwrap()
        .into_iter()
        .take(count as usize)
        .map(|d| d.to_string())
        .collect();

    assert_eq!(via_rscale, via_gregorian, "{rule} from {dtstart_s}");
}
