"""Unit tests for the public Python API."""

import pytest

import calcard

SIMPLE = (
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Tea\r\n"
    "END:VEVENT\r\nEND:VCALENDAR\r\n"
)


def test_parse_and_navigate():
    doc = calcard.parse(SIMPLE)
    assert len(doc) == 1
    cal = doc.components[0]
    assert cal.name == "VCALENDAR"
    event = cal.comp("vevent")
    assert event.prop("summary").value == "Tea"
    assert doc.repairs == []


def test_document_is_iterable():
    doc = calcard.parse(SIMPLE)
    assert [c.name for c in doc] == ["VCALENDAR"]


def test_serialize_round_trip():
    doc = calcard.parse(SIMPLE)
    assert doc.serialize() == SIMPLE


# ---------------------------------------------------------------------------
# children/params/values are live lists: in-place mutation is respected.


def test_children_append_is_respected():
    comp = calcard.Component("VCARD")
    comp.children.append(calcard.Property("FN", "Bob"))
    assert [p.name for p in comp.properties()] == ["FN"]
    assert "FN:Bob" in calcard.serialize([comp])


def test_params_and_values_append_are_respected():
    prop = calcard.Property("TEL", "+441234")
    prop.params.append(calcard.Param("TYPE", ["work"]))
    prop.params[0].values.append("pref")
    comp = calcard.Component("VCARD")
    comp.children.append(prop)
    assert "TEL;TYPE=work,pref:+441234" in calcard.serialize([comp])


def test_children_is_the_same_live_list():
    comp = calcard.parse_one("BEGIN:VCARD\r\nFN:a\r\nNOTE:b\r\nEND:VCARD\r\n")
    kids = comp.children
    assert kids is comp.children
    del kids[0]
    assert [p.name for p in comp.properties()] == ["NOTE"]
    # Children are shared references, not copies.
    comp.children[0].value = "c"
    assert comp.prop("NOTE").value == "c"


def test_children_assignment_copies_the_sequence():
    comp = calcard.Component("VCARD")
    external = [calcard.Property("FN", "Bob")]
    comp.children = external
    external.append(calcard.Property("NOTE", "x"))
    assert [p.name for p in comp.properties()] == ["FN"]


def test_non_model_children_error_at_use():
    comp = calcard.Component("VCARD")
    comp.children.append(42)
    with pytest.raises(ValueError, match="Property or Component"):
        calcard.serialize([comp])


def test_param_values_must_be_strings():
    with pytest.raises(TypeError):
        calcard.Param("TYPE", [1])
    with pytest.raises(TypeError):
        calcard.Property("X", "v", params=["not a param"])


def test_serialize_accepts_document_component_and_list():
    doc = calcard.parse(SIMPLE)
    comp = doc.components[0]
    assert calcard.serialize(doc) == SIMPLE
    assert calcard.serialize(comp) == SIMPLE
    assert calcard.serialize([comp]) == SIMPLE
    assert calcard.serialize([comp, comp]) == SIMPLE + SIMPLE


def test_serialize_and_conversions_accept_typed_views():
    doc = calcard.parse(SIMPLE)
    cal = doc.calendars[0]
    assert calcard.serialize(cal) == SIMPLE
    assert calcard.to_jcal(cal) == calcard.to_jcal(cal.component)
    assert calcard.to_xcal(cal) == calcard.to_xcal(cal.component)


def test_parse_one():
    comp = calcard.parse_one(SIMPLE)
    assert comp.name == "VCALENDAR"
    with pytest.raises(calcard.ParseError):
        calcard.parse_one(SIMPLE + SIMPLE)
    with pytest.raises(calcard.ParseError):
        calcard.parse_one("")


def test_parse_one_error_carries_line_attribute():
    # Regression: the synthetic "expected exactly one component" error must
    # expose .line like core-raised ParseErrors do (None: no single line).
    for source in ("", SIMPLE + SIMPLE):
        with pytest.raises(calcard.ParseError) as excinfo:
            calcard.parse_one(source)
        assert excinfo.value.line is None


