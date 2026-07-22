"""Cross-implementation compatibility tests: calcard vs PyPI ``vobject``
(py-vobject).

One direction Hypothesis-generates event models, serializes with calcard,
parses with ``vobject.readOne`` (which applies py-vobject's behaviours,
decoding text and datetimes), and asserts the decoded values match. The
other direction builds the same models through py-vobject's attribute API,
serializes with ``.serialize()``, and parses with calcard, asserting zero
repairs and matching native values.

Scope: the generated subset sticks to what py-vobject handles reliably.
Datetimes are date / naive / UTC only — py-vobject resolves TZID parameters
through its timezone registry and dateutil and generally needs a VTIMEZONE
component for foreign TZIDs, so TZID-parameterized datetimes are not
generated for this pair (the icalendar compat suite covers them). Nothing
QUOTED-PRINTABLE is generated either.

Known deviations:

* py-vobject cannot serialize ``datetime.timezone.utc`` values at all
  (``VObjectError: Unable to guess TZID for tzinfo UTC`` — its UTC
  detection recognizes dateutil/pytz UTC objects, and its tzname fallback
  needs ``.dst()`` to return a zero timedelta where ``datetime.timezone``
  returns ``None``). The theirs->ours direction therefore builds UTC
  datetimes with ``dateutil.tz.tzutc()``, which py-vobject serializes to
  the RFC ``Z`` form; comparisons are done on instants so the two UTC
  tzinfo implementations compare equal.
"""

import datetime as dt

import vobject
from dateutil import tz
from hypothesis import given, settings, strategies as st

import calcard
from calcard import Component, Property

# ---------------------------------------------------------------------------
# Strategies

ical_text = st.text(
    alphabet=st.one_of(
        st.characters(blacklist_categories=("Cs", "Cc")),
        st.sampled_from("\n\t"),
    ),
    max_size=60,
)

naive_datetimes = st.datetimes(
    min_value=dt.datetime(1990, 1, 1),
    max_value=dt.datetime(2035, 12, 31, 23, 59, 59),
).map(lambda d: d.replace(microsecond=0))


@st.composite
def start_ends(draw):
    """(kind, start, end) sharing one value type; no TZID kinds here (see
    module docstring)."""
    kind = draw(st.sampled_from(["date", "naive", "utc"]))
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
        "categories": st.none()
        | st.lists(ical_text.filter(lambda c: c != ""), min_size=1, max_size=4),
        "sequence": st.none() | st.integers(0, 10_000),
        "start_end": st.none() | start_ends(),
    }
)


# ---------------------------------------------------------------------------
# Helpers

def calcard_wire(model) -> str:
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


def pyvobject_wire(model) -> str:
    cal = vobject.iCalendar()
    ev = cal.add("vevent")
    ev.add("uid").value = model["uid"]
    if model["summary"] is not None:
        ev.add("summary").value = model["summary"]
    if model["description"] is not None:
        ev.add("description").value = model["description"]
    if model["categories"] is not None:
        ev.add("categories").value = model["categories"]
    if model["sequence"] is not None:
        ev.add("sequence").value = str(model["sequence"])
    if model["start_end"] is not None:
        _kind, start, end = model["start_end"]
        if isinstance(start, dt.datetime) and start.tzinfo is not None:
            # py-vobject cannot serialize datetime.timezone.utc; see the
            # Known deviations section of the module docstring.
            start = start.replace(tzinfo=tz.tzutc())
            end = end.replace(tzinfo=tz.tzutc())
        ev.add("dtstart").value = start
        ev.add("dtend").value = end
    return cal.serialize()


def assert_same_point(kind, actual, expected):
    if kind == "date":
        assert type(actual) is dt.date
        assert actual == expected
        return
    assert type(actual) is dt.datetime
    if kind == "naive":
        assert actual.tzinfo is None
        assert actual == expected
    else:
        assert actual.utcoffset() == dt.timedelta(0)
        assert actual == expected  # instant comparison across UTC tzinfos


# ---------------------------------------------------------------------------
# calcard -> py-vobject

@given(event_models)
@settings(deadline=None)
def test_calcard_output_read_by_pyvobject(model):
    wire = calcard_wire(model)
    cal = vobject.readOne(wire)
    events = cal.contents.get("vevent", [])
    assert len(events) == 1
    ev = events[0]

    assert ev.uid.value == model["uid"]
    for name in ("summary", "description"):
        if model[name] is None:
            assert name not in ev.contents
        else:
            assert ev.contents[name][0].value == model[name]
    if model["categories"] is None:
        assert "categories" not in ev.contents
    else:
        assert list(ev.categories.value) == model["categories"]
    if model["sequence"] is None:
        assert "sequence" not in ev.contents
    else:
        assert int(ev.sequence.value) == model["sequence"]
    if model["start_end"] is not None:
        kind, start, end = model["start_end"]
        assert_same_point(kind, ev.dtstart.value, start)
        assert_same_point(kind, ev.dtend.value, end)


# ---------------------------------------------------------------------------
# py-vobject -> calcard

@given(event_models)
@settings(deadline=None)
def test_pyvobject_output_read_by_calcard(model):
    wire = pyvobject_wire(model)
    doc = calcard.parse(wire)
    assert doc.repairs == [], f"py-vobject emitted non-conformant output: {wire!r}"
    assert len(doc.calendars) == 1
    events = doc.calendars[0].events
    assert len(events) == 1
    ev = events[0]

    assert ev.uid == model["uid"]
    assert ev.summary == model["summary"]
    assert ev.description == model["description"]
    cats_prop = ev.component.prop("CATEGORIES")
    if model["categories"] is None:
        assert cats_prop is None
    else:
        assert calcard.native_value(cats_prop) == model["categories"]
    if model["sequence"] is None:
        assert ev.component.prop("SEQUENCE") is None
    else:
        assert ev.text("SEQUENCE") == model["sequence"]
    if model["start_end"] is not None:
        kind, start, end = model["start_end"]
        assert_same_point(kind, ev.start, start)
        assert_same_point(kind, ev.end, end)


# ---------------------------------------------------------------------------
# Text round-trip through SUMMARY, both directions

@given(ical_text)
@settings(deadline=None)
def test_summary_text_calcard_to_pyvobject(text):
    cal = Component("VCALENDAR")
    cal.children = [Property("VERSION", "2.0")]
    ev = Component("VEVENT")
    cal.children = cal.children + [ev]
    view = calcard.wrap(ev)
    view.uid = "text-round-trip"
    view.summary = text
    parsed = vobject.readOne(calcard.serialize([cal]))
    assert parsed.vevent.summary.value == text


@given(ical_text)
@settings(deadline=None)
def test_summary_text_pyvobject_to_calcard(text):
    cal = vobject.iCalendar()
    ev = cal.add("vevent")
    ev.add("uid").value = "text-round-trip"
    ev.add("summary").value = text
    doc = calcard.parse(cal.serialize())
    assert doc.repairs == []
    assert doc.calendars[0].events[0].summary == text
