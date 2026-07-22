"""Cross-implementation compatibility tests: calcard vs the ``icalendar``
package.

One direction Hypothesis-generates event models, serializes them with
calcard, parses with ``icalendar.Calendar.from_ical``, and asserts the
decoded semantic values match. The other direction builds equivalent
calendars through icalendar's own API, serializes with ``to_ical()``, and
parses them with calcard — asserting zero repairs (their output is expected
to be conformant) and matching native values through calcard's typed views.

Known deviations:

* Text values beginning with U+FEFF: icalendar's ``to_ical`` silently
  drops a leading U+FEFF from a text value (it round-trips property text
  through the ``utf-8-sig`` codec, which treats it as a BOM), although its
  own ``from_ical`` preserves the character and RFC 5545 TEXT allows any
  non-control character. calcard preserves it in both directions. The text
  strategy therefore never starts a value with U+FEFF.
"""

import datetime as dt
from zoneinfo import ZoneInfo

import icalendar
from hypothesis import given, settings, strategies as st

import calcard
from calcard import Component, Property

# ---------------------------------------------------------------------------
# Strategies

# TEXT values: any non-surrogate character except controls, plus the two
# controls RFC 5545 TEXT can carry (HTAB literally, LF via the \n escape).
ical_text = st.text(
    alphabet=st.one_of(
        st.characters(blacklist_categories=("Cs", "Cc")),
        st.sampled_from("\n\t"),
    ),
    max_size=60,
).filter(lambda s: not s.startswith("﻿"))  # see Known deviations

# Zones whose TZIDs both libraries resolve through the system tzdata.
ZONES = [
    "America/New_York",
    "Europe/London",
    "Asia/Tokyo",
    "Australia/Sydney",
    "America/Sao_Paulo",
    "Asia/Kolkata",
]

naive_datetimes = st.datetimes(
    min_value=dt.datetime(1990, 1, 1),
    max_value=dt.datetime(2035, 12, 31, 23, 59, 59),
).map(lambda d: d.replace(microsecond=0))


@st.composite
def start_ends(draw):
    """(kind, start, end) with DTSTART/DTEND sharing one value type, as
    RFC 5545 3.8.2.2 requires (icalendar also enforces this)."""
    kind = draw(st.sampled_from(["date", "naive", "utc", "zoned"]))
    if kind == "date":
        start = draw(
            st.dates(min_value=dt.date(1990, 1, 1), max_value=dt.date(2035, 12, 31))
        )
        end = start + dt.timedelta(days=draw(st.integers(1, 60)))
        return kind, start, end
    start = draw(naive_datetimes)
    end = start + dt.timedelta(seconds=draw(st.integers(0, 10 * 86_400)))
    if kind == "utc":
        start = start.replace(tzinfo=dt.timezone.utc)
        end = end.replace(tzinfo=dt.timezone.utc)
    elif kind == "zoned":
        zone = ZoneInfo(draw(st.sampled_from(ZONES)))
        # Wall-clock times; some may be imaginary across a DST gap, which
        # both libraries carry through unchanged as wall time + zone.
        start = start.replace(tzinfo=zone)
        end = end.replace(tzinfo=zone)
    return kind, start, end


event_models = st.fixed_dictionaries(
    {
        "uid": st.text(
            alphabet=st.characters(
                codec="ascii", categories=("L", "N"), include_characters="-"
            ),
            min_size=1,
            max_size=30,
        ),
        "summary": st.none() | ical_text,
        "description": st.none() | ical_text,
        "location": st.none() | ical_text,
        "categories": st.none()
        | st.lists(ical_text.filter(lambda c: c != ""), min_size=1, max_size=4),
        "sequence": st.none() | st.integers(0, 10_000),
        "start_end": st.none() | start_ends(),
    }
)


# ---------------------------------------------------------------------------
# Helpers

def calcard_wire(model) -> str:
    """Serialize an event model with calcard."""
    cal = Component("VCALENDAR")
    cal.children = [
        Property("VERSION", "2.0"),
        Property("PRODID", "-//calcard compat tests//EN"),
    ]
    ev = Component("VEVENT")
    cal.children = cal.children + [ev]
    view = calcard.wrap(ev)
    view.uid = model["uid"]
    if model["summary"] is not None:
        view.summary = model["summary"]
    if model["description"] is not None:
        view.description = model["description"]
    if model["location"] is not None:
        view.location = model["location"]
    if model["categories"] is not None:
        ev.children = ev.children + [
            Property(
                "CATEGORIES",
                ",".join(calcard.escape_text(c) for c in model["categories"]),
            )
        ]
    if model["sequence"] is not None:
        ev.children = ev.children + [
            Property("SEQUENCE", str(model["sequence"]))
        ]
    if model["start_end"] is not None:
        _kind, start, end = model["start_end"]
        view.start = start
        view.end = end
    return calcard.serialize([cal])


def icalendar_wire(model) -> bytes:
    """Serialize the same event model with icalendar's own API."""
    cal = icalendar.Calendar()
    cal.add("version", "2.0")
    cal.add("prodid", "-//icalendar compat tests//EN")
    ev = icalendar.Event()
    ev.add("uid", model["uid"])
    if model["summary"] is not None:
        ev.add("summary", model["summary"])
    if model["description"] is not None:
        ev.add("description", model["description"])
    if model["location"] is not None:
        ev.add("location", model["location"])
    if model["categories"] is not None:
        ev.add("categories", model["categories"])
    if model["sequence"] is not None:
        ev.add("sequence", model["sequence"])
    if model["start_end"] is not None:
        _kind, start, end = model["start_end"]
        ev.add("dtstart", start)
        ev.add("dtend", end)
    cal.add_component(ev)
    return cal.to_ical()


