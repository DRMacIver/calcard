"""Timezone resolution from in-document VTIMEZONE components.

RFC 5545 requires every ``TZID`` parameter to reference a ``VTIMEZONE``
component in the same document, which makes conformant documents
self-describing: no external timezone database is needed to interpret
them. This module builds real :class:`datetime.tzinfo` objects from
those components by expanding each STANDARD/DAYLIGHT observance's onset
rule (through the core RRULE engine) into a transition table.

Resolution precedence lives in :mod:`calcard.values`: the host's
zoneinfo wins for names it knows — real-world in-document VTIMEZONE
copies are frequently stale — and the in-document definition covers
everything else (Outlook-style names, custom TZIDs).
"""

from __future__ import annotations

import datetime as _dt
import re
import warnings
from bisect import bisect_right

from calcard._core import Component, Property
from calcard._core import expand_rrule as _expand_rrule
from calcard._core import typed_value as _typed_value


class TimezoneResolutionWarning(UserWarning):
    """A TZID could not be resolved through zoneinfo or a VTIMEZONE."""


_ZERO = _dt.timedelta(0)


def _first_typed(component, name: str):
    prop = component.prop(name)
    if prop is None:
        return None
    kind, payload = _typed_value(prop)
    if kind in ("text", "integer", "float") and isinstance(payload, list):
        return payload[0] if payload else None
    if kind in ("date", "datetime", "time") and payload:
        return payload[0]
    return payload


def _rewrite_utc_until(rule: str, offset_from: _dt.timedelta) -> str:
    """VTIMEZONE onset rules give UNTIL in UTC; the onset expansion runs
    in the observance's local (OFFSETFROM) wall clock, so shift it."""

    def repl(match):
        until = _dt.datetime.strptime(match.group(1), "%Y%m%dT%H%M%S")
        return "UNTIL=" + (until + offset_from).strftime("%Y%m%dT%H%M%S")

    return re.sub(r"UNTIL=(\d{8}T\d{6})Z", repl, rule, flags=re.IGNORECASE)


class _Observance:
    def __init__(self, component, is_daylight: bool):
        start = _first_typed(component, "DTSTART")
        if start is None or len(start) < 6:
            raise ValueError("VTIMEZONE observance without a DTSTART date-time")
        self.dtstart_wall = _dt.datetime(*start[:6])
        offset_from = _first_typed(component, "TZOFFSETFROM")
        offset_to = _first_typed(component, "TZOFFSETTO")
        if offset_from is None or offset_to is None:
            raise ValueError("VTIMEZONE observance without TZOFFSETFROM/TZOFFSETTO")
        # The core's utc-offset payload is signed seconds.
        self.offset_from = _dt.timedelta(seconds=offset_from)
        self.offset_to = _dt.timedelta(seconds=offset_to)
        self.dst = (
            max(self.offset_to - self.offset_from, _ZERO) if is_daylight else _ZERO
        )
        name = _first_typed(component, "TZNAME")
        self.name = name if isinstance(name, str) else None

        rrule_prop = component.prop("RRULE")
        self.rule = rrule_prop.value if rrule_prop is not None else None
        self.rdates_wall: list[_dt.datetime] = []
        for prop in component.props("RDATE"):
            kind, payload = _typed_value(prop)
            if kind == "datetime":
                for t in payload:
                    if len(t) >= 6:
                        self.rdates_wall.append(_dt.datetime(*t[:6]))

    def onsets_through(self, year: int) -> list[_dt.datetime]:
        """All onset wall times (in the OFFSETFROM frame) up to the end of
        ``year``."""
        onsets = {self.dtstart_wall}
        onsets.update(self.rdates_wall)
        if self.rule is not None:
            rule = self.rule
            upper = rule.upper()
            if "COUNT=" not in upper and "UNTIL=" not in upper:
                rule = f"{rule};UNTIL={year}1231T235959"
            else:
                rule = _rewrite_utc_until(rule, self.offset_from)
            span_years = max(year - self.dtstart_wall.year + 2, 2)
            for t in _expand_rrule(
                rule,
                self.dtstart_wall.strftime("%Y%m%dT%H%M%S"),
                limit=span_years * 60,
            ):
                if len(t) >= 6:
                    onsets.add(_dt.datetime(*t[:6]))
        return sorted(o for o in onsets if o.year <= year)


