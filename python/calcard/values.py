"""Conversion of typed property values to native Python objects.

The Rust core parses raw values into typed representations; this module
maps them onto datetime/date/time/timedelta/etc. Date, datetime, time,
duration, and period values are always lists (even with one element), as
are text-lists (CATEGORIES) and structured values (N, REQUEST-STATUS).
The ``text``, ``integer``, and ``float`` kinds deliberately unwrap a
single value to a bare scalar — SUMMARY gives a string and PRIORITY an
int, not one-element lists — and are lists only when the property
genuinely carries several comma-separated values.
"""

from __future__ import annotations

import datetime as _dt
from typing import Any

from calcard._core import Property
from calcard._core import typed_value as _typed_value

try:
    from zoneinfo import ZoneInfo, ZoneInfoNotFoundError
except ImportError:  # pragma: no cover - zoneinfo is stdlib on >=3.9
    ZoneInfo = None  # type: ignore[assignment]


def _tzinfo_for(
    prop: Property, timezones: dict[str, _dt.tzinfo] | None = None
) -> _dt.tzinfo | None:
    """Resolve a TZID parameter to a tzinfo.

    Precedence: the host's zoneinfo for names it knows (real-world
    in-document VTIMEZONE copies are often stale), then the document's
    own VTIMEZONE definitions (``timezones``, a TZID -> tzinfo map).
    When both fail, a :class:`TimezoneResolutionWarning` is emitted and
    ``None`` is returned: datetimes come back naive (local wall time,
    zone information dropped).
    """
    for param in prop.params:
        if param.name.upper() == "TZID" and param.values:
            raw = param.values[0]
            tzid = raw.lstrip("/")
            if ZoneInfo is not None:
                try:
                    return ZoneInfo(tzid)
                except (ZoneInfoNotFoundError, ValueError, KeyError):
                    pass
            if timezones is not None:
                tz = timezones.get(raw) or timezones.get(tzid)
                if tz is not None:
                    return tz
            import warnings

            from calcard.timezones import TimezoneResolutionWarning

            warnings.warn(
                f"TZID {raw!r} is not a known timezone and no VTIMEZONE in the "
                "document defines it; interpreting as naive local time",
                TimezoneResolutionWarning,
                stacklevel=3,
            )
            return None
    return None


def _to_date(t: tuple) -> _dt.date:
    return _dt.date(t[0], t[1], t[2])


def _to_datetime(t: tuple, tz: _dt.tzinfo | None) -> _dt.datetime | _dt.date:
    if len(t) == 3:
        return _dt.date(t[0], t[1], t[2])
    year, month, day, hour, minute, second, utc = t
    second = min(second, 59)  # Python datetime cannot carry leap seconds
    if utc:
        tzinfo: _dt.tzinfo | None = _dt.timezone.utc
    else:
        tzinfo = tz
    return _dt.datetime(year, month, day, hour, minute, second, tzinfo=tzinfo)


def _to_time(t: tuple) -> _dt.time:
    hour, minute, second, utc = t
    return _dt.time(
        hour, minute, min(second, 59), tzinfo=_dt.timezone.utc if utc else None
    )


def native_value(
    prop: Property,
    dialect: str = "icalendar",
    timezones: dict[str, _dt.tzinfo] | None = None,
) -> Any:
    """The property's value as a native Python object.

    Raises ``ParseError`` (via the core) if the raw value does not parse as
    the property's resolved type. ``timezones`` is an optional
    TZID -> tzinfo map (built from the document's VTIMEZONE components by
    :func:`calcard.timezones.timezone_map`; the typed views supply it
    automatically) consulted for TZIDs zoneinfo cannot resolve; a TZID
    neither can resolve warns and yields naive datetimes (the wall time is
    kept, the unresolvable zone is dropped).
    """
    kind, payload = _typed_value(prop, dialect)
    # Only datetime-bearing kinds consult TZID; resolving it lazily keeps
    # a stray TZID on e.g. a TEXT property from warning about resolution.
    tz = _tzinfo_for(prop, timezones) if kind in ("datetime", "period") else None

    if kind == "text":
        return payload[0] if len(payload) == 1 else payload
    if kind == "text-list":
        return payload
    if kind == "structured":
        return payload
    if kind == "date":
        return [_to_date(t) for t in payload]
    if kind == "datetime":
        return [_to_datetime(t, tz) for t in payload]
    if kind == "time":
        return [_to_time(t) for t in payload]
    if kind == "duration":
        return [_dt.timedelta(seconds=s) for s in payload]
    if kind == "period":
        out = []
        for start, end_kind, end in payload:
            start_dt = _to_datetime(start, tz)
            if end_kind == "end":
                out.append((start_dt, _to_datetime(end, tz)))
            else:
                out.append((start_dt, _dt.timedelta(seconds=end)))
        return out
    if kind == "integer":
        return payload[0] if len(payload) == 1 else payload
    if kind == "float":
        return payload[0] if len(payload) == 1 else payload
    if kind == "boolean":
        return payload
    if kind == "binary":
        return bytes(payload)
    if kind == "utc-offset":
        return _dt.timezone(_dt.timedelta(seconds=payload))
    # recur, uri, cal-address, unknown: plain strings.
    return payload
