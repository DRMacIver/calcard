"""Tests for native value conversion, recurrence expansion, and jCal."""

import datetime as dt
import json
from pathlib import Path
from zoneinfo import ZoneInfo

from hypothesis import given, strategies as st

import vobject
from vobject import native_value


def prop_of(line: str, dialect: str = "icalendar"):
    wrapper = "VCALENDAR" if dialect == "icalendar" else "VCARD"
    doc = vobject.parse(f"BEGIN:{wrapper}\r\n{line}\r\nEND:{wrapper}\r\n")
    return doc.components[0].properties()[0]


def test_datetime_values():
    p = prop_of("DTSTART:20260722T160000Z")
    assert native_value(p) == [
        dt.datetime(2026, 7, 22, 16, 0, 0, tzinfo=dt.timezone.utc)
    ]
    p = prop_of("DTSTART;TZID=Europe/London:20260722T160000")
    assert native_value(p) == [
        dt.datetime(2026, 7, 22, 16, 0, 0, tzinfo=ZoneInfo("Europe/London"))
    ]
    p = prop_of("DTSTART:20260722T160000")
    assert native_value(p) == [dt.datetime(2026, 7, 22, 16, 0, 0)]
    p = prop_of("DTSTART;VALUE=DATE:20260722")
    assert native_value(p) == [dt.date(2026, 7, 22)]


def test_text_and_lists():
    assert native_value(prop_of("SUMMARY:a\\, b\\nc")) == "a, b\nc"
    assert native_value(prop_of("CATEGORIES:one,two")) == ["one", "two"]
    assert native_value(prop_of("CATEGORIES:solo")) == ["solo"]


def test_duration_and_offsets():
    assert native_value(prop_of("DURATION:-PT1H30M")) == [
        dt.timedelta(hours=-1, minutes=-30)
    ]
    tz = native_value(prop_of("TZOFFSETTO:-0500"))
    assert tz.utcoffset(None) == dt.timedelta(hours=-5)


def test_structured_and_numbers():
    assert native_value(prop_of("GEO:37.386013;-122.082932")) == [
        37.386013,
        -122.082932,
    ]
    assert native_value(prop_of("PRIORITY:5")) == 5
    assert native_value(
        prop_of("N:Public;John;Quinlan;Mr.;Esq.", "vcard4"), "vcard4"
    ) == [["Public"], ["John"], ["Quinlan"], ["Mr."], ["Esq."]]


def test_binary():
    assert native_value(prop_of("X-B;VALUE=BINARY:Zm9vYmFy")) == b"foobar"


def test_expand_rrule():
    got = vobject.expand_rrule(
        "FREQ=WEEKLY;COUNT=3", dt.datetime(2026, 7, 22, 9, 0, 0)
    )
    assert got == [
        dt.datetime(2026, 7, 22, 9, 0),
        dt.datetime(2026, 7, 29, 9, 0),
        dt.datetime(2026, 8, 5, 9, 0),
    ]
    got = vobject.expand_rrule("FREQ=YEARLY;COUNT=2", dt.date(2024, 2, 29))
    assert got == [dt.date(2024, 2, 29), dt.date(2028, 2, 29)]


def test_expand_rrule_infinite_respects_limit():
    got = vobject.expand_rrule("FREQ=DAILY", dt.date(2026, 1, 1), limit=5)
    assert len(got) == 5


def test_to_jcal_matches_fixture():
    fixtures = Path(__file__).parent.parent / "conformance/fixtures/icaljs/parser"
    comp = vobject.parse((fixtures / "boolean.ics").read_bytes()).components[0]
    expected = json.loads((fixtures / "boolean.json").read_text())
    assert vobject.to_jcal(comp) == expected


@given(st.integers(min_value=1, max_value=30), st.integers(min_value=1, max_value=6))
def test_expand_rrule_daily_interval_property(count, interval):
    start = dt.datetime(2026, 1, 1, 12, 0, 0)
    got = vobject.expand_rrule(f"FREQ=DAILY;COUNT={count};INTERVAL={interval}", start)
    assert len(got) == count
    for i, d in enumerate(got):
        assert d == start + dt.timedelta(days=i * interval)