class VTimezone(_dt.tzinfo):
    """A tzinfo backed by a document's VTIMEZONE component.

    Transition tables are built lazily out to the latest year queried
    (plus slack), so unbounded onset rules keep working arbitrarily far
    into the future. ``fold`` follows PEP 495: for ambiguous wall times
    fold=0 selects the earlier offset and fold=1 the later; in gaps
    fold=0 maps with the pre-transition offset.
    """

    def __init__(self, tzid: str, observances: list[_Observance]):
        if not observances:
            raise ValueError("VTIMEZONE without STANDARD or DAYLIGHT observances")
        self._tzid = tzid
        self._observances = observances
        self._horizon = 0
        self._ensure(max(_dt.date.today().year + 20, 2050))

    @property
    def key(self) -> str:
        """The document TZID (mirrors zoneinfo's ``key`` so serialization
        emits the right TZID parameter)."""
        return self._tzid

    def _ensure(self, year: int) -> None:
        if year <= self._horizon:
            return
        self._horizon = year + 10
        transitions: list[tuple[_dt.datetime, _Observance]] = []
        for obs in self._observances:
            for wall in obs.onsets_through(self._horizon):
                transitions.append((wall - obs.offset_from, obs))
        transitions.sort(key=lambda pair: pair[0])

        first_obs = transitions[0][1]
        self._initial = (first_obs.offset_from, _ZERO, None)
        self._utc = [utc for utc, _ in transitions]
        self._after = [(obs.offset_to, obs.dst, obs.name) for _, obs in transitions]
        before = [self._initial[0]] + [entry[0] for entry in self._after[:-1]]
        self._wall_fold0 = [
            utc + max(b, a[0]) for utc, b, a in zip(self._utc, before, self._after)
        ]
        self._wall_fold1 = [
            utc + min(b, a[0]) for utc, b, a in zip(self._utc, before, self._after)
        ]
        self._before = before

    def _entry_for(self, dt: _dt.datetime | None):
        if dt is None:
            return self._initial
        self._ensure(dt.year)
        walls = self._wall_fold1 if dt.fold else self._wall_fold0
        idx = bisect_right(walls, dt.replace(tzinfo=None))
        if idx == 0:
            return self._initial
        return self._after[idx - 1]

    def utcoffset(self, dt):
        return self._entry_for(dt)[0]

    def dst(self, dt):
        return self._entry_for(dt)[1]

    def tzname(self, dt):
        name = self._entry_for(dt)[2]
        return name if name is not None else self._tzid

    def fromutc(self, dt):
        if dt.tzinfo is not self:
            raise ValueError("fromutc: dt.tzinfo is not self")
        self._ensure(dt.year + 1)
        u = dt.replace(tzinfo=None)
        idx = bisect_right(self._utc, u)
        if idx == 0:
            offset = self._initial[0]
            fold = 0
        else:
            offset = self._after[idx - 1][0]
            # Inside the ambiguity window that follows a backward
            # transition, this is the second occurrence of the wall time.
            shrink = self._before[idx - 1] - offset
            fold = 1 if shrink > _ZERO and u - self._utc[idx - 1] < shrink else 0
        return (u + offset).replace(tzinfo=self, fold=fold)

    def __repr__(self) -> str:
        return f"VTimezone({self._tzid!r})"

    def __eq__(self, other) -> bool:
        return isinstance(other, VTimezone) and other._tzid == self._tzid

    def __hash__(self) -> int:
        return hash((VTimezone, self._tzid))


def tzinfo_from_vtimezone(component) -> VTimezone:
    """Build a :class:`datetime.tzinfo` from a VTIMEZONE component."""
    if not component.name.upper() == "VTIMEZONE":
        raise ValueError(f"expected a VTIMEZONE component, got {component.name}")
    tzid_prop = component.prop("TZID")
    tzid = tzid_prop.value if tzid_prop is not None else "unknown"
    observances = [
        _Observance(comp, comp.name.upper() == "DAYLIGHT")
        for comp in component.components()
        if comp.name.upper() in ("STANDARD", "DAYLIGHT")
    ]
    return VTimezone(tzid, observances)


