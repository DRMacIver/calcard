"""Tests for the typed component views."""

import datetime as dt
from zoneinfo import ZoneInfo

import calcard

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
    doc = calcard.parse(CAL)
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
    (cal,) = calcard.parse(CAL).calendars
    (event,) = cal.events
    got = event.occurrences()
    assert got == [
        dt.datetime(2026, 7, 22, 16, 0, tzinfo=LONDON),
        dt.datetime(2026, 7, 29, 16, 0, tzinfo=LONDON),
        dt.datetime(2026, 8, 5, 16, 0, tzinfo=LONDON),
    ]


def test_event_without_rrule_occurs_once():
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    event.component.children = [
        c
        for c in event.component.children
        if not (isinstance(c, calcard.Property) and c.name == "RRULE")
    ]
    assert event.occurrences() == [event.start]


def test_mutation_via_typed_view_reflects_in_serialization():
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    event.summary = "Coffee; with cream"
    event.start = dt.datetime(2026, 7, 23, 9, 0, tzinfo=LONDON)
    out = doc.serialize()
    assert "SUMMARY:Coffee\\; with cream\r\n" in out
    assert "DTSTART;TZID=Europe/London:20260723T090000\r\n" in out
    # The document reparses cleanly and the view agrees.
    again = calcard.parse(out, strict=True)
    assert again.calendars[0].events[0].summary == "Coffee; with cream"


def test_setting_utc_and_date_values():
    doc = calcard.parse(CAL)
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
    event = calcard.parse(text).calendars[0].events[0]
    assert event.end == dt.date(2026, 7, 23)
    assert event.duration == dt.timedelta(days=1)


def test_duration_used_for_end():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTSTART:20260722T160000Z\r\nDURATION:PT2H\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    event = calcard.parse(text).calendars[0].events[0]
    assert event.end == dt.datetime(2026, 7, 22, 18, 0, tzinfo=dt.timezone.utc)


def test_todo_due():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n"
        "SUMMARY:Fix\r\nDUE:20260801T120000Z\r\n"
        "END:VTODO\r\nEND:VCALENDAR\r\n"
    )
    todo = calcard.parse(text).calendars[0].todos[0]
    assert todo.summary == "Fix"
    assert todo.due == dt.datetime(2026, 8, 1, 12, 0, tzinfo=dt.timezone.utc)


def test_fixed_offset_datetime_preserves_instant():
    # Regression: an aware tzinfo without a zone name (e.g. a fixed
    # timezone(timedelta)) used to be written as floating local time,
    # silently changing the instant.
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    ist = dt.timezone(dt.timedelta(hours=5, minutes=30))
    value = dt.datetime(2026, 7, 22, 10, 0, tzinfo=ist)
    event.start = value
    assert event.start == value
    out = doc.serialize()
    assert "DTSTART:20260722T043000Z\r\n" in out
    again = calcard.parse(out, strict=True)
    assert again.calendars[0].events[0].start == value


def test_negative_fixed_offset_datetime_preserves_instant():
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    value = dt.datetime(
        2026, 1, 2, 1, 30, tzinfo=dt.timezone(dt.timedelta(hours=-7))
    )
    event.end = value
    assert event.end == value
    assert "DTEND:20260102T083000Z\r\n" in doc.serialize()


def test_todo_due_setter_fixed_offset_round_trips():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VTODO\r\n"
        "SUMMARY:Fix\r\nEND:VTODO\r\nEND:VCALENDAR\r\n"
    )
    doc = calcard.parse(text)
    todo = doc.calendars[0].todos[0]
    value = dt.datetime(
        2026, 8, 1, 12, 0, tzinfo=dt.timezone(dt.timedelta(hours=3))
    )
    todo.due = value
    assert todo.due == value
    again = calcard.parse(doc.serialize(), strict=True)
    assert again.calendars[0].todos[0].due == value


def test_pytz_style_zone_attribute_used_as_tzid():
    pytz = __import__("pytz")
    zone = pytz.timezone("Europe/London")
    value = zone.localize(dt.datetime(2026, 7, 22, 16, 0))
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    event.start = value
    assert "DTSTART;TZID=Europe/London:20260722T160000\r\n" in doc.serialize()
    assert event.start == value


