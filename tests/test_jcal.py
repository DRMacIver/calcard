"""Tests for jCal/jCard parsing (from_jcal) through the Python API."""

import json

import pytest
from hypothesis import given, settings, strategies as st

import vobject

from test_properties import any_input

CAL = (
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
    "SUMMARY:Tea \\, biscuits\r\nDTSTART;TZID=Europe/London:20260722T160000\r\n"
    "RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
)

CARD = (
    "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Example\r\n"
    "N:Example;Alice;;;\r\nBDAY:--0203\r\nEND:VCARD\r\n"
)


def test_jcal_round_trip():
    doc = vobject.parse(CAL)
    j = vobject.to_jcal(doc.components[0])
    assert j[0] == "vcalendar"
    back = vobject.from_jcal(j)
    assert back.components[0] == doc.components[0]
    assert back.serialize() == CAL


def test_from_jcal_accepts_json_text():
    doc = vobject.parse(CAL)
    j = vobject.to_jcal(doc.components[0])
    back = vobject.from_jcal(json.dumps(j))
    assert back.serialize() == CAL


def test_jcard_round_trip():
    doc = vobject.parse(CARD)
    j = vobject.to_jcal(doc.components[0])
    assert j[0] == "vcard"
    back = vobject.from_jcal(j)
    assert back.components[0] == doc.components[0]
    assert back.serialize() == CARD


def test_from_jcal_multiple_documents():
    docs = vobject.parse(CAL + CARD)
    js = [vobject.to_jcal(c) for c in docs.components]
    back = vobject.from_jcal(js)
    assert len(back.components) == 2
    assert back.serialize() == CAL + CARD


def test_from_jcal_value_param():
    back = vobject.from_jcal(["vcalendar", [["dtstart", {}, "date", "2026-07-22"]], []])
    prop = back.components[0].prop("DTSTART")
    assert prop.value == "20260722"


def test_from_jcal_rejects_garbage():
    for bad in ["not json", "{}", "42", "[]", '["vcalendar"]', '[7, [], []]']:
        with pytest.raises(vobject.ParseError):
            vobject.from_jcal(bad)


def test_from_jcal_rejects_bad_structures():
    with pytest.raises(vobject.ParseError):
        vobject.from_jcal({"not": "jcal"})
    with pytest.raises(vobject.ParseError):
        vobject.from_jcal(["vcalendar", [["summary", {}, "text"]], []])


def test_from_jcal_depth_bomb_errors_cleanly():
    bomb = "[" * 100_000 + "]" * 100_000
    with pytest.raises(vobject.ParseError):
        vobject.from_jcal(bomb)


@given(any_input)
@settings(max_examples=300)
def test_jcal_fixed_point_on_parsed_documents(text):
    doc = vobject.parse(text)
    if not doc.components:
        return
    js = [vobject.to_jcal(c) for c in doc.components]
    back = vobject.from_jcal(js)
    assert len(back.components) == len(doc.components)
    assert [vobject.to_jcal(c) for c in back.components] == js


@given(st.text(max_size=200))
def test_from_jcal_is_total_on_text(text):
    try:
        vobject.from_jcal(text)
    except vobject.ParseError:
        pass  # errors are fine; crashes are not


json_scalars = st.one_of(
    st.none(),
    st.booleans(),
    st.integers(min_value=-(10**9), max_value=10**9),
    st.floats(allow_nan=False, allow_infinity=False),
    st.text(max_size=20),
)
json_values = st.recursive(
    json_scalars,
    lambda children: st.one_of(
        st.lists(children, max_size=4),
        st.dictionaries(st.text(max_size=8), children, max_size=4),
    ),
    max_leaves=25,
)


@given(json_values)
@settings(max_examples=300)
def test_from_jcal_is_total_on_json_structures(value):
    try:
        vobject.from_jcal(value)
    except vobject.ParseError:
        pass  # errors are fine; crashes are not
