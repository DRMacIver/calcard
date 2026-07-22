"""Cross-implementation recurrence tests: calcard.expand_rrule vs
dateutil.rrule.rrulestr.

Hypothesis generates structured RRULEs (all FREQ values; INTERVAL; COUNT or
UNTIL; BYDAY including ordinals; BYMONTH; BYMONTHDAY; BYSETPOS;
BYHOUR/BYMINUTE; WKST) plus a naive DTSTART, expands with both engines, and
requires the first N instances to agree exactly.

Known deviations (constrained out of the generator, each verified against a
third implementation):

* FREQ=YEARLY with BYMONTHDAY but no BYMONTH: calcard pins DTSTART's month
  (only that month's matching days recur), agreeing with libical and ical.js
  (see conformance/fixtures/libical/recur/icalrecur_test.txt, the
  ``FREQ=YEARLY;BYMONTHDAY=29`` case, whose expected instances are Feb 29
  only); dateutil expands over every month of the year. RFC 5545 leaves the
  unspecified-BYMONTH case to the "limit if absent" reading of its expansion
  table, and calcard deliberately follows the libical majority. The
  generator therefore only emits BYMONTHDAY for YEARLY together with an
  explicit BYMONTH; ``test_yearly_bymonthday_pinning_deviation`` documents
  both behaviours so drift in either engine is caught.

* Same-unit BYxxx unreachable under INTERVAL (e.g.
  ``FREQ=MINUTELY;INTERVAL=2;BYMINUTE=0`` from a DTSTART at an odd minute):
  dateutil raises ``ValueError("Invalid rrule byxxx generates an empty
  set.")`` at construction time, while calcard treats a never-matching rule
  as simply yielding no instances (RFC 5545 does not make such rules
  errors, and libical expands them to nothing). The generator keeps
  HOURLY+BYHOUR / MINUTELY+BYMINUTE combinations reachable from DTSTART
  modulo gcd(INTERVAL, base), mirroring dateutil's reachability rule.

* Mixed plain and ordinal entries in one BYDAY list (e.g.
  ``FREQ=YEARLY;BYDAY=MO,1MO``): RFC 5545 3.3.10 makes a BYDAY list a
  union of its entries ("every Monday" union "the first Monday"), which is
  how calcard and libical expand it; dateutil applies its plain-weekday set
  and its ordinal-weekday set as two independent filters, i.e. the
  intersection, which can even be empty (``BYDAY=MO,1TU`` never matches
  under dateutil). The generator emits either all-plain or all-ordinal
  BYDAY lists, never a mixture.

* WEEKLY + BYSETPOS when DTSTART is mid-week: RFC 5545 says BYSETPOS
  "operates on a set of recurrence instances in one interval of the
  recurrence rule"; calcard builds the full week's set and only afterwards
  discards instances before DTSTART, which matches the libical fixture
  ``FREQ=WEEKLY;BYDAY=MO,TU,SU,SA,TH;BYSETPOS=3,2`` from DTSTART Tuesday
  2024-01-02 (expected instances include Jan 2 and Jan 4 — positions
  computed over the whole Mon-Sun week). dateutil truncates the first week
  at DTSTART before applying BYSETPOS and so selects different days in
  that week (its other frequencies use full calendar periods and agree).
  The generator only attaches BYSETPOS to WEEKLY rules whose DTSTART falls
  on the week start (Monday, with no WKST override).

Generator bounds that are NOT deviations, just tractability constraints:
never-matching or absurdly-sparse rules are excluded (e.g. BYMONTH=2 with
BYMONTHDAY=31) because dateutil iterates its base frequency step by step and
would scan effectively forever, and calcard's lenient engine deliberately
abandons rules after ``max_empty_periods`` consecutive empty periods.
Sub-daily frequencies only combine with BY parts whose worst-case gap stays
within both engines' comfortable ranges.
"""

import datetime as dt
from itertools import islice

from dateutil.rrule import rrulestr
from hypothesis import given, settings, strategies as st

import calcard

N_INSTANCES = 25

WEEKDAYS = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]

