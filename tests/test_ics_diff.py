"""Tests for the ics_diff diff engine in the py-vobject compatibility layer."""

import datetime

import calcard.compat.pyvobject
from calcard.compat.pyvobject.ics_diff import diff, prettyDiff


def build_calendar(events):
    """Build a VCALENDAR from (uid, dtstart, summary) tuples."""
    cal = calcard.compat.pyvobject.iCalendar()
    for uid, dtstart, summary in events:
        event = cal.add("vevent")
        event.add("uid").value = uid
        event.add("dtstart").value = dtstart
        event.add("summary").value = summary
    return cal


def test_identical_calendars_have_no_differences():
    events = [
        ("a@example.com", datetime.datetime(2026, 3, 1, 9, 0), "Breakfast"),
        ("b@example.com", datetime.datetime(2026, 3, 2, 12, 0), "Lunch"),
    ]
    assert diff(build_calendar(events), build_calendar(events)) == []


def test_changed_dtstart_is_detected():
    left = build_calendar([("a@example.com", datetime.datetime(2026, 3, 1, 9, 0), "Breakfast")])
    right = build_calendar([("a@example.com", datetime.datetime(2026, 3, 1, 10, 0), "Breakfast")])
    result = diff(left, right)
    assert len(result) == 1
    left_diff, right_diff = result[0]
    assert left_diff.uid.value == "a@example.com"
    assert right_diff.uid.value == "a@example.com"
    assert left_diff.dtstart.value == datetime.datetime(2026, 3, 1, 9, 0)
    assert right_diff.dtstart.value == datetime.datetime(2026, 3, 1, 10, 0)


def test_event_missing_from_one_side_is_detected():
    both = ("a@example.com", datetime.datetime(2026, 3, 1, 9, 0), "Breakfast")
    only_left = ("b@example.com", datetime.datetime(2026, 3, 2, 12, 0), "Lunch")
    left = build_calendar([both, only_left])
    right = build_calendar([both])
    result = diff(left, right)
    assert len(result) == 1
    left_diff, right_diff = result[0]
    assert right_diff is None
    assert left_diff.uid.value == "b@example.com"

    # And the mirror image: present only on the right.
    result = diff(right, left)
    assert len(result) == 1
    left_diff, right_diff = result[0]
    assert left_diff is None
    assert right_diff.uid.value == "b@example.com"


def _calendars_with_differing_alarms():
    left = build_calendar([("a@example.com", datetime.datetime(2026, 3, 1, 9, 0), "Breakfast")])
    right = build_calendar([("a@example.com", datetime.datetime(2026, 3, 1, 9, 0), "Breakfast")])
    alarm = left.vevent.add("valarm")
    alarm.add("action").value = "DISPLAY"
    alarm.add("description").value = "Reminder"
    return left, right


def test_differing_subcomponents_are_stored_as_reusable_lists():
    # Regression test: differing subcomponents used to be stored as a
    # single-use filter() iterator, so the second traversal of the diff
    # component's contents silently saw nothing.
    left, right = _calendars_with_differing_alarms()
    result = diff(left, right)
    assert len(result) == 1
    left_diff, right_diff = result[0]
    assert right_diff is not None
    first_pass = list(left_diff.contents["valarm"])
    second_pass = list(left_diff.contents["valarm"])
    assert len(first_pass) == 1
    assert second_pass == first_pass
    assert first_pass[0].action.value == "DISPLAY"


def test_pretty_diff_prints_differing_subcomponents(capsys):
    # prettyPrint is a second traversal after diff() already walked the
    # contents, so this also exercises the single-use-iterator regression.
    left, right = _calendars_with_differing_alarms()
    prettyDiff(left, right)
    captured = capsys.readouterr()
    assert "<<<<<<<<<<<<<<<" in captured.out
    assert ">>>>>>>>>>>>>>>" in captured.out
    assert "VALARM" in captured.out