def assert_same_point(kind, actual, expected):
    """The parsed date/datetime carries the same semantics as the model's.

    Datetimes are compared as wall time plus zone identity rather than by
    instant so that a decode into the wrong zone can never sneak through
    equal-instant comparison.
    """
    if kind == "date":
        assert type(actual) is dt.date
        assert actual == expected
        return
    assert type(actual) is dt.datetime
    assert actual.replace(tzinfo=None) == expected.replace(tzinfo=None)
    if kind == "naive":
        assert actual.tzinfo is None
    elif kind == "utc":
        assert actual.utcoffset() == dt.timedelta(0)
    else:
        assert getattr(actual.tzinfo, "key", None) == expected.tzinfo.key


# ---------------------------------------------------------------------------
# calcard -> icalendar

@given(event_models)
@settings(deadline=None)
def test_calcard_output_read_by_icalendar(model):
    wire = calcard_wire(model)
    cal = icalendar.Calendar.from_ical(wire)
    events = cal.walk("VEVENT")
    assert len(events) == 1
    ev = events[0]

    assert str(ev["UID"]) == model["uid"]
    for name in ("summary", "description", "location"):
        if model[name] is None:
            assert name.upper() not in ev
        else:
            assert str(ev[name.upper()]) == model[name]
    if model["categories"] is not None:
        assert [str(c) for c in ev["CATEGORIES"].cats] == model["categories"]
    else:
        assert "CATEGORIES" not in ev
    if model["sequence"] is not None:
        assert ev.decoded("SEQUENCE") == model["sequence"]
    if model["start_end"] is not None:
        kind, start, end = model["start_end"]
        assert_same_point(kind, ev.decoded("DTSTART"), start)
        assert_same_point(kind, ev.decoded("DTEND"), end)
        if kind == "date":
            assert ev["DTSTART"].params.get("VALUE") == "DATE"
        elif kind == "zoned":
            assert ev["DTSTART"].params.get("TZID") == start.tzinfo.key


# ---------------------------------------------------------------------------
# icalendar -> calcard

@given(event_models)
@settings(deadline=None)
def test_icalendar_output_read_by_calcard(model):
    wire = icalendar_wire(model)
    doc = calcard.parse(wire)
    assert doc.repairs == [], f"icalendar emitted non-conformant output: {wire!r}"
    assert len(doc.calendars) == 1
    events = doc.calendars[0].events
    assert len(events) == 1
    ev = events[0]

    assert ev.uid == model["uid"]
    assert ev.summary == model["summary"]
    assert ev.description == model["description"]
    assert ev.location == model["location"]
    cats_prop = ev.component.prop("CATEGORIES")
    if model["categories"] is None:
        assert cats_prop is None
    else:
        assert calcard.native_value(cats_prop) == model["categories"]
    if model["sequence"] is None:
        assert ev.component.prop("SEQUENCE") is None
    else:
        assert ev.text("SEQUENCE") == model["sequence"]
    if model["start_end"] is None:
        assert ev.component.prop("DTSTART") is None
    else:
        kind, start, end = model["start_end"]
        assert_same_point(kind, ev.start, start)
        assert_same_point(kind, ev.end, end)


def test_winter_zoned_datetime_keeps_its_zone_across_libraries():
    """Regression: a Europe/London datetime during winter (offset UTC+0)
    must serialize with TZID=Europe/London, not silently degrade to the
    ``Z`` form — the instant survives either way but the zone identity
    (and with it correct DST behaviour for anything recurring) does not.
    """
    cal = Component("VCALENDAR")
    cal.children = [Property("VERSION", "2.0")]
    ev = Component("VEVENT")
    cal.children = cal.children + [ev]
    view = calcard.wrap(ev)
    view.uid = "winter-zone"
    view.start = dt.datetime(2000, 1, 1, 0, 0, tzinfo=ZoneInfo("Europe/London"))
    wire = calcard.serialize([cal])
    assert "DTSTART;TZID=Europe/London:20000101T000000" in wire
    parsed = icalendar.Calendar.from_ical(wire)
    decoded = parsed.walk("VEVENT")[0].decoded("DTSTART")
    assert decoded.tzinfo.key == "Europe/London"


def test_utc_zoneinfo_still_serializes_as_z_form():
    """A named zone that IS UTC keeps the conventional ``Z`` form."""
    ev = Component("VEVENT")
    view = calcard.wrap(ev)
    view.start = dt.datetime(2000, 1, 1, 0, 0, tzinfo=ZoneInfo("UTC"))
    assert ev.prop("DTSTART").value == "20000101T000000Z"
    assert ev.prop("DTSTART").params == []


# ---------------------------------------------------------------------------
# Text round-trip through SUMMARY, both directions

@given(ical_text)
@settings(deadline=None)
def test_summary_text_calcard_to_icalendar(text):
    cal = Component("VCALENDAR")
    cal.children = [Property("VERSION", "2.0")]
    ev = Component("VEVENT")
    cal.children = cal.children + [ev]
    view = calcard.wrap(ev)
    view.uid = "text-round-trip"
    view.summary = text
    parsed = icalendar.Calendar.from_ical(calcard.serialize([cal]))
    assert str(parsed.walk("VEVENT")[0]["SUMMARY"]) == text


@given(ical_text)
@settings(deadline=None)
def test_summary_text_icalendar_to_calcard(text):
    cal = icalendar.Calendar()
    cal.add("version", "2.0")
    ev = icalendar.Event()
    ev.add("uid", "text-round-trip")
    ev.add("summary", text)
    cal.add_component(ev)
    doc = calcard.parse(cal.to_ical())
    assert doc.repairs == []
    assert doc.calendars[0].events[0].summary == text