def test_set_datetime_preserves_unrelated_params():
    # Regression: setting .end used to replace the whole params list,
    # discarding parameters the setter does not manage.
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTEND;X-FOO=bar;TZID=Europe/London:20260722T163000\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    event = calcard.parse(text).calendars[0].events[0]
    event.end = dt.datetime(2026, 7, 23, 10, 0, tzinfo=LONDON)
    params = {p.name.upper(): p.values for p in event.component.prop("DTEND").params}
    assert params.get("X-FOO") == ["bar"]
    assert params.get("TZID") == ["Europe/London"]

    # Switching to a date keeps X-FOO, drops TZID, and adds VALUE=DATE.
    event.end = dt.date(2026, 7, 23)
    params = {p.name.upper(): p.values for p in event.component.prop("DTEND").params}
    assert params.get("X-FOO") == ["bar"]
    assert "TZID" not in params
    assert params.get("VALUE") == ["DATE"]

    # And back to a UTC datetime: X-FOO survives, VALUE=DATE is removed.
    event.end = dt.datetime(2026, 7, 24, 9, 0, tzinfo=dt.timezone.utc)
    params = {p.name.upper(): p.values for p in event.component.prop("DTEND").params}
    assert params.get("X-FOO") == ["bar"]
    assert "TZID" not in params
    assert "VALUE" not in params


def test_card_accessors():
    (card,) = calcard.parse(CARD).cards
    assert card.fn == "Alice Example"
    assert card.version == "4.0"
    assert card.n == [["Example"], ["Alice"], [""], [""], [""]]
    assert card.emails == ["alice@example.com", "work@example.com"]


def test_wrap_dispatch():
    doc = calcard.parse(CAL + CARD)
    views = [calcard.wrap(c) for c in doc]
    assert isinstance(views[0], calcard.Calendar)
    assert isinstance(views[1], calcard.Card)
    unknown = calcard.Component("X-CUSTOM")
    assert type(calcard.wrap(unknown)) is calcard.TypedComponent


def test_wrong_component_type_rejected():
    import pytest

    (card,) = calcard.parse(CARD).components
    with pytest.raises(ValueError):
        calcard.Event(card)


def _event_of(body: str) -> calcard.Event:
    text = f"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n{body}END:VEVENT\r\nEND:VCALENDAR\r\n"
    return calcard.parse(text).calendars[0].events[0]


def test_card_vcard3_dialect():
    card3 = calcard.parse(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Bob\r\nEND:VCARD\r\n"
    ).cards[0]
    assert card3.DIALECT == "vcard3"
    card21 = calcard.parse(
        "BEGIN:VCARD\r\nVERSION:2.1\r\nFN:Bob\r\nEND:VCARD\r\n"
    ).cards[0]
    assert card21.DIALECT == "vcard3"
    card4 = calcard.parse(CARD).cards[0]
    assert card4.DIALECT == "vcard4"


def test_card_tels():
    card = calcard.parse(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Bob\r\n"
        "TEL:+441234\r\nTEL:+445678\r\nEND:VCARD\r\n"
    ).cards[0]
    assert card.tels == ["+441234", "+445678"]


def test_explicit_duration_wins():
    event = _event_of(
        "DTSTART:20260722T160000Z\r\nDTEND:20260722T163000Z\r\nDURATION:PT2H\r\n"
    )
    assert event.duration == dt.timedelta(hours=2)


def test_duration_none_when_start_or_end_missing():
    assert _event_of("DTEND:20260722T163000Z\r\n").duration is None
    assert _event_of("SUMMARY:x\r\n").duration is None


def test_duration_none_for_mixed_date_and_datetime():
    event = _event_of(
        "DTSTART;VALUE=DATE:20260722\r\nDTEND:20260723T100000Z\r\n"
    )
    assert event.duration is None


def test_end_none_without_start():
    assert _event_of("SUMMARY:x\r\n").end is None


