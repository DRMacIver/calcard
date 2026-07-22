"""Pretalx's ICS-export workflows, ported from py-vobject to calcard.

pretalx (https://github.com/pretalx/pretalx) generates iCalendar exports
with py-vobject in ``src/pretalx/schedule/domain/ical.py``: it builds a
VCALENDAR with a stable PRODID, appends one VEVENT per scheduled talk
(SUMMARY, DTSTAMP, LOCATION, UID, DTSTART/DTEND as zone-aware local
datetimes, DESCRIPTION, URL), and serializes the result for HTTP
responses (``text/calendar`` exports) and email attachments. It never
parses iCalendar data.

This module ports that workflow to calcard faithfully — including
pretalx's ``strip_control_characters`` sanitizer and the structure of its
own test suite (``src/tests/schedule/domain/test_ical.py``) — and then
tests every library capability the port depends on:

* building a calendar from scratch and serializing it,
* zone-aware datetimes keeping their IANA TZID (the bug class pretalx
  works around in vobject with ``patch_out_timezone_cache``: vobject
  rewrites ``Europe/Berlin`` to the ambiguous abbreviation ``CET``),
* VTIMEZONE generation for every referenced TZID (vobject emits these
  automatically at serialize time; calcard makes the step explicit via
  ``add_missing_timezones``),
* text escaping, folding, and unicode surviving round-trips through
  calcard itself and through the readers real calendar consumers use
  (py-vobject and icalendar are both dev dependencies here).
"""

from __future__ import annotations

import dataclasses
import datetime as dt
from zoneinfo import ZoneInfo

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import calcard
from calcard import Calendar, Component, Event, Property
from calcard.timezones import (
    TimezoneResolutionWarning,
    add_missing_timezones,
    tzinfo_from_vtimezone,
    vtimezone_from_tzinfo,
)

BERLIN = ZoneInfo("Europe/Berlin")
UTC = ZoneInfo("UTC")
NETLOC = "pretalx.example.com"


# --------------------------------------------------------------------------
# The port of pretalx/schedule/domain/ical.py.
#
# ``patch_out_timezone_cache`` has no equivalent: it patches a vobject bug
# (a process-global TZID cache keyed by abbreviation, so "PST" could mean
# Pacific or Philippine standard time). calcard keeps the IANA name from
# the datetime's tzinfo and holds no global state, so the workaround —
# and the bug — do not exist.
# --------------------------------------------------------------------------

# pretalx/common/text/xml.py: C0 controls except tab and newline, DEL,
# and C1 controls.
STRIP_CONTROL_CHARS = dict.fromkeys(
    list(range(9)) + list(range(11, 32)) + list(range(127, 160))
)


def strip_control_characters(text):
    return str(text or "").translate(STRIP_CONTROL_CHARS)


def get_calendar(netloc, prodid):
    """pretalx ``get_calendar``: an empty iCalendar tagged with
    ``-//pretalx//{netloc}//{prodid}``."""
    return Calendar(
        Component(
            "VCALENDAR",
            [
                Property("VERSION", "2.0"),
                Property("PRODID", f"-//pretalx//{netloc}//{prodid}"),
            ],
        )
    )


