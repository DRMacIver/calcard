"""Timezone-aware recurrence expansion across DST transitions.

Expected values are ported from sabre/calcard's RRuleIteratorTest DST data
providers (Europe/Zurich; spring-forward 2023-03-26 02:00 → 03:00), the
only reference implementation with explicit DST-crossing expectations.
Semantics (RFC 5545 §3.3.5): the recurrence is generated on the local wall
clock; a wall time in the spring-forward gap resolves with the pre-gap
offset, landing after the gap.
"""

import datetime as dt
from zoneinfo import ZoneInfo

import pytest

import calcard

ZURICH = ZoneInfo("Europe/Zurich")


def local(spec: str) -> dt.datetime:
    naive = dt.datetime.strptime(spec, "%Y-%m-%d %H:%M:%S")
    return naive.replace(tzinfo=ZURICH)


def expand_local(rule: str, start: str) -> list[str]:
    got = calcard.expand_rrule(rule, local(start))
    return [d.strftime("%Y-%m-%d %H:%M:%S") for d in got]


# sabre dst2HourlyTransitionProvider
@pytest.mark.parametrize(
    ("start", "expected"),
    [
        (
            "2023-03-26 00:00:00",
            [
                "2023-03-26 00:00:00",
                "2023-03-26 03:00:00",
                "2023-03-26 04:00:00",
                "2023-03-26 06:00:00",
                "2023-03-26 08:00:00",
            ],
        ),
        (
            "2023-03-26 00:15:00",
            [
                "2023-03-26 00:15:00",
                "2023-03-26 03:15:00",
                "2023-03-26 04:15:00",
                "2023-03-26 06:15:00",
                "2023-03-26 08:15:00",
            ],
        ),
        (
            "2023-03-26 01:00:00",
            [
                "2023-03-26 01:00:00",
                "2023-03-26 03:00:00",
                "2023-03-26 05:00:00",
                "2023-03-26 07:00:00",
                "2023-03-26 09:00:00",
            ],
        ),
        (
            "2023-03-26 01:15:00",
            [
                "2023-03-26 01:15:00",
                "2023-03-26 03:15:00",
                "2023-03-26 05:15:00",
                "2023-03-26 07:15:00",
                "2023-03-26 09:15:00",
            ],
        ),
    ],
)
def test_2hourly_across_spring_forward(start, expected):
    assert expand_local("FREQ=HOURLY;INTERVAL=2;COUNT=5", start) == expected


# sabre dst6HourlyTransitionProvider
@pytest.mark.parametrize(
    ("start", "expected"),
    [
        (
            "2023-03-25 20:00:00",
            [
                "2023-03-25 20:00:00",
                "2023-03-26 03:00:00",
                "2023-03-26 08:00:00",
                "2023-03-26 14:00:00",
                "2023-03-26 20:00:00",
            ],
        ),
        (
            "2023-03-25 21:00:00",
            [
                "2023-03-25 21:00:00",
                "2023-03-26 03:00:00",
                "2023-03-26 09:00:00",
                "2023-03-26 15:00:00",
                "2023-03-26 21:00:00",
            ],
        ),
    ],
)
def test_6hourly_across_spring_forward(start, expected):
    assert expand_local("FREQ=HOURLY;INTERVAL=6;COUNT=5", start) == expected


# sabre dstDailyTransitionProvider
@pytest.mark.parametrize(
    ("start", "expected"),
    [
        (
            "2023-03-24 02:00:00",
            [
                "2023-03-24 02:00:00",
                "2023-03-25 02:00:00",
                "2023-03-26 03:00:00",
                "2023-03-27 02:00:00",
                "2023-03-28 02:00:00",
            ],
        ),
        (
            "2023-03-24 03:00:00",
            [
                "2023-03-24 03:00:00",
                "2023-03-25 03:00:00",
                "2023-03-26 03:00:00",
                "2023-03-27 03:00:00",
                "2023-03-28 03:00:00",
            ],
        ),
    ],
)
def test_daily_across_spring_forward(start, expected):
    assert expand_local("FREQ=DAILY;INTERVAL=1;COUNT=5", start) == expected


# sabre testWeeklyByDaySpecificHourOnDstTransition
def test_weekly_across_spring_forward():
    got = calcard.expand_rrule(
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=SA,SU",
        local("2023-03-11 02:30:00"),
        limit=6,
    )
    assert [d.strftime("%Y-%m-%d %H:%M:%S") for d in got] == [
        "2023-03-11 02:30:00",
        "2023-03-12 02:30:00",
        "2023-03-25 02:30:00",
        "2023-03-26 03:30:00",
        "2023-04-08 02:30:00",
        "2023-04-09 02:30:00",
    ]


def test_instances_are_real_instants():
    """The gap instance is not just formatted as 03:00 — it is the correct
    absolute time (01:00Z, the pre-gap offset applied to the wall time)."""
    got = calcard.expand_rrule("FREQ=DAILY;COUNT=3", local("2023-03-25 02:00:00"))
    instants = [d.astimezone(dt.timezone.utc) for d in got]
    assert instants == [
        # 02:00 CET (+01:00) before the transition,
        dt.datetime(2023, 3, 25, 1, 0, tzinfo=dt.timezone.utc),
        # the gap time resolved with the pre-gap offset,
        dt.datetime(2023, 3, 26, 1, 0, tzinfo=dt.timezone.utc),
        # and 02:00 CEST (+02:00) once summer time is in force.
        dt.datetime(2023, 3, 27, 0, 0, tzinfo=dt.timezone.utc),
    ]


def test_fall_back_ambiguous_takes_first_occurrence():
    # Zurich falls back 2023-10-29 03:00 -> 02:00; 02:30 happens twice.
    got = calcard.expand_rrule("FREQ=DAILY;COUNT=2", local("2023-10-28 02:30:00"))
    instants = [d.astimezone(dt.timezone.utc) for d in got]
    assert instants == [
        dt.datetime(2023, 10, 28, 0, 30, tzinfo=dt.timezone.utc),
        # First (CEST, +02:00) occurrence of the ambiguous wall time.
        dt.datetime(2023, 10, 29, 0, 30, tzinfo=dt.timezone.utc),
    ]


def test_utc_until_bounds_zoned_expansion():
    # UNTIL is an instant: 2023-03-26 01:30:00Z == 03:30 Zurich wall time
    # after the transition.
    got = calcard.expand_rrule(
        "FREQ=HOURLY;INTERVAL=2;UNTIL=20230326T013000Z",
        local("2023-03-26 00:00:00"),
    )
    assert [d.strftime("%Y-%m-%d %H:%M:%S") for d in got] == [
        "2023-03-26 00:00:00",
        "2023-03-26 03:00:00",
    ]


def test_event_occurrences_are_dst_aware():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "DTSTART;TZID=Europe/Zurich:20230325T020000\r\n"
        "RRULE:FREQ=DAILY;COUNT=3\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    event = calcard.parse(text).calendars[0].events[0]
    got = [o.strftime("%Y-%m-%d %H:%M:%S") for o in event.occurrences()]
    assert got == [
        "2023-03-25 02:00:00",
        "2023-03-26 03:00:00",
        "2023-03-27 02:00:00",
    ]
