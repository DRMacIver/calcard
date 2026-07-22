"""Typed views over parsed components: the ergonomic face of the clean API.

A typed view wraps a :class:`vobject.Component` without copying it —
mutations through the view are mutations of the underlying model, and
serializing the document reflects them. Views never hide the generic
model: ``.component`` is always the wrapped object, and unknown properties
remain reachable through it.
"""

from __future__ import annotations

import datetime as _dt

from calcard._core import Component, Param, Property
from calcard.values import native_value

__all__ = [
    "Alarm",
    "Calendar",
    "Card",
    "Event",
    "FreeBusy",
    "Journal",
    "Timezone",
    "Todo",
    "TypedComponent",
    "wrap",
]


def _wire_datetime(value: _dt.date | _dt.datetime) -> tuple[str, list[Param]]:
    """Serialize a date/datetime to wire text plus the parameters it needs.

    Naive datetimes become floating local time; a zero UTC offset becomes
    the ``Z`` form; a tzinfo with a zone name (zoneinfo ``.key`` or
    pytz-style ``.zone``) becomes local time with a TZID parameter; any
    other aware datetime is converted to UTC and written with ``Z`` so the
    instant is never silently changed.
    """
    if isinstance(value, _dt.datetime):
        if value.tzinfo is None:
            return value.strftime("%Y%m%dT%H%M%S"), []
        if value.utcoffset() == _dt.timedelta(0):
            return value.strftime("%Y%m%dT%H%M%S") + "Z", []
        key = getattr(value.tzinfo, "key", None) or getattr(
            value.tzinfo, "zone", None
        )
        if key:
            return value.strftime("%Y%m%dT%H%M%S"), [Param("TZID", [key])]
        utc = value.astimezone(_dt.timezone.utc)
        return utc.strftime("%Y%m%dT%H%M%S") + "Z", []
    return value.strftime("%Y%m%d"), [Param("VALUE", ["DATE"])]


class TypedComponent:
    """Base class for typed component views."""

    NAME: str = ""
    DIALECT = "icalendar"

    def __init__(self, component: Component):
        if self.NAME and not component.name.upper() == self.NAME:
            raise ValueError(
                f"expected a {self.NAME} component, got {component.name}"
            )
        self.component = component

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.component!r})"

    def __eq__(self, other) -> bool:
        return type(self) is type(other) and self.component == other.component

    # -- generic access ----------------------------------------------------

    def text(self, name: str, default=None):
        """The native value of the first property with this name, or
        ``default`` if the property is absent.

        For TEXT-typed properties this is the unescaped string; other
        value types follow :func:`vobject.values.native_value`, so e.g.
        CATEGORIES yields a list of strings and date-valued names yield
        dates or datetimes.
        """
        prop = self.component.prop(name)
        if prop is None:
            return default
        value = native_value(prop, self.DIALECT)
        return value

    def set_text(self, name: str, value: str) -> None:
        """Set (replacing the first, or appending) a TEXT property."""
        from calcard._core import escape_text

        prop = self.component.prop(name)
        if prop is None:
            self.component.children = self.component.children + [
                Property(name, escape_text(value))
            ]
        else:
            prop.value = escape_text(value)
            prop.params = [p for p in prop.params if p.name.upper() != "VALUE"]

    def _first_native(self, name: str):
        prop = self.component.prop(name)
        if prop is None:
            return None
        value = native_value(prop, self.DIALECT)
        return value[0] if isinstance(value, list) and value else value

    def _set_datetime(self, name: str, value: _dt.date | _dt.datetime) -> None:
        text, params = _wire_datetime(value)
        prop = self.component.prop(name)
        if prop is None:
            prop = Property(name, text, params=params)
            self.component.children = self.component.children + [prop]
        else:
            prop.value = text
            # Update only the parameters this setter manages (TZID, VALUE);
            # everything else on the property is preserved.
            managed = {"TZID", "VALUE"}
            prop.params = [
                p for p in prop.params if p.name.upper() not in managed
            ] + params

    # -- shared iCalendar accessors -----------------------------------------

    @property
    def uid(self) -> str | None:
        return self.text("UID")

    @uid.setter
    def uid(self, value: str) -> None:
        self.set_text("UID", value)