# -- VTIMEZONE generation (tzinfo -> component) -----------------------------


def _format_offset(delta: _dt.timedelta) -> str:
    total = round(delta.total_seconds())
    sign = "-" if total < 0 else "+"
    hours, rest = divmod(abs(total), 3600)
    minutes, seconds = divmod(rest, 60)
    if seconds:
        return f"{sign}{hours:02d}{minutes:02d}{seconds:02d}"
    return f"{sign}{hours:02d}{minutes:02d}"


def _probe(tz: _dt.tzinfo, naive_utc: _dt.datetime):
    """(utcoffset, dst, tzname) in force at a naive-UTC instant."""
    local = naive_utc.replace(tzinfo=_dt.timezone.utc).astimezone(tz)
    return (local.utcoffset(), local.dst() or _ZERO, local.tzname())


def _transitions(tz: _dt.tzinfo, start: _dt.datetime, end: _dt.datetime):
    """Offset/name transitions in the naive-UTC interval ``(start, end]``,
    as ``(instant, state_before, state_after)`` tuples, found by daily
    probing refined to the second (real zones never transition twice in
    one day)."""
    out = []
    step = _dt.timedelta(days=1)
    at = start
    before = _probe(tz, at)
    while at < end:
        upto = min(at + step, end)
        after = _probe(tz, upto)
        if after != before:
            lo, hi = at, upto
            while hi - lo > _dt.timedelta(seconds=1):
                mid = lo + _dt.timedelta(seconds=int((hi - lo).total_seconds() // 2))
                if _probe(tz, mid) == before:
                    lo = mid
                else:
                    hi = mid
            out.append((hi, before, _probe(tz, hi)))
        before = after
        at = upto
    return out


def vtimezone_from_tzinfo(
    tz: _dt.tzinfo,
    *,
    start: _dt.datetime,
    end: _dt.datetime,
    tzid: str | None = None,
) -> Component:
    """A VTIMEZONE component describing ``tz`` over ``[start, end]``.

    ``start``/``end`` are instants (naive values are taken as UTC).
    Transitions inside the window become STANDARD/DAYLIGHT observances —
    grouped by offsets and name, extra onsets as RDATEs — with onset
    wall times in the pre-transition (TZOFFSETFROM) frame as RFC 5545
    requires; a window with no transitions yields a single fixed
    STANDARD observance. No recurrence rules are inferred, so the
    component describes exactly the covered window: callers must pick a
    window spanning every datetime that will reference it (which is what
    :func:`add_missing_timezones` automates).
    """

    def utc_naive(value: _dt.datetime) -> _dt.datetime:
        if value.tzinfo is None:
            return value
        return value.astimezone(_dt.timezone.utc).replace(tzinfo=None)

    start, end = utc_naive(start), utc_naive(end)
    if end < start:
        raise ValueError("end must not precede start")
    if tzid is None:
        tzid = getattr(tz, "key", None) or getattr(tz, "zone", None) or str(tz)

    # (offset_from, offset_to, name, is_daylight) -> onset wall times.
    groups: dict[tuple, list[_dt.datetime]] = {}
    for instant, before, after in _transitions(tz, start, end):
        key = (before[0], after[0], after[2], after[1] > _ZERO)
        groups.setdefault(key, []).append(instant + before[0])

    observances = []
    if not groups:
        offset, dst, name = _probe(tz, start)
        groups[(offset, offset, name, dst > _ZERO)] = [start + offset]
    for (offset_from, offset_to, name, is_daylight), onsets in groups.items():
        children = [
            Property("DTSTART", onsets[0].strftime("%Y%m%dT%H%M%S")),
            Property("TZOFFSETFROM", _format_offset(offset_from)),
            Property("TZOFFSETTO", _format_offset(offset_to)),
        ]
        if name:
            children.append(Property("TZNAME", name))
        if len(onsets) > 1:
            children.append(
                Property(
                    "RDATE",
                    ",".join(o.strftime("%Y%m%dT%H%M%S") for o in onsets[1:]),
                )
            )
        observances.append(
            Component("DAYLIGHT" if is_daylight else "STANDARD", children)
        )
    return Component("VTIMEZONE", [Property("TZID", tzid)] + observances)


_WIRE_DATETIME_RE = re.compile(r"\d{8}T\d{6}")


def add_missing_timezones(
    calendar: Component, *, padding: _dt.timedelta = _dt.timedelta(days=366)
) -> list[Component]:
    """Insert a generated VTIMEZONE for every TZID parameter used in
    ``calendar`` but not defined by one of its VTIMEZONE children.

    TZIDs are resolved through the host's zoneinfo (an unresolvable one
    warns :class:`TimezoneResolutionWarning` and is skipped — the
    document alone must already define it for conformance). Each
    generated component covers the span of the datetimes referencing its
    zone, widened by ``padding`` on both sides so the offset in force at
    the earliest reference is always derivable. New components are
    inserted ahead of the first existing subcomponent; the inserted
    components are returned (empty when nothing was missing).
    """
    existing = set()
    for comp in calendar.comps("VTIMEZONE"):
        prop = comp.prop("TZID")
        if prop is not None:
            existing.add(prop.value)

    spans: dict[str, tuple[_dt.datetime, _dt.datetime]] = {}
    order: list[str] = []

    def visit(component: Component) -> None:
        for prop in component.properties():
            tzid = next(
                (
                    param.values[0]
                    for param in prop.params
                    if param.name.upper() == "TZID" and param.values
                ),
                None,
            )
            if tzid is None:
                continue
            for text in _WIRE_DATETIME_RE.findall(prop.value):
                # The wall time stands in for the instant here; the
                # padding dwarfs the offset error.
                wall = _dt.datetime.strptime(text, "%Y%m%dT%H%M%S")
                if tzid not in spans:
                    order.append(tzid)
                    spans[tzid] = (wall, wall)
                else:
                    lo, hi = spans[tzid]
                    spans[tzid] = (min(lo, wall), max(hi, wall))
        for child in component.components():
            if child.name.upper() != "VTIMEZONE":
                visit(child)

    visit(calendar)

    added = []
    for tzid in order:
        if tzid in existing:
            continue
        tz = None
        try:
            from zoneinfo import ZoneInfo

            tz = ZoneInfo(tzid.lstrip("/"))
        except Exception:  # noqa: BLE001 - any failure means "not a host zone"
            warnings.warn(
                f"cannot generate a VTIMEZONE for unknown TZID {tzid!r}",
                TimezoneResolutionWarning,
                stacklevel=2,
            )
            continue
        lo, hi = spans[tzid]
        added.append(
            vtimezone_from_tzinfo(tz, start=lo - padding, end=hi + padding, tzid=tzid)
        )
    if added:
        children = list(calendar.children)
        first_comp = next(
            (i for i, child in enumerate(children) if isinstance(child, Component)),
            len(children),
        )
        calendar.children = children[:first_comp] + added + children[first_comp:]
    return added


def timezone_map(calendar_component) -> dict[str, _dt.tzinfo]:
    """TZID -> tzinfo for every well-formed VTIMEZONE child of a
    VCALENDAR component (malformed ones are skipped: resolution then
    falls through to the naive-with-warning path)."""
    out: dict[str, _dt.tzinfo] = {}
    for comp in calendar_component.comps("VTIMEZONE"):
        tzid_prop = comp.prop("TZID")
        if tzid_prop is None:
            continue
        try:
            tz = tzinfo_from_vtimezone(comp)
        except Exception as e:  # noqa: BLE001 - deliberate: see below
            # A malformed VTIMEZONE must not make the whole document
            # uninterpretable; the affected TZIDs warn and fall back.
            warnings.warn(
                f"ignoring malformed VTIMEZONE {tzid_prop.value!r}: {e}",
                TimezoneResolutionWarning,
                stacklevel=2,
            )
            continue
        out.setdefault(tzid_prop.value, tz)
    return out
