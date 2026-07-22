"""VTIMEZONE-based TZID resolution (issue #1).

Precedence: a TZID is resolved through the host's zoneinfo when it names
a zone the host knows (real-world in-document VTIMEZONE copies are often
stale); otherwise through the document's own VTIMEZONE component; only
when both fail does interpretation fall back to a naive datetime, with a
TimezoneResolutionWarning.
"""

import datetime as dt
import warnings
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest
from hypothesis import given, settings, strategies as st

import calcard
from calcard import TimezoneResolutionWarning
from calcard.timezones import tzinfo_from_vtimezone

FIXTURE_ZONES = (
    Path(__file__).parent.parent / "conformance" / "fixtures" / "libical" / "timezones"
)

# A custom (non-Olson) TZID with US-Eastern-style rules.
FAKE_EASTERN = (
    "BEGIN:VTIMEZONE\r\n"
    "TZID:Corp/Head-Office\r\n"
    "BEGIN:STANDARD\r\n"
    "DTSTART:19701101T020000\r\n"
    "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\n"
    "TZOFFSETFROM:-0400\r\n"
    "TZOFFSETTO:-0500\r\n"
    "TZNAME:EST\r\n"
    "END:STANDARD\r\n"
    "BEGIN:DAYLIGHT\r\n"
    "DTSTART:19700308T020000\r\n"
    "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r\n"
    "TZOFFSETFROM:-0500\r\n"
    "TZOFFSETTO:-0400\r\n"
    "TZNAME:EDT\r\n"
    "END:DAYLIGHT\r\n"
    "END:VTIMEZONE\r\n"
)