def build_slot_vevent(slot, calendar, *, creation_time=None, netloc=None):
    """pretalx ``build_slot_vevent``: append a VEVENT for *slot*, no-op if
    the slot is incomplete."""
    if not slot.start or not slot.local_end or not slot.room or not slot.submission:
        return
    creation_time = creation_time or dt.datetime.now(UTC)
    netloc = netloc or NETLOC

    component = Component("VEVENT")
    calendar.component.children = calendar.component.children + [component]
    vevent = Event(component)
    vevent.summary = strip_control_characters(
        f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    vevent.set_datetime("DTSTAMP", creation_time)
    vevent.location = strip_control_characters(slot.room)
    vevent.uid = (
        f"pretalx-{slot.event_slug}-{slot.submission.code}{slot.id_suffix}@{netloc}"
    )
    vevent.start = slot.local_start
    vevent.end = slot.local_end
    vevent.description = strip_control_characters(slot.submission.abstract)
    vevent.url = slot.submission.url


def get_slots_ical(event_slug, slots, prodid_suffix=None, netloc=NETLOC):
    """pretalx ``get_slots_ical``. The one structural addition over the
    vobject version: vobject invents VTIMEZONE components inside
    ``serialize()``; with calcard that step is the explicit
    ``add_missing_timezones`` call."""
    prodid = event_slug
    if prodid_suffix:
        prodid = f"{prodid}//{prodid_suffix}"
    cal = get_calendar(netloc, prodid)
    creation_time = dt.datetime.now(UTC)
    for slot in slots:
        build_slot_vevent(slot, cal, creation_time=creation_time, netloc=netloc)
    cal.add_missing_timezones()
    return cal


def get_speaker_ical(event_slug, speaker_code, slots):
    return get_slots_ical(event_slug, slots, prodid_suffix=f"speaker//{speaker_code}")


def get_submission_ical(event_slug, submission_code, slots):
    return get_slots_ical(event_slug, slots, prodid_suffix=f"talk//{submission_code}")


def get_slot_ical(slot):
    cal = get_calendar(NETLOC, slot.submission.code)
    build_slot_vevent(slot, cal)
    cal.add_missing_timezones()
    return cal


# -- Slot/submission stand-ins for pretalx's Django models ------------------


@dataclasses.dataclass
class Submission:
    title: str = "Continuous Deployment of Small Satellites"
    display_speaker_names: str = "Ada Lovelace, Grace Hopper"
    abstract: str = (
        "A talk in two parts.\n\nWith a blank line, ümlauts, and 100% emoji: \N{ROCKET}"
    )
    code: str = "ABC123"
    url: str = "https://pretalx.example.com/democon/talk/ABC123/"


@dataclasses.dataclass
class Slot:
    start: dt.datetime | None = dt.datetime(2026, 7, 22, 10, 0, tzinfo=UTC)
    end: dt.datetime | None = dt.datetime(2026, 7, 22, 10, 45, tzinfo=UTC)
    room: str | None = "Room 1; the big, nice one"
    submission: Submission | None = dataclasses.field(default_factory=Submission)
    event_slug: str = "democon"
    tz: ZoneInfo = BERLIN
    id_suffix: str = ""

    @property
    def local_start(self):
        if self.start:
            return self.start.astimezone(self.tz)

    @property
    def local_end(self):
        if self.end:
            return self.end.astimezone(self.tz)


def roundtrip(cal: Calendar) -> Calendar:
    """Serialize under the strict grammar and parse back."""
    doc = calcard.parse(cal.serialize(), strict=True)
    assert doc.repairs == []
    (parsed,) = doc.calendars
    return parsed


# --------------------------------------------------------------------------
# Ports of pretalx's own test assertions (test_ical.py).
# --------------------------------------------------------------------------


def test_build_slot_vevent_appends_vevent():
    slot = Slot()
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(slot, cal)

    (vevent,) = cal.events
    assert (
        vevent.summary
        == f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    assert vevent.location == slot.room
    assert vevent.start == slot.local_start
    assert vevent.end == slot.local_end
    assert vevent.description == slot.submission.abstract
    assert vevent.url == slot.submission.url
    assert vevent.uid == f"pretalx-democon-{slot.submission.code}@{NETLOC}"


@pytest.mark.parametrize("missing", ["start", "end", "room", "submission"])
def test_build_slot_vevent_does_not_mutate_calendar_when_incomplete(missing):
    slot = Slot(**{missing: None})
    cal = get_calendar(NETLOC, "democon")

    build_slot_vevent(slot, cal)

    assert cal.events == []
    assert cal.component.comps("VEVENT") == []


def test_get_slots_ical_with_slot():
    cal = get_slots_ical("democon", [Slot()])

    result = cal.serialize()
    assert "BEGIN:VCALENDAR" in result
    assert "BEGIN:VEVENT" in result
    assert "democon" in cal.prodid


def test_get_slots_ical_prodid_with_suffix():
    cal = get_slots_ical("democon", [Slot()], prodid_suffix="faved")
    assert cal.prodid.endswith("//faved")


def test_get_slots_ical_empty_slots():
    cal = get_slots_ical("democon", [])
    result = cal.serialize()
    assert "BEGIN:VCALENDAR" in result
    assert "BEGIN:VEVENT" not in result


def test_get_slot_ical():
    slot = Slot()
    cal = get_slot_ical(slot)
    result = cal.serialize()
    assert "BEGIN:VCALENDAR" in result
    assert "BEGIN:VEVENT" in result
    assert slot.submission.code in cal.prodid


def test_get_speaker_ical():
    cal = get_speaker_ical("democon", "SPKR1", [Slot()])
    assert "speaker//SPKR1" in cal.prodid


def test_get_submission_ical():
    cal = get_submission_ical("democon", "ABC123", [Slot()])
    assert "talk//ABC123" in cal.prodid


def test_build_slot_vevent_strips_control_characters():
    slot = Slot(
        submission=Submission(
            title="Talk\x1btitle",  # ESC
            abstract="Abstract\x9bwith control",  # 8-bit CSI
        )
    )
    cal = get_slot_ical(slot)

    serialized = cal.serialize()
    assert "\x1b" not in serialized
    assert "\x9b" not in serialized
    (vevent,) = cal.events
    assert "Talktitle" in vevent.summary
    assert "Abstractwith control" in vevent.description


def test_unsanitized_control_characters_are_kept_and_flagged():
    """The sanitizer stays pretalx's job: calcard is lossless, so a raw
    ESC survives serialization, fails the strict grammar, and is kept
    with a Repair in lenient mode — never silently altered."""
    cal = get_calendar(NETLOC, "democon")
    ev = Component("VEVENT")
    cal.component.children = cal.component.children + [ev]
    Event(ev).summary = "bad \x1b esc"

    out = cal.serialize()
    assert "\x1b" in out
    with pytest.raises(calcard.ParseError):
        calcard.parse(out, strict=True)
    doc = calcard.parse(out)
    assert any("control" in repair.message for repair in doc.repairs)
    assert doc.calendars[0].events[0].summary == "bad \x1b esc"


# --------------------------------------------------------------------------
# Wire-format requirements of the export.
# --------------------------------------------------------------------------


def test_serialized_wire_structure():
    creation = dt.datetime(2026, 7, 1, 12, 0, tzinfo=UTC)
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(Slot(), cal, creation_time=creation)
    cal.add_missing_timezones()
    out = cal.serialize()

    assert out.startswith("BEGIN:VCALENDAR\r\n")
    assert out.endswith("END:VCALENDAR\r\n")
    lines = out.split("\r\n")
    assert "VERSION:2.0" in lines
    assert "PRODID:-//pretalx//pretalx.example.com//democon" in lines
    # Local times keep their IANA zone; DTSTAMP is the UTC Z-form.
    assert "DTSTART;TZID=Europe/Berlin:20260722T120000" in lines
    assert "DTEND;TZID=Europe/Berlin:20260722T124500" in lines
    assert "DTSTAMP:20260701T120000Z" in lines
    assert f"UID:pretalx-democon-ABC123@{NETLOC}" in lines
    # No line may exceed the RFC 5545 75-octet fold width.
    for line in lines:
        assert len(line.encode()) <= 75


def test_no_ambiguous_abbreviation_tzids():
    """The vobject bug pretalx patches around: with a shared abbreviation
    cache, "PST" may mean Pacific (-08:00) or Philippine (+08:00)
    standard time. calcard must keep both zones' IANA names apart in one
    calendar and never emit a bare abbreviation TZID."""
    la = Slot(
        start=dt.datetime(2026, 1, 15, 18, 0, tzinfo=UTC),
        end=dt.datetime(2026, 1, 15, 19, 0, tzinfo=UTC),
        tz=ZoneInfo("America/Los_Angeles"),
        submission=Submission(code="LATALK"),
    )
    manila = Slot(
        start=dt.datetime(2026, 1, 16, 2, 0, tzinfo=UTC),
        end=dt.datetime(2026, 1, 16, 3, 0, tzinfo=UTC),
        tz=ZoneInfo("Asia/Manila"),
        submission=Submission(code="PHTALK"),
    )
    cal = get_slots_ical("democon", [la, manila])
    out = cal.serialize()

    assert "TZID=America/Los_Angeles" in out
    assert "TZID=Asia/Manila" in out
    assert "TZID=PST" not in out
    assert "TZID:PST" not in out

    first, second = roundtrip(cal).events
    assert first.start == la.local_start
    assert second.start == manila.local_start
    # The two 10:00-local times are 16 hours apart as instants.
    assert first.start.utcoffset() == dt.timedelta(hours=-8)
    assert second.start.utcoffset() == dt.timedelta(hours=8)


def test_zero_offset_winter_zone_keeps_identity():
    """A London winter time has UTC offset zero but is not UTC; the TZID
    must survive rather than collapsing to the Z-form."""
    slot = Slot(
        start=dt.datetime(2026, 1, 15, 10, 0, tzinfo=UTC),
        end=dt.datetime(2026, 1, 15, 11, 0, tzinfo=UTC),
        tz=ZoneInfo("Europe/London"),
    )
    cal = get_slots_ical("democon", [slot])
    assert "DTSTART;TZID=Europe/London:20260115T100000" in cal.serialize()
    (vevent,) = roundtrip(cal).events
    assert vevent.start == slot.local_start
    assert vevent.start.tzinfo == ZoneInfo("Europe/London")


def test_full_semantic_round_trip():
    creation = dt.datetime(2026, 7, 1, 12, 0, tzinfo=UTC)
    slot = Slot()
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(slot, cal, creation_time=creation)
    cal.add_missing_timezones()

    (vevent,) = roundtrip(cal).events
    assert (
        vevent.summary
        == f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    assert vevent.description == slot.submission.abstract
    assert vevent.location == slot.room
    assert vevent.url == slot.submission.url
    assert vevent.uid == f"pretalx-democon-ABC123@{NETLOC}"
    assert vevent.start == slot.local_start
    assert vevent.end == slot.local_end
    assert vevent.text("DTSTAMP") == [creation]


def test_multiple_slots_share_one_calendar_and_timezone():
    slots = [
        Slot(
            start=dt.datetime(2026, 7, 22 + i, 10, 0, tzinfo=UTC),
            end=dt.datetime(2026, 7, 22 + i, 11, 0, tzinfo=UTC),
            submission=Submission(code=f"TALK{i}"),
        )
        for i in range(3)
    ]
    cal = get_slots_ical("democon", slots)

    assert [e.uid for e in cal.events] == [
        f"pretalx-democon-TALK{i}@{NETLOC}" for i in range(3)
    ]
    # One VTIMEZONE covers all three events.
    assert [t.tzid for t in cal.timezones] == ["Europe/Berlin"]
    parsed = roundtrip(cal)
    assert [e.uid for e in parsed.events] == [e.uid for e in cal.events]


def test_export_as_bytes_attachment():
    """The email-attachment path: content is encoded UTF-8 and must parse
    back byte-for-byte (strict; parse_bytes)."""
    cal = get_slot_ical(Slot())
    payload = cal.serialize().encode("utf-8")
    doc = calcard.parse(payload, strict=True)
    assert doc.repairs == []
    assert doc.serialize().encode("utf-8") == payload


def test_url_with_reserved_characters_survives_unescaped():
    """URL is a URI-valued property: commas and semicolons in it must not
    be TEXT-escaped, or consumers double-unescape them."""
    slot = Slot(submission=Submission(url="https://pretalx.example.com/t/?a=1,2;b=3"))
    cal = get_slot_ical(slot)
    assert "URL:https://pretalx.example.com/t/?a=1,2;b=3" in cal.serialize()
    (vevent,) = roundtrip(cal).events
    assert vevent.url == slot.submission.url


def test_url_setter_rejects_line_breaks():
    ev = Event(Component("VEVENT"))
    with pytest.raises(ValueError):
        ev.url = "https://x.example/\r\nX-INJECTED:1"


def test_long_summary_folds_and_unfolds():
    slot = Slot(
        submission=Submission(
            title="A very long talk title that has to be folded "
            + "beyond seventy-five octets " * 5,
            abstract="Ümläüts and emoji \N{ROCKET} force octet-aware folding " * 10,
        )
    )
    cal = get_slot_ical(slot)
    out = cal.serialize()
    for line in out.split("\r\n"):
        assert len(line.encode()) <= 75
    (vevent,) = roundtrip(cal).events
    assert vevent.summary.startswith(slot.submission.title[:40])
    assert (
        vevent.summary
        == f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    assert vevent.description == slot.submission.abstract


def test_empty_abstract_round_trips_as_empty_description():
    """pretalx's sanitizer maps a null abstract to the empty string; the
    export must survive an empty DESCRIPTION."""
    slot = Slot(submission=Submission(abstract=""))
    cal = get_slot_ical(slot)
    (vevent,) = roundtrip(cal).events
    assert vevent.description == ""


TEXT_CONTENT = st.text(
    alphabet=st.characters(blacklist_categories=("Cs", "Cc"), blacklist_characters="﻿"),
    max_size=200,
).flatmap(
    lambda s: st.sampled_from(["", "\n", "\t"]).map(lambda sep: s + sep + s[::-1])
)


@settings(max_examples=200, deadline=None)
# A falsy room makes build_slot_vevent skip the slot (pretalx's incomplete
# guard), so rooms are kept non-empty.
@given(title=TEXT_CONTENT, abstract=TEXT_CONTENT, room=TEXT_CONTENT.filter(bool))
def test_arbitrary_text_content_round_trips(title, abstract, room):
    """Any control-char-free text pretalx can produce (titles, abstracts,
    room names — including newlines, tabs, escapes, and unicode) must
    survive the full export/parse cycle unchanged."""
    slot = Slot(submission=Submission(title=title, abstract=abstract), room=room)
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(slot, cal)
    (vevent,) = roundtrip(cal).events
    assert vevent.summary == f"{title} - {slot.submission.display_speaker_names}"
    assert vevent.description == abstract
    assert vevent.location == room


# --------------------------------------------------------------------------
# Third-party readers: the consumers of pretalx's exports.
# --------------------------------------------------------------------------


def _sample_calendar():
    creation = dt.datetime(2026, 7, 1, 12, 0, tzinfo=UTC)
    slot = Slot()
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(slot, cal, creation_time=creation)
    cal.add_missing_timezones()
    return slot, cal


def test_py_vobject_reads_the_export():
    import vobject

    slot, cal = _sample_calendar()
    parsed = vobject.readOne(cal.serialize())
    vevent = parsed.vevent
    assert (
        vevent.summary.value
        == f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    assert vevent.description.value == slot.submission.abstract
    assert vevent.location.value == slot.room
    assert vevent.url.value == slot.submission.url
    assert vevent.dtstart.value == slot.local_start
    assert vevent.dtend.value == slot.local_end


def test_icalendar_reads_the_export():
    import icalendar

    slot, cal = _sample_calendar()
    parsed = icalendar.Calendar.from_ical(cal.serialize())
    (vevent,) = parsed.walk("VEVENT")
    assert (
        vevent["SUMMARY"]
        == f"{slot.submission.title} - {slot.submission.display_speaker_names}"
    )
    assert vevent["DESCRIPTION"] == slot.submission.abstract
    assert vevent.decoded("DTSTART") == slot.local_start
    assert vevent.decoded("DTEND") == slot.local_end
    assert vevent["DTSTART"].params["TZID"] == "Europe/Berlin"
    # The generated VTIMEZONE is one icalendar can interpret.
    (vtz,) = parsed.walk("VTIMEZONE")
    assert str(vtz["TZID"]) == "Europe/Berlin"
    assert vtz.to_tz().utcoffset(dt.datetime(2026, 7, 22, 12, 0)) == dt.timedelta(
        hours=2
    )


# --------------------------------------------------------------------------
# VTIMEZONE generation (add_missing_timezones / vtimezone_from_tzinfo).
# --------------------------------------------------------------------------


def test_add_missing_timezones_inserts_before_events():
    _, cal = _sample_calendar()
    kinds = [
        child.name for child in cal.component.children if isinstance(child, Component)
    ]
    assert kinds == ["VTIMEZONE", "VEVENT"]


def test_all_daylight_window_is_labeled_daylight():
    # A window with no transitions that sits entirely inside DST must be
    # emitted as a DAYLIGHT observance, not STANDARD.
    vtz = vtimezone_from_tzinfo(
        ZoneInfo("America/New_York"),
        start=dt.datetime(2026, 6, 1),
        end=dt.datetime(2026, 7, 1),
    )
    (obs,) = vtz.components()
    assert obs.name == "DAYLIGHT"
    assert obs.prop("TZNAME").value == "EDT"


def test_add_missing_timezones_is_idempotent():
    _, cal = _sample_calendar()
    before = cal.serialize()
    cal.add_missing_timezones()
    assert cal.serialize() == before


def test_add_missing_timezones_ignores_utc_and_naive():
    cal = get_calendar(NETLOC, "democon")
    ev = Component("VEVENT")
    cal.component.children = cal.component.children + [ev]
    tev = Event(ev)
    tev.set_datetime("DTSTAMP", dt.datetime(2026, 7, 1, 12, 0, tzinfo=UTC))
    tev.start = dt.datetime(2026, 7, 22, 10, 0)  # floating
    cal.add_missing_timezones()
    assert cal.timezones == []


def test_add_missing_timezones_respects_existing_definition():
    """A custom TZID already defined in-document must be left alone and
    not shadowed or duplicated."""
    text = (
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"
        "BEGIN:VTIMEZONE\r\nTZID:Custom/Zone\r\n"
        "BEGIN:STANDARD\r\nDTSTART:19700101T000000\r\n"
        "TZOFFSETFROM:+0300\r\nTZOFFSETTO:+0300\r\nEND:STANDARD\r\n"
        "END:VTIMEZONE\r\n"
        "BEGIN:VEVENT\r\nUID:x@y\r\nDTSTAMP:20260701T120000Z\r\n"
        "DTSTART;TZID=Custom/Zone:20260722T100000\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    doc = calcard.parse(text, strict=True)
    (cal,) = doc.calendars
    cal.add_missing_timezones()
    assert [t.tzid for t in cal.timezones] == ["Custom/Zone"]


def test_add_missing_timezones_warns_on_unknown_tzid():
    text = (
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"
        "BEGIN:VEVENT\r\nUID:x@y\r\nDTSTAMP:20260701T120000Z\r\n"
        "DTSTART;TZID=Not/AZone:20260722T100000\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    doc = calcard.parse(text, strict=True)
    (cal,) = doc.calendars
    with pytest.warns(TimezoneResolutionWarning):
        cal.add_missing_timezones()
    assert cal.timezones == []


def _assert_vtimezone_matches_zoneinfo(vtz_component, zone_name, span_start, span_end):
    """Oracle: interpreting the generated VTIMEZONE with calcard's own
    VTIMEZONE engine must agree with host zoneinfo at every probed
    instant across the covered span."""
    built = tzinfo_from_vtimezone(vtz_component)
    zone = ZoneInfo(zone_name)
    probe = span_start
    while probe <= span_end:
        instant = probe.replace(tzinfo=dt.timezone.utc)
        expect = instant.astimezone(zone)
        got = instant.astimezone(built)
        assert got.utcoffset() == expect.utcoffset(), (zone_name, instant)
        assert got.replace(tzinfo=None) == expect.replace(tzinfo=None)
        probe += dt.timedelta(hours=7)


@pytest.mark.parametrize(
    "zone_name",
    [
        "Europe/Berlin",
        "Europe/London",
        "America/Los_Angeles",
        "Asia/Manila",  # no DST
        "Asia/Kolkata",  # half-hour offset, no DST
        "Australia/Lord_Howe",  # 30-minute DST shift
        "Europe/Dublin",  # negative DST in the tz database
        "Pacific/Auckland",  # southern hemisphere
    ],
)
def test_generated_vtimezone_matches_zoneinfo(zone_name):
    start = dt.datetime(2025, 1, 1)
    end = dt.datetime(2027, 1, 1)
    vtz = vtimezone_from_tzinfo(ZoneInfo(zone_name), start=start, end=end)
    assert vtz.prop("TZID").value == zone_name
    # The component must itself serialize as strictly valid iCalendar.
    doc = calcard.parse(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"
        + calcard.serialize(vtz)
        + "END:VCALENDAR\r\n",
        strict=True,
    )
    assert doc.repairs == []
    _assert_vtimezone_matches_zoneinfo(vtz, zone_name, start, end)


def test_generated_vtimezone_shape_for_dst_zone():
    vtz = vtimezone_from_tzinfo(
        BERLIN, start=dt.datetime(2026, 1, 1), end=dt.datetime(2027, 1, 1)
    )
    standards = vtz.comps("STANDARD")
    daylights = vtz.comps("DAYLIGHT")
    assert standards and daylights
    for obs in standards + daylights:
        assert obs.prop("DTSTART") is not None
        assert obs.prop("TZOFFSETFROM") is not None
        assert obs.prop("TZOFFSETTO") is not None
    names = {
        obs.prop("TZNAME").value for obs in standards + daylights if obs.prop("TZNAME")
    }
    assert names == {"CET", "CEST"}


def test_generated_vtimezone_shape_for_fixed_zone():
    vtz = vtimezone_from_tzinfo(
        ZoneInfo("Asia/Kolkata"),
        start=dt.datetime(2026, 1, 1),
        end=dt.datetime(2027, 1, 1),
    )
    (standard,) = vtz.comps("STANDARD")
    assert vtz.comps("DAYLIGHT") == []
    assert standard.prop("TZOFFSETFROM").value == "+0530"
    assert standard.prop("TZOFFSETTO").value == "+0530"


def test_py_vobject_reads_generated_vtimezone():
    import vobject

    _, cal = _sample_calendar()
    parsed = vobject.readOne(cal.serialize())
    tz = parsed.vtimezone.gettzinfo()
    assert tz.utcoffset(dt.datetime(2026, 7, 22, 12, 0)) == dt.timedelta(hours=2)
    assert tz.utcoffset(dt.datetime(2026, 1, 22, 12, 0)) == dt.timedelta(hours=1)


def test_vtimezone_generation_covers_event_span_with_padding():
    """The default coverage window derives from the datetimes that
    reference the zone, padded a year to each side, so the offsets in
    force at the events are always derivable from the table."""
    slot = Slot(
        start=dt.datetime(2026, 3, 29, 0, 30, tzinfo=UTC),  # spring-forward day
        end=dt.datetime(2026, 3, 29, 2, 30, tzinfo=UTC),
    )
    cal = get_slots_ical("democon", [slot])
    (vtz,) = cal.timezones
    built = tzinfo_from_vtimezone(vtz.component)
    for probe_utc in (
        dt.datetime(2026, 3, 28, 12, 0),
        dt.datetime(2026, 3, 29, 0, 59),
        dt.datetime(2026, 3, 29, 1, 1),
        dt.datetime(2026, 6, 1, 12, 0),
    ):
        instant = probe_utc.replace(tzinfo=dt.timezone.utc)
        assert (
            instant.astimezone(built).utcoffset()
            == instant.astimezone(BERLIN).utcoffset()
        )


# --------------------------------------------------------------------------
# Typed-API surface the port relies on.
# --------------------------------------------------------------------------


def test_typed_serialize_matches_module_serialize():
    _, cal = _sample_calendar()
    assert cal.serialize() == calcard.serialize(cal.component)
    assert cal.serialize(line_ending="\n") == calcard.serialize(
        cal.component, line_ending="\n"
    )


def test_set_datetime_is_public_and_z_forms_utc():
    ev = Event(Component("VEVENT"))
    ev.set_datetime("DTSTAMP", dt.datetime(2026, 7, 1, 12, 0, tzinfo=UTC))
    assert ev.component.prop("DTSTAMP").value == "20260701T120000Z"
    ev.set_datetime("DTSTAMP", dt.datetime(2026, 7, 1, 13, 0, tzinfo=BERLIN))
    prop = ev.component.prop("DTSTAMP")
    assert prop.value == "20260701T130000"
    assert [(p.name, p.values) for p in prop.params] == [("TZID", ["Europe/Berlin"])]


def test_add_missing_timezones_available_on_component_level():
    """The functional form works on a bare component, without the typed
    wrapper, mirroring calcard's two-level API."""
    cal = get_calendar(NETLOC, "democon")
    build_slot_vevent(Slot(), cal)
    added = add_missing_timezones(cal.component)
    assert [c.prop("TZID").value for c in added] == ["Europe/Berlin"]
    assert add_missing_timezones(cal.component) == []