# Maximum length each month ever has (Feb: leap years).
MONTH_MAX_DAYS = {
    1: 31, 2: 29, 3: 31, 4: 30, 5: 31, 6: 30,
    7: 31, 8: 31, 9: 30, 10: 31, 11: 30, 12: 31,
}


def _satisfiable(months, monthdays):
    """Some month in ``months`` can ever contain some day in ``monthdays``."""
    return any(
        (d > 0 and d <= MONTH_MAX_DAYS[m]) or (d < 0 and -d <= MONTH_MAX_DAYS[m])
        for m in months
        for d in monthdays
    )


dtstarts = st.datetimes(
    min_value=dt.datetime(1990, 1, 1),
    max_value=dt.datetime(2035, 12, 31, 23, 59, 59),
).map(lambda d: d.replace(microsecond=0))


@st.composite
def rules_with_start(draw):
    dtstart = draw(dtstarts)
    freq = draw(
        st.sampled_from(
            ["SECONDLY", "MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "YEARLY"]
        )
    )
    parts = [f"FREQ={freq}"]
    interval = 1
    if draw(st.booleans()):
        interval = draw(st.integers(1, 4))
        parts.append(f"INTERVAL={interval}")

    months = []
    if freq in ("YEARLY", "MONTHLY", "WEEKLY", "DAILY", "HOURLY") and draw(
        st.booleans()
    ):
        months = sorted(
            draw(st.sets(st.integers(1, 12), min_size=1, max_size=3))
        )

    # BYMONTHDAY: forbidden for WEEKLY (RFC 5545 3.3.10); for YEARLY only
    # together with BYMONTH (see module docstring); for HOURLY only with
    # magnitudes <= 28 when BYMONTH is present, so a match happens every
    # year and neither engine scans an unbounded gap hour by hour.
    monthdays = []
    monthday_ok = freq in ("MONTHLY", "DAILY", "HOURLY") or (
        freq == "YEARLY" and months
    )
    if monthday_ok and draw(st.booleans()):
        magnitude = 28 if (freq == "HOURLY" and months) else 31
        candidates = st.integers(-magnitude, magnitude).filter(lambda d: d != 0)
        monthdays = sorted(
            draw(st.sets(candidates, min_size=1, max_size=3))
        )
        if months and not _satisfiable(months, monthdays):
            monthdays = []

    # BYDAY. Ordinal prefixes only for MONTHLY/YEARLY, and (RFC 5545) never
    # together with BYMONTHDAY. DAILY/HOURLY skip BYDAY when BYMONTHDAY is
    # present so the worst-case match gap stays bounded (Feb-29-that-is-a-
    # Monday style rules only recur every ~28 years).
    bydays = []
    byday_ok = freq != "SECONDLY" and freq != "MINUTELY"
    if freq in ("DAILY", "HOURLY") and monthdays:
        byday_ok = False
    if byday_ok and draw(st.booleans()):
        # All-plain or all-ordinal, never mixed (see module docstring).
        allow_ordinals = freq in ("MONTHLY", "YEARLY") and not monthdays
        ordinal_mode = allow_ordinals and draw(st.booleans())
        ordinal = (
            st.integers(-5, 5).filter(lambda n: n != 0)
            if ordinal_mode
            else st.just(0)
        )
        entries = draw(
            st.lists(
                st.tuples(st.sampled_from(WEEKDAYS), ordinal),
                min_size=1,
                max_size=3,
                unique=True,
            )
        )
        bydays = [f"{ordinal or ''}{day}" for day, ordinal in entries]

    from math import gcd

    hours = []
    if freq != "SECONDLY" and draw(st.booleans()):
        hours = sorted(draw(st.sets(st.integers(0, 23), min_size=1, max_size=3)))
        if freq == "HOURLY":
            # Keep at least one hour reachable from DTSTART under INTERVAL
            # (see the same-unit-BYxxx deviation in the module docstring).
            step = gcd(interval, 24)
            hours = [h for h in hours if (h - dtstart.hour) % step == 0]

    minutes = []
    if freq != "SECONDLY" and draw(st.booleans()):
        minutes = sorted(draw(st.sets(st.integers(0, 59), min_size=1, max_size=2)))
        if freq == "MINUTELY":
            step = gcd(interval, 60)
            minutes = [m for m in minutes if (m - dtstart.minute) % step == 0]

    # BYSETPOS only in shapes where a conservative lower bound on the
    # per-period set size is known, so |pos| always selects something and
    # neither engine is asked to scan for a never-matching rule.
    guaranteed = 0
    plain_bydays = bydays and all(not b[0].isdigit() and b[0] != "-" for b in bydays)
    if freq == "MONTHLY" and plain_bydays and not monthdays and not months:
        guaranteed = 4 * len(bydays)
    elif freq == "YEARLY" and plain_bydays and months and not monthdays:
        guaranteed = 4 * len(bydays)
    elif (
        freq == "WEEKLY" and bydays and not months and dtstart.weekday() == 0
    ):
        # Only from a Monday DTSTART (see the WEEKLY+BYSETPOS deviation in
        # the module docstring).
        guaranteed = len(bydays)
    elif freq == "DAILY" and hours and not months and not monthdays and not bydays:
        guaranteed = len(hours)
    setposes = []
    if guaranteed and draw(st.booleans()):
        bound = min(guaranteed, 3)
        setposes = sorted(
            draw(
                st.sets(
                    st.integers(-bound, bound).filter(lambda p: p != 0),
                    min_size=1,
                    max_size=2,
                )
            )
        )

    if months:
        parts.append("BYMONTH=" + ",".join(map(str, months)))
    if monthdays:
        parts.append("BYMONTHDAY=" + ",".join(map(str, monthdays)))
    if bydays:
        parts.append("BYDAY=" + ",".join(bydays))
    if hours:
        parts.append("BYHOUR=" + ",".join(map(str, hours)))
    if minutes:
        parts.append("BYMINUTE=" + ",".join(map(str, minutes)))
    if setposes:
        parts.append("BYSETPOS=" + ",".join(map(str, setposes)))
    if freq == "WEEKLY" and not setposes and draw(st.booleans()):
        parts.append(f"WKST={draw(st.sampled_from(WEEKDAYS))}")

    terminator = draw(st.sampled_from(["count", "until", "none"]))
    if terminator == "count":
        parts.append(f"COUNT={draw(st.integers(1, 8))}")
    elif terminator == "until":
        span = {"SECONDLY": 7_200, "MINUTELY": 3 * 86_400, "HOURLY": 60 * 86_400}.get(
            freq, 1_500 * 86_400
        )
        until = dtstart + dt.timedelta(seconds=draw(st.integers(0, span)))
        parts.append("UNTIL=" + until.strftime("%Y%m%dT%H%M%S"))

    return ";".join(parts), dtstart


@given(rules_with_start())
@settings(deadline=None)
def test_expansion_matches_dateutil(rule_and_start):
    rule, dtstart = rule_and_start
    ours = calcard.expand_rrule(rule, dtstart, limit=N_INSTANCES)
    theirs = list(islice(rrulestr(rule, dtstart=dtstart), N_INSTANCES))
    assert ours == theirs, f"rule={rule} dtstart={dtstart}"


def test_yearly_bymonthday_pinning_deviation():
    """Documented deviation: YEARLY + BYMONTHDAY with no BYMONTH.

    calcard pins DTSTART's month, agreeing with libical/ical.js (see the
    libical fixture case FREQ=YEARLY;BYMONTHDAY=29 from DTSTART 20240229,
    which expects only Feb 29ths); dateutil recurs in every month. Both
    behaviours are asserted so a change in either engine is noticed.
    """
    rule = "FREQ=YEARLY;BYMONTHDAY=10;COUNT=3"
    dtstart = dt.datetime(2026, 1, 15, 9, 0)
    assert calcard.expand_rrule(rule, dtstart) == [
        dt.datetime(2027, 1, 10, 9, 0),
        dt.datetime(2028, 1, 10, 9, 0),
        dt.datetime(2029, 1, 10, 9, 0),
    ]
    assert list(rrulestr(rule, dtstart=dtstart)) == [
        dt.datetime(2026, 2, 10, 9, 0),
        dt.datetime(2026, 3, 10, 9, 0),
        dt.datetime(2026, 4, 10, 9, 0),
    ]