def calendar_with(vtimezone: str, *properties: str) -> str:
    return (
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"
        + vtimezone
        + "BEGIN:VEVENT\r\n"
        + "".join(p + "\r\n" for p in properties)
        + "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )


def test_custom_tzid_resolves_through_vtimezone():
    doc = calcard.parse(
        calendar_with(
            FAKE_EASTERN,
            "DTSTART;TZID=Corp/Head-Office:20260115T090000",  # winter
            "DTEND;TZID=Corp/Head-Office:20260715T090000",  # summer
        ),
        strict=True,
    )
    event = doc.calendars[0].events[0]
    start, end = event.start, event.end
    assert start is not None and start.tzinfo is not None
    assert start.utcoffset() == dt.timedelta(hours=-5)
    assert start.tzname() == "EST"
    assert end.utcoffset() == dt.timedelta(hours=-4)
    assert end.tzname() == "EDT"
    # The instant is right, not just the offset label.
    assert start.astimezone(dt.timezone.utc) == dt.datetime(
        2026, 1, 15, 14, 0, tzinfo=dt.timezone.utc
    )


def test_vtimezone_zone_serializes_with_its_tzid():
    doc = calcard.parse(
        calendar_with(FAKE_EASTERN, "DTSTART;TZID=Corp/Head-Office:20260115T090000"),
        strict=True,
    )
    event = doc.calendars[0].events[0]
    event.start = event.start + dt.timedelta(days=1)
    assert "DTSTART;TZID=Corp/Head-Office:20260116T090000" in doc.serialize()


def test_zoneinfo_preferred_over_wrong_vtimezone():
    # A document carrying a deliberately wrong VTIMEZONE for a name the
    # host database knows: the database wins.
    wrong = (
        "BEGIN:VTIMEZONE\r\n"
        "TZID:Europe/London\r\n"
        "BEGIN:STANDARD\r\n"
        "DTSTART:19700101T000000\r\n"
        "TZOFFSETFROM:+0500\r\n"
        "TZOFFSETTO:+0500\r\n"
        "TZNAME:WRONG\r\n"
        "END:STANDARD\r\n"
        "END:VTIMEZONE\r\n"
    )
    doc = calcard.parse(
        calendar_with(wrong, "DTSTART;TZID=Europe/London:20260115T090000"),
        strict=True,
    )
    start = doc.calendars[0].events[0].start
    assert start.utcoffset() == dt.timedelta(0)  # GMT, not +05:00


def test_unresolvable_tzid_warns_and_falls_back_to_naive():
    doc = calcard.parse(
        calendar_with("", "DTSTART;TZID=Not/A-Zone:20260115T090000"),
        strict=True,
    )
    with pytest.warns(TimezoneResolutionWarning, match="Not/A-Zone"):
        start = doc.calendars[0].events[0].start
    assert start == dt.datetime(2026, 1, 15, 9, 0)
    assert start.tzinfo is None


def test_resolved_tzids_do_not_warn():
    doc = calcard.parse(
        calendar_with(FAKE_EASTERN, "DTSTART;TZID=Corp/Head-Office:20260115T090000"),
        strict=True,
    )
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        assert doc.calendars[0].events[0].start is not None


def test_occurrences_follow_vtimezone_dst():
    # Weekly 09:00 across the March 2026 US spring-forward: wall time is
    # constant, the UTC offset changes.
    doc = calcard.parse(
        calendar_with(
            FAKE_EASTERN,
            "DTSTART;TZID=Corp/Head-Office:20260305T090000",
            "RRULE:FREQ=WEEKLY;COUNT=2",
        ),
        strict=True,
    )
    first, second = doc.calendars[0].events[0].occurrences()
    assert (first.hour, second.hour) == (9, 9)
    assert first.utcoffset() == dt.timedelta(hours=-5)
    assert second.utcoffset() == dt.timedelta(hours=-4)


def test_fold_semantics_match_zoneinfo_style():
    # 2026-11-01 01:30 Corp/Head-Office is ambiguous (fall back at 02:00
    # EDT -> 01:00 EST): fold selects the offset.
    doc = calcard.parse(calendar_with(FAKE_EASTERN), strict=True)
    tz = tzinfo_from_vtimezone(doc.components[0].comp("VTIMEZONE"))
    ambiguous = dt.datetime(2026, 11, 1, 1, 30, tzinfo=tz)
    assert ambiguous.utcoffset() == dt.timedelta(hours=-4)  # fold=0: first
    assert ambiguous.replace(fold=1).utcoffset() == dt.timedelta(hours=-5)
    # 2026-03-08 02:30 does not exist (spring forward): fold=0 maps with
    # the pre-gap offset, fold=1 with the post-gap offset.
    gap = dt.datetime(2026, 3, 8, 2, 30, tzinfo=tz)
    assert gap.utcoffset() == dt.timedelta(hours=-5)
    assert gap.replace(fold=1).utcoffset() == dt.timedelta(hours=-4)


# ---------------------------------------------------------------------------
# Oracle: libical's tzdata-generated VTIMEZONE fixtures, interpreted by us,
# must agree with the host zoneinfo for the same zone.


def _fixture_zones():
    out = []
    for path in sorted(FIXTURE_ZONES.glob("*.ics")):
        doc = calcard.parse(path.read_text())
        vtz = doc.components[0].comp("VTIMEZONE")
        location = vtz.prop("X-LIC-LOCATION").value
        out.append((path.name, vtz, location))
    return out


ZONES = _fixture_zones()
assert len(ZONES) == 8


@pytest.mark.parametrize("name,vtz,location", ZONES, ids=[z[0] for z in ZONES])
@given(
    when=st.datetimes(
        min_value=dt.datetime(1993, 1, 1),
        max_value=dt.datetime(2024, 12, 31),
    ),
    fold=st.integers(0, 1),
)
# The first example against each zone pays the one-off cost of building
# the full transition table, which can exceed the default deadline.
@settings(deadline=None)
def test_vtimezone_interpretation_matches_zoneinfo(name, vtz, location, when, fold):
    ours = tzinfo_from_vtimezone(vtz)
    theirs = ZoneInfo(location)
    w = when.replace(fold=fold)
    assert w.replace(tzinfo=ours).utcoffset() == w.replace(tzinfo=theirs).utcoffset(), (
        f"{name} at {w} fold={fold}"
    )


@pytest.mark.parametrize("name,vtz,location", ZONES, ids=[z[0] for z in ZONES])
@given(
    when=st.datetimes(
        min_value=dt.datetime(1993, 1, 1),
        max_value=dt.datetime(2024, 12, 31),
        timezones=st.just(dt.timezone.utc),
    )
)
@settings(deadline=None)
def test_vtimezone_fromutc_matches_zoneinfo(name, vtz, location, when):
    ours = when.astimezone(tzinfo_from_vtimezone(vtz))
    theirs = when.astimezone(ZoneInfo(location))
    assert ours.replace(tzinfo=None) == theirs.replace(tzinfo=None), (
        f"{name} from {when}"
    )
