"""Tests for native value conversion, recurrence expansion, and jCal."""

import datetime as dt
import json
from pathlib import Path
from zoneinfo import ZoneInfo

from hypothesis import given, strategies as st

import calcard
from calcard import native_value


def prop_of(line: str, dialect: str = "icalendar"):
    wrapper = "VCALENDAR" if dialect == "icalendar" else "VCARD"
    doc = calcard.parse(f"BEGIN:{wrapper}\r\n{line}\r\nEND:{wrapper}\r\n")
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


def test_time_values():
    assert native_value(prop_of("X-T;VALUE=TIME:123045")) == [
        dt.time(12, 30, 45)
    ]
    assert native_value(prop_of("X-T;VALUE=TIME:123045Z")) == [
        dt.time(12, 30, 45, tzinfo=dt.timezone.utc)
    ]


def test_period_start_end():
    got = native_value(prop_of("FREEBUSY:19970308T160000Z/19970308T190000Z"))
    assert got == [
        (
            dt.datetime(1997, 3, 8, 16, 0, tzinfo=dt.timezone.utc),
            dt.datetime(1997, 3, 8, 19, 0, tzinfo=dt.timezone.utc),
        )
    ]


def test_period_start_duration():
    got = native_value(prop_of("FREEBUSY:19970308T160000Z/PT3H"))
    assert got == [
        (
            dt.datetime(1997, 3, 8, 16, 0, tzinfo=dt.timezone.utc),
            dt.timedelta(hours=3),
        )
    ]


def test_period_with_tzid():
    got = native_value(
        prop_of("FREEBUSY;TZID=Europe/London:19970308T160000/PT1H")
    )
    assert got == [
        (
            dt.datetime(1997, 3, 8, 16, 0, tzinfo=ZoneInfo("Europe/London")),
            dt.timedelta(hours=1),
        )
    ]


def test_boolean_values():
    assert native_value(prop_of("X-B;VALUE=BOOLEAN:TRUE")) is True
    assert native_value(prop_of("X-B;VALUE=BOOLEAN:FALSE")) is False


def test_unresolvable_tzid_warns_and_yields_naive_datetime():
    import pytest

    from calcard import TimezoneResolutionWarning

    with pytest.warns(TimezoneResolutionWarning, match="Not/AZone"):
        got = native_value(prop_of("DTSTART;TZID=Not/AZone:20260722T160000"))
    assert got == [dt.datetime(2026, 7, 22, 16, 0)]
    assert got[0].tzinfo is None


def test_date_shaped_value_in_datetime_position():
    # A DATE-TIME-typed property whose value is date-shaped comes back as
    # a date (the tuple has three fields; _to_datetime keeps it a date).
    assert native_value(prop_of("DTSTART:20260722")) == [dt.date(2026, 7, 22)]


def test_expand_rrule():
    got = calcard.expand_rrule(
        "FREQ=WEEKLY;COUNT=3", dt.datetime(2026, 7, 22, 9, 0, 0)
    )
    assert got == [
        dt.datetime(2026, 7, 22, 9, 0),
        dt.datetime(2026, 7, 29, 9, 0),
        dt.datetime(2026, 8, 5, 9, 0),
    ]
    got = calcard.expand_rrule("FREQ=YEARLY;COUNT=2", dt.date(2024, 2, 29))
    assert got == [dt.date(2024, 2, 29), dt.date(2028, 2, 29)]


def test_expand_rrule_infinite_respects_limit():
    got = calcard.expand_rrule("FREQ=DAILY", dt.date(2026, 1, 1), limit=5)
    assert len(got) == 5


def test_to_jcal_matches_fixture():
    fixtures = Path(__file__).parent.parent / "conformance/fixtures/icaljs/parser"
    comp = calcard.parse((fixtures / "boolean.ics").read_bytes()).components[0]
    expected = json.loads((fixtures / "boolean.json").read_text())
    assert calcard.to_jcal(comp) == expected


@given(st.integers(min_value=1, max_value=30), st.integers(min_value=1, max_value=6))
def test_expand_rrule_daily_interval_property(count, interval):
    start = dt.datetime(2026, 1, 1, 12, 0, 0)
    got = calcard.expand_rrule(f"FREQ=DAILY;COUNT={count};INTERVAL={interval}", start)
    assert len(got) == count
    for i, d in enumerate(got):
        assert d == start + dt.timedelta(days=i * interval)
