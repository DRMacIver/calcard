"""Unit tests for the public Python API."""

import pytest

import vobject

SIMPLE = (
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Tea\r\n"
    "END:VEVENT\r\nEND:VCALENDAR\r\n"
)


def test_parse_and_navigate():
    doc = vobject.parse(SIMPLE)
    assert len(doc) == 1
    cal = doc.components[0]
    assert cal.name == "VCALENDAR"
    event = cal.comp("vevent")
    assert event.prop("summary").value == "Tea"
    assert doc.repairs == []


def test_document_is_iterable():
    doc = vobject.parse(SIMPLE)
    assert [c.name for c in doc] == ["VCALENDAR"]


def test_serialize_round_trip():
    doc = vobject.parse(SIMPLE)
    assert doc.serialize() == SIMPLE


def test_serialize_accepts_document_component_and_list():
    doc = vobject.parse(SIMPLE)
    comp = doc.components[0]
    assert vobject.serialize(doc) == SIMPLE
    assert vobject.serialize(comp) == SIMPLE
    assert vobject.serialize([comp]) == SIMPLE
    assert vobject.serialize([comp, comp]) == SIMPLE + SIMPLE


def test_parse_one():
    comp = vobject.parse_one(SIMPLE)
    assert comp.name == "VCALENDAR"
    with pytest.raises(vobject.ParseError):
        vobject.parse_one(SIMPLE + SIMPLE)
    with pytest.raises(vobject.ParseError):
        vobject.parse_one("")


def test_strict_mode_raises_with_line_number():
    with pytest.raises(vobject.ParseError) as excinfo:
        vobject.parse("BEGIN:VCALENDAR\nEND:VCALENDAR\n", strict=True)
    assert excinfo.value.line == 1


def test_lenient_records_repairs():
    doc = vobject.parse("BEGIN:VCARD\nFN:Bob\n")
    assert doc.components[0].prop("FN").value == "Bob"
    lines = [r.line for r in doc.repairs]
    assert all(line >= 1 for line in lines)
    messages = " ".join(r.message for r in doc.repairs)
    assert "unterminated" in messages


def test_bytes_input_utf8_and_bom():
    assert vobject.parse(SIMPLE.encode()).serialize() == SIMPLE
    assert vobject.parse(b"\xef\xbb\xbf" + SIMPLE.encode()).serialize() == SIMPLE
    assert vobject.parse("﻿" + SIMPLE).serialize() == SIMPLE


def test_bytes_input_latin1_fallback():
    data = "BEGIN:VCARD\r\nFN:Rémi\r\nEND:VCARD\r\n".encode("latin-1")
    doc = vobject.parse(data)
    assert doc.components[0].prop("FN").value == "Rémi"


def test_building_a_document_from_scratch():
    event = vobject.Component(
        "VEVENT",
        [
            vobject.Property("SUMMARY", "Tea"),
            vobject.Property(
                "DTSTART",
                "20260722T160000",
                params=[vobject.Param("TZID", ["Europe/London"])],
            ),
        ],
    )
    cal = vobject.Component("VCALENDAR", [vobject.Property("VERSION", "2.0"), event])
    out = vobject.serialize(cal)
    assert "DTSTART;TZID=Europe/London:20260722T160000\r\n" in out
    reparsed = vobject.parse(out, strict=True)
    assert reparsed.components[0] == cal


def test_mutation_in_place():
    doc = vobject.parse(SIMPLE)
    doc.components[0].comp("VEVENT").prop("SUMMARY").value = "Coffee"
    assert "SUMMARY:Coffee\r\n" in doc.serialize()


def test_param_quoting_and_caret_encoding():
    prop = vobject.Property(
        "X-TEST", "v", params=[vobject.Param("A", ['needs "quotes", yes\nreally'])]
    )
    out = vobject.serialize(vobject.Component("VCARD", [prop]))
    reparsed = vobject.parse(out, strict=True).components[0]
    assert reparsed.prop("X-TEST").params[0].values == ['needs "quotes", yes\nreally']


def test_escape_helpers():
    assert vobject.escape_text("a,b;c\nd\\e") == "a\\,b\\;c\\nd\\\\e"
    assert vobject.unescape_text("a\\,b\\;c\\nd\\\\e") == "a,b;c\nd\\e"
    assert vobject.split_unescaped("a,b\\,c,d", ",") == ["a", "b\\,c", "d"]


def test_depth_limit_configurable():
    deep = "BEGIN:A\r\n" * 5 + "END:A\r\n" * 5
    with pytest.raises(vobject.ParseError):
        vobject.parse(deep, strict=True, max_depth=3)
    doc = vobject.parse(deep, max_depth=3)
    assert doc.repairs  # over-deep BEGINs dropped with repairs