def test_strict_mode_raises_with_line_number():
    with pytest.raises(calcard.ParseError) as excinfo:
        calcard.parse("BEGIN:VCALENDAR\nEND:VCALENDAR\n", strict=True)
    assert excinfo.value.line == 1


def test_lenient_records_repairs():
    doc = calcard.parse("BEGIN:VCARD\nFN:Bob\n")
    assert doc.components[0].prop("FN").value == "Bob"
    lines = [r.line for r in doc.repairs]
    assert all(line >= 1 for line in lines)
    messages = " ".join(r.message for r in doc.repairs)
    assert "unterminated" in messages


def test_bytes_input_utf8_and_bom():
    assert calcard.parse(SIMPLE.encode()).serialize() == SIMPLE
    assert calcard.parse(b"\xef\xbb\xbf" + SIMPLE.encode()).serialize() == SIMPLE
    assert calcard.parse("﻿" + SIMPLE).serialize() == SIMPLE


def test_bytes_input_latin1_fallback():
    data = "BEGIN:VCARD\r\nFN:Rémi\r\nEND:VCARD\r\n".encode("latin-1")
    doc = calcard.parse(data)
    assert doc.components[0].prop("FN").value == "Rémi"


def test_building_a_document_from_scratch():
    event = calcard.Component(
        "VEVENT",
        [
            calcard.Property("SUMMARY", "Tea"),
            calcard.Property(
                "DTSTART",
                "20260722T160000",
                params=[calcard.Param("TZID", ["Europe/London"])],
            ),
        ],
    )
    cal = calcard.Component("VCALENDAR", [calcard.Property("VERSION", "2.0"), event])
    out = calcard.serialize(cal)
    assert "DTSTART;TZID=Europe/London:20260722T160000\r\n" in out
    reparsed = calcard.parse(out, strict=True)
    assert reparsed.components[0] == cal


def test_mutation_in_place():
    doc = calcard.parse(SIMPLE)
    doc.components[0].comp("VEVENT").prop("SUMMARY").value = "Coffee"
    assert "SUMMARY:Coffee\r\n" in doc.serialize()


def test_param_quoting_and_caret_encoding():
    prop = calcard.Property(
        "X-TEST", "v", params=[calcard.Param("A", ['needs "quotes", yes\nreally'])]
    )
    out = calcard.serialize(calcard.Component("VCARD", [prop]))
    reparsed = calcard.parse(out, strict=True).components[0]
    assert reparsed.prop("X-TEST").params[0].values == ['needs "quotes", yes\nreally']


def test_escape_helpers():
    assert calcard.escape_text("a,b;c\nd\\e") == "a\\,b\\;c\\nd\\\\e"
    assert calcard.unescape_text("a\\,b\\;c\\nd\\\\e") == "a,b;c\nd\\e"
    assert calcard.split_unescaped("a,b\\,c,d", ",") == ["a", "b\\,c", "d"]


def test_depth_limit_configurable():
    deep = "BEGIN:A\r\n" * 5 + "END:A\r\n" * 5
    with pytest.raises(calcard.ParseError):
        calcard.parse(deep, strict=True, max_depth=3)
    doc = calcard.parse(deep, max_depth=3)
    assert doc.repairs  # over-deep BEGINs dropped with repairs


def test_expand_rrule_max_years_limit():
    import datetime as dt

    out = calcard.expand_rrule(
        "FREQ=YEARLY", dt.datetime(2026, 1, 1, 9, 0), max_years=5
    )
    # Years 2026..2031 inclusive: the scan stops after dtstart year + 5.
    assert len(out) == 6
    assert out[0] == dt.datetime(2026, 1, 1, 9, 0)
    assert out[-1] == dt.datetime(2031, 1, 1, 9, 0)


def test_expand_rrule_max_empty_periods_limit():
    import datetime as dt

    # Never matches; a tight empty-period budget must end expansion early
    # instead of scanning the full default year range.
    out = calcard.expand_rrule(
        "FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30",
        dt.datetime(2026, 1, 1, 9, 0),
        max_empty_periods=5,
    )
    assert out == []
