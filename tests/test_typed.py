"""Tests for the typed component views."""

import datetime as dt
from zoneinfo import ZoneInfo

import vobject

CAL = (
    "BEGIN:VCALENDAR\r\n"
    "VERSION:2.0\r\n"
    "PRODID:-//test//EN\r\n"
    "BEGIN:VEVENT\r\n"
    "UID:abc-123\r\n"
    "SUMMARY:Tea\\, obviously\r\n"
    "DTSTART;TZID=Europe/London:20260722T160000\r\n"
    "DTEND;TZID=Europe/London:20260722T163000\r\n"
    "RRULE:FREQ=WEEKLY;COUNT=3\r\n"
    "BEGIN:VALARM\r\n"
    "ACTION:DISPLAY\r\n"
    "TRIGGER:-PT15M\r\n"
    "END:VALARM\r\n"
    "END:VEVENT\r\n"
    "END:VCALENDAR\r\n"
)

CARD = (
    "BEGIN:VCARD\r\n"
    "VERSION:4.0\r\n"
    "FN:Alice Example\r\n"
    "N:Example;Alice;;;\r\n"
    "EMAIL:alice@example.com\r\n"
    "EMAIL:work@example.com\r\n"
    "END:VCARD\r\n"
)

LONDON = ZoneInfo("Europe/London")


def test_calendar_and_event_accessors():
    doc = vobject.parse(CAL)
    (cal,) = doc.calendars
    assert cal.version == "2.0"
    assert cal.prodid == "-//test//EN"
    (event,) = cal.events
    assert event.uid == "abc-123"
    assert event.summary == "Tea, obviously"
    assert event.start == dt.datetime(2026, 7, 22, 16, 0, tzinfo=LONDON)
    assert event.end == dt.datetime(2026, 7, 22, 16, 30, tzinfo=LONDON)
    assert event.duration == dt.timedelta(minutes=30)
    (alarm,) = event.alarms
    assert alarm.action == "DISPLAY"
    assert alarm.trigger == dt.timedelta(minutes=-15)


def test_occurrences():
    (cal,) = vobject.parse(CAL).calendars
    (event,) = cal.events
    got = event.occurrences()
    assert got == [
        dt.datetime(2026, 7, 22, 16, 0, tzinfo=LONDON),
        dt.datetime(2026, 7, 29, 16, 0, tzinfo=LONDON),
        dt.datetime(2026, 8, 5, 16, 0, tzinfo=LONDON),
    ]


def test_event_without_rrule_occurs_once():
    doc = vobject.parse(CAL)
    event = doc.calendars[0].events[0]
    event.component.children = [
        c
        for c in event.component.children
        if not (isinstance(c, vobject.Property) and c.name == "RRULE")
    ]
    assert event.occurrences() == [event.start]


def test_mutation_via_typed_view_reflects_in_serialization():
    doc = vobject.parse(CAL)
    event = doc.calendars[0].events[0]
    event.summary = "Coffee; with cream"
    event.start = dt.datetime(2026, 7, 23, 9, 0, tzinfo=LONDON)
    out = doc.serialize()
    assert "SUMMARY:Coffee\\; with cream\r\n" in out
    assert "DTSTART;TZID=Europe/London:20260723T090000\r\n" in out
    # The document reparses cleanly and the view agrees.
    again = vobject.parse(out, strict=True)
    assert again.calendars[0].events[0].summary == "Coffee; with cream"


def test_setting_utc_and_date_values():
    doc = vobject.parse(CAL)
    event = doc.calendars[0].events[0]
    event.start = dt.datetime(2026, 7, 23, 9, 0, tzinfo=dt.timezone.utc)
    assert "DTSTART:20260723T090000Z\r\n" in doc.serialize()
    event.start = dt.date(2026, 7, 23)
    assert "DTSTART;VALUE=DATE:20260723\r\n" in doc.serialize()
    assert event.start == dt.date(2026, 7, 23)


def test_date_event_default_end_is_next_day():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTSTART;VALUE=DATE:20260722\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    event = vobject.parse(text).calendars[0].events[0]
    assert event.end == dt.date(2026, 7, 23)
    assert event.duration == dt.timedelta(days=1)


def test_duration_used_for_end():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTSTART:20260722T160000Z\r\nDURATION:PT2H\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    event = vobject.parse(text).calendars[0].events[0]
    assert event.end == dt.datetime(2026, 7, 22, 18, 0, tzinfo=dt.timezone.utc)


def test_todo_due():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n"
        "SUMMARY:Fix\r\nDUE:20260801T120000Z\r\n"
        "END:VTODO\r\nEND:VCALENDAR\r\n"
    )
    todo = vobject.parse(text).calendars[0].todos[0]
    assert todo.summary == "Fix"
    assert todo.due == dt.datetime(2026, 8, 1, 12, 0, tzinfo=dt.timezone.utc)


def test_card_accessors():
    (card,) = vobject.parse(CARD).cards
    assert card.fn == "Alice Example"
    assert card.version == "4.0"
    assert card.n == [["Example"], ["Alice"], [""], [""], [""]]
    assert card.emails == ["alice@example.com", "work@example.com"]


def test_wrap_dispatch():
    doc = vobject.parse(CAL + CARD)
    views = [vobject.wrap(c) for c in doc]
    assert isinstance(views[0], vobject.Calendar)
    assert isinstance(views[1], vobject.Card)
    unknown = vobject.Component("X-CUSTOM")
    assert type(vobject.wrap(unknown)) is vobject.TypedComponent


def test_wrong_component_type_rejected():
    import pytest

    (card,) = vobject.parse(CARD).components
    with pytest.raises(ValueError):
        vobject.Event(card)