def test_datetime_start_defaults_to_zero_duration_end():
    event = _event_of("DTSTART:20260722T160000Z\r\n")
    assert event.end == dt.datetime(2026, 7, 22, 16, 0, tzinfo=dt.timezone.utc)
    assert event.duration == dt.timedelta(0)


def test_uid_setter():
    event = _event_of("SUMMARY:x\r\n")
    event.uid = "new-uid-1"
    assert event.uid == "new-uid-1"
    assert event.component.prop("UID").value == "new-uid-1"


def test_end_setter_creates_property():
    event = _event_of("DTSTART:20260722T160000Z\r\n")
    event.end = dt.datetime(2026, 7, 22, 17, 0, tzinfo=dt.timezone.utc)
    assert event.component.prop("DTEND").value == "20260722T170000Z"
    assert event.end == dt.datetime(2026, 7, 22, 17, 0, tzinfo=dt.timezone.utc)


def test_repr_and_eq():
    doc = calcard.parse(CAL)
    event = doc.calendars[0].events[0]
    assert repr(event).startswith("Event(")
    assert event == doc.calendars[0].events[0]
    assert event != doc.calendars[0]
    assert not (event == "not a view")


def test_text_default():
    event = _event_of("SUMMARY:x\r\n")
    assert event.text("LOCATION") is None
    assert event.text("LOCATION", "nowhere") == "nowhere"


def test_set_text_appends_when_missing():
    event = _event_of("SUMMARY:x\r\n")
    event.description = "line one\nline two, with comma"
    assert event.component.prop("DESCRIPTION") is not None
    assert event.description == "line one\nline two, with comma"


def test_occurrences_without_dtstart():
    assert _event_of("SUMMARY:x\r\n").occurrences() == []


def test_timezone_tzid():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/London\r\n"
        "END:VTIMEZONE\r\nEND:VCALENDAR\r\n"
    )
    cal = calcard.parse(text).calendars[0]
    (tz,) = cal.timezones
    assert tz.tzid == "Europe/London"


def test_calendar_journals():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VJOURNAL\r\nSUMMARY:Dear diary\r\n"
        "DTSTART;VALUE=DATE:20260722\r\nEND:VJOURNAL\r\nEND:VCALENDAR\r\n"
    )
    cal = calcard.parse(text).calendars[0]
    (journal,) = cal.journals
    assert journal.summary == "Dear diary"
    assert journal.start == dt.date(2026, 7, 22)


def test_status_setter():
    ev = calcard.parse(CAL).calendars[0].events[0]
    assert ev.status is None
    ev.status = "CANCELLED"
    assert ev.status == "CANCELLED"
    # A freshly parsed copy is unaffected.
    assert calcard.parse(CAL).calendars[0].events[0].status is None


def test_status_setter_serializes():
    doc = calcard.parse(CAL)
    ev = doc.calendars[0].events[0]
    ev.status = "TENTATIVE"
    assert "STATUS:TENTATIVE" in doc.serialize()


def test_rrule_setter_round_trips():
    doc = calcard.parse(CAL)
    ev = doc.calendars[0].events[0]
    ev.rrule = "FREQ=DAILY;COUNT=4"
    assert ev.rrule == "FREQ=DAILY;COUNT=4"
    assert len(ev.occurrences()) == 4
    assert "RRULE:FREQ=DAILY;COUNT=4" in doc.serialize()


def test_rrule_setter_creates_property():
    doc = calcard.parse(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTSTART:20260101T090000\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    ev = doc.calendars[0].events[0]
    assert ev.rrule is None
    ev.rrule = "FREQ=YEARLY;COUNT=2"
    assert len(ev.occurrences()) == 2


def test_rrule_setter_rejects_invalid_rules():
    import pytest

    ev = calcard.parse(CAL).calendars[0].events[0]
    for bad in ["FREQ=BOGUS", "COUNT=3", "", "FREQ=DAILY;INTERVAL=0"]:
        with pytest.raises(calcard.ParseError):
            ev.rrule = bad
    # The original rule is untouched after a failed assignment.
    assert ev.rrule == "FREQ=WEEKLY;COUNT=3"
