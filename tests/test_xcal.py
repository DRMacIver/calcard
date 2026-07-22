"""Tests for xCal/xCard conversion through the Python API."""

from hypothesis import given, strategies as st

import calcard

CAL = (
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\n"
    "SUMMARY:Tea <& biscuits>\r\nDTSTART;TZID=Europe/London:20260722T160000\r\n"
    "RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
)

CARD = (
    "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Example\r\n"
    "N:Example;Alice;;;\r\nBDAY:--0203\r\nEND:VCARD\r\n"
)


def test_xcal_round_trip():
    doc = calcard.parse(CAL)
    xml = calcard.to_xcal(doc)
    assert xml.startswith('<?xml version="1.0" encoding="utf-8"?><icalendar')
    assert "<summary><text>Tea &lt;&amp; biscuits&gt;</text></summary>" in xml
    back = calcard.from_xcal(xml)
    assert back.components[0] == doc.components[0]
    assert back.serialize() == CAL


def test_xcard_round_trip():
    doc = calcard.parse(CARD)
    xml = calcard.to_xcal(doc)
    assert "<vcards" in xml
    assert "<bday><date-and-or-time>--02-03</date-and-or-time></bday>" in xml
    back = calcard.from_xcal(xml)
    assert back.serialize() == CARD


def test_from_xcal_rejects_garbage():
    import pytest

    with pytest.raises(calcard.ParseError):
        calcard.from_xcal("<not-a-calendar/>")


@given(st.text(max_size=200))
def test_from_xcal_is_total(text):
    try:
        calcard.from_xcal(text)
    except calcard.ParseError:
        pass  # errors are fine; crashes are not