class _StartEndMixin(TypedComponent):
    _END_NAME = "DTEND"

    @property
    def start(self) -> _dt.date | _dt.datetime | None:
        return self._first_native("DTSTART")

    @start.setter
    def start(self, value: _dt.date | _dt.datetime) -> None:
        self._set_datetime("DTSTART", value)

    @property
    def duration(self) -> _dt.timedelta | None:
        explicit = self._first_native("DURATION")
        if explicit is not None:
            return explicit
        start, end = self.start, self.end
        if start is None or end is None:
            return None
        if isinstance(start, _dt.datetime) != isinstance(end, _dt.datetime):
            return None
        return end - start

    @property
    def end(self) -> _dt.date | _dt.datetime | None:
        explicit = self._first_native(self._END_NAME)
        if explicit is not None:
            return explicit
        start = self.start
        explicit_duration = self._first_native("DURATION")
        if start is not None and explicit_duration is not None:
            return start + explicit_duration
        if start is None:
            return None
        # RFC 5545 fallbacks: a date-valued start covers one day; a
        # datetime-valued start covers zero duration.
        if isinstance(start, _dt.datetime):
            return start
        return start + _dt.timedelta(days=1)

    @end.setter
    def end(self, value: _dt.date | _dt.datetime) -> None:
        self._set_datetime(self._END_NAME, value)

    status = property(
        lambda self: self.text("STATUS"),
        lambda self, v: self.set_text("STATUS", v),
    )

    @property
    def rrule(self) -> str | None:
        prop = self.component.prop("RRULE")
        return prop.value if prop is not None else None

    @rrule.setter
    def rrule(self, value: str) -> None:
        from calcard._core import typed_value

        # Validate before storing: an invalid rule raises ParseError and
        # leaves the component untouched.
        typed_value(Property("RRULE", value), self.DIALECT)
        prop = self.component.prop("RRULE")
        if prop is None:
            self.component.children = self.component.children + [
                Property("RRULE", value)
            ]
        else:
            prop.value = value

    def occurrences(self, *, limit: int = 1000) -> list:
        """Expand this component's RRULE from its start (start alone if
        there is no rule), with RFC 5545 local-time DST semantics for
        zone-aware starts. EXDATE/RDATE handling is the caller's concern."""
        from calcard import expand_rrule

        start = self.start
        if start is None:
            return []
        rule = self.rrule
        if rule is None:
            return [start]
        return expand_rrule(rule, start, limit=limit)


class Event(_StartEndMixin):
    NAME = "VEVENT"

    summary = property(
        lambda self: self.text("SUMMARY"),
        lambda self, v: self.set_text("SUMMARY", v),
    )
    description = property(
        lambda self: self.text("DESCRIPTION"),
        lambda self, v: self.set_text("DESCRIPTION", v),
    )
    location = property(
        lambda self: self.text("LOCATION"),
        lambda self, v: self.set_text("LOCATION", v),
    )
    @property
    def alarms(self) -> list[Alarm]:
        return [Alarm(c) for c in self.component.comps("VALARM")]


class Todo(_StartEndMixin):
    NAME = "VTODO"
    _END_NAME = "DUE"

    summary = property(
        lambda self: self.text("SUMMARY"),
        lambda self, v: self.set_text("SUMMARY", v),
    )

    @property
    def due(self) -> _dt.date | _dt.datetime | None:
        return self.end

    @due.setter
    def due(self, value) -> None:
        self.end = value


class Journal(_StartEndMixin):
    NAME = "VJOURNAL"

    summary = property(
        lambda self: self.text("SUMMARY"),
        lambda self, v: self.set_text("SUMMARY", v),
    )


class FreeBusy(TypedComponent):
    NAME = "VFREEBUSY"


class Alarm(TypedComponent):
    NAME = "VALARM"

    action = property(lambda self: self.text("ACTION"))

    @property
    def trigger(self):
        return self._first_native("TRIGGER")


class Timezone(TypedComponent):
    NAME = "VTIMEZONE"

    @property
    def tzid(self) -> str | None:
        return self.text("TZID")


class Calendar(TypedComponent):
    NAME = "VCALENDAR"

    prodid = property(lambda self: self.text("PRODID"))
    version = property(lambda self: self.text("VERSION"))
    method = property(lambda self: self.text("METHOD"))

    @property
    def events(self) -> list[Event]:
        return [Event(c) for c in self.component.comps("VEVENT")]

    @property
    def todos(self) -> list[Todo]:
        return [Todo(c) for c in self.component.comps("VTODO")]

    @property
    def journals(self) -> list[Journal]:
        return [Journal(c) for c in self.component.comps("VJOURNAL")]

    @property
    def timezones(self) -> list[Timezone]:
        return [Timezone(c) for c in self.component.comps("VTIMEZONE")]


class Card(TypedComponent):
    NAME = "VCARD"

    @property
    def DIALECT(self) -> str:  # type: ignore[override]
        version = self.component.prop("VERSION")
        if version is not None and version.value.strip() in ("2.1", "3.0"):
            return "vcard3"
        return "vcard4"

    fn = property(
        lambda self: self.text("FN"),
        lambda self, v: self.set_text("FN", v),
    )
    version = property(lambda self: self.text("VERSION"))

    @property
    def n(self) -> list[list[str]] | None:
        prop = self.component.prop("N")
        return native_value(prop, self.DIALECT) if prop is not None else None

    @property
    def emails(self) -> list[str]:
        return [
            native_value(p, self.DIALECT) for p in self.component.props("EMAIL")
        ]

    @property
    def tels(self) -> list[str]:
        return [
            native_value(p, self.DIALECT) for p in self.component.props("TEL")
        ]


_BY_NAME = {
    cls.NAME: cls
    for cls in (Calendar, Event, Todo, Journal, FreeBusy, Alarm, Timezone, Card)
}


def wrap(component: Component) -> TypedComponent:
    """The most specific typed view for a component; a plain
    :class:`TypedComponent` for anything unrecognized."""
    cls = _BY_NAME.get(component.name.upper())
    if cls is None:
        view = TypedComponent(component)
        return view
    return cls(component)
