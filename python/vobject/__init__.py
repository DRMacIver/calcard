"""vobject: robust iCalendar and vCard parsing and serialization.

Backed by a Rust core (``vobject._core``). The document model is lossless:
parsing and re-serializing preserves property order, unknown properties and
parameters, vCard groups, and interleaving of properties with subcomponents.

Parsing is lenient by default: real-world breakage is recovered from, and
every recovery is reported as a :class:`Repair` on the returned
:class:`Document`. Pass ``strict=True`` to reject anything outside the RFC
grammars instead.

Basic usage::

    import vobject

    doc = vobject.parse(text)
    for cal in doc.components:
        for event in cal.comps("VEVENT"):
            print(event.prop("SUMMARY").value)
    out = doc.serialize()
"""

from __future__ import annotations

from dataclasses import dataclass, field

from vobject._core import (
    Component,
    Param,
    ParseError,
    Property,
    Repair,
    escape_text,
    split_unescaped,
    unescape_text,
)
from vobject._core import parse as _core_parse
from vobject._core import serialize as _core_serialize
from vobject._core import expand_rrule as _expand_rrule
from vobject._core import to_jcal_json as _to_jcal_json
from vobject.values import native_value
from vobject.typed import (
    Alarm,
    Calendar,
    Card,
    Event,
    FreeBusy,
    Journal,
    Timezone,
    Todo,
    TypedComponent,
    wrap,
)

__all__ = [
    "Alarm",
    "Calendar",
    "Card",
    "Component",
    "Document",
    "Event",
    "FreeBusy",
    "Journal",
    "Param",
    "ParseError",
    "Property",
    "Repair",
    "Timezone",
    "Todo",
    "TypedComponent",
    "escape_text",
    "expand_rrule",
    "native_value",
    "parse",
    "parse_one",
    "serialize",
    "split_unescaped",
    "to_jcal",
    "unescape_text",
    "wrap",
]

__version__ = "0.1.0"

DEFAULT_MAX_DEPTH = 512

# --------------------------------------------------------------------------
# py-vobject compatibility surface.
#
# The modules vobject.base, vobject.behavior, vobject.icalendar,
# vobject.vcard, vobject.hcalendar, vobject.change_tz and vobject.ics_diff
# provide a py-vobject 1.0-compatible API (their test suite runs against
# this package; see tests_upstream/). They are loaded lazily so that the
# clean API above has no hard dependency on python-dateutil/pytz.

_PYVOBJECT_NAMES = ("readComponents", "readOne", "newFromBehavior", "VERSION")


def _load_compat():
    """Import the compat modules together: importing vobject.icalendar and
    vobject.vcard registers their behaviors, which base.readComponents and
    newFromBehavior rely on."""
    import importlib

    base = importlib.import_module("vobject.base")
    importlib.import_module("vobject.icalendar")
    importlib.import_module("vobject.vcard")
    return base


_COMPAT_MODULES = (
    "base",
    "behavior",
    "icalendar",
    "vcard",
    "hcalendar",
    "change_tz",
    "ics_diff",
)


def __getattr__(name):
    # `from . import x` inside the compat modules consults this hook before
    # falling back to a submodule import, so it must be reentrancy-safe:
    # return partially-initialized modules from sys.modules as-is.
    if name in _COMPAT_MODULES:
        import importlib
        import sys

        mod = sys.modules.get(f"vobject.{name}")
        if mod is not None:
            return mod
        return importlib.import_module(f"vobject.{name}")
    if name in _PYVOBJECT_NAMES:
        return getattr(_load_compat(), name)
    raise AttributeError(f"module 'vobject' has no attribute {name!r}")


def iCalendar():
    """py-vobject compatibility: a new VCALENDAR 2.0 component."""
    return _load_compat().newFromBehavior("vcalendar", "2.0")


def vCard():
    """py-vobject compatibility: a new VCARD 3.0 component."""
    return _load_compat().newFromBehavior("vcard", "3.0")


@dataclass
class Document:
    """A parsed vobject stream: top-level components plus any repairs the
    lenient parser had to make. ``repairs`` is empty exactly when the input
    was strictly conformant."""

    components: list[Component] = field(default_factory=list)
    repairs: list[Repair] = field(default_factory=list)

    def serialize(self, *, line_ending: str = "\r\n", fold_width: int | None = 75) -> str:
        return _core_serialize(
            self.components, line_ending=line_ending, fold_width=fold_width
        )

    def __iter__(self):
        return iter(self.components)

    def __len__(self) -> int:
        return len(self.components)

    @property
    def calendars(self) -> list:
        """Typed views of the top-level VCALENDAR components."""
        from vobject.typed import Calendar

        return [Calendar(c) for c in self.components if c.name.upper() == "VCALENDAR"]

    @property
    def cards(self) -> list:
        """Typed views of the top-level VCARD components."""
        from vobject.typed import Card

        return [Card(c) for c in self.components if c.name.upper() == "VCARD"]


def _decode(data: bytes) -> str:
    """Decode input bytes, tolerating a UTF-8 BOM and falling back to
    Latin-1 (which cannot fail) for non-UTF-8 legacy data."""
    if data.startswith(b"\xef\xbb\xbf"):
        data = data[3:]
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("latin-1")


def parse(
    source: str | bytes,
    *,
    strict: bool = False,
    max_depth: int = DEFAULT_MAX_DEPTH,
) -> Document:
    """Parse a vobject document (an iCalendar file, a vCard, or a stream of
    several top-level components)."""
    if isinstance(source, bytes):
        source = _decode(source)
    elif source.startswith("﻿"):
        source = source[1:]
    components, repairs = _core_parse(source, strict=strict, max_depth=max_depth)
    return Document(components=components, repairs=repairs)


def parse_one(
    source: str | bytes,
    *,
    strict: bool = False,
    max_depth: int = DEFAULT_MAX_DEPTH,
) -> Component:
    """Parse input that must contain exactly one top-level component, and
    return it."""
    doc = parse(source, strict=strict, max_depth=max_depth)
    if len(doc.components) != 1:
        raise ParseError(
            f"expected exactly one top-level component, found {len(doc.components)}"
        )
    return doc.components[0]


def expand_rrule(rule: str, dtstart, *, limit: int = 1000) -> list:
    """Expand a recurrence rule from a start date or datetime.

    ``dtstart`` may be a :class:`datetime.date`, :class:`datetime.datetime`
    (naive or UTC), or a wire-format string (``20260722T160000``). Returns
    dates or naive/UTC datetimes matching the start's form, up to ``limit``
    instances.
    """
    import datetime as _dt

    if isinstance(dtstart, _dt.datetime):
        z = "Z" if dtstart.tzinfo is not None else ""
        start = dtstart.strftime("%Y%m%dT%H%M%S") + z
    elif isinstance(dtstart, _dt.date):
        start = dtstart.strftime("%Y%m%d")
    else:
        start = dtstart

    out = []
    for t in _expand_rrule(rule, start, limit=limit):
        if len(t) == 3:
            out.append(_dt.date(*t))
        else:
            y, mo, d, h, mi, s, utc = t
            out.append(
                _dt.datetime(
                    y, mo, d, h, mi, min(s, 59),
                    tzinfo=_dt.timezone.utc if utc else None,
                )
            )
    return out


def to_jcal(component: Component, *, dialect: str | None = None):
    """The jCal (RFC 7265) / jCard (RFC 7095) representation of a
    component, as Python data structures."""
    import json

    return json.loads(_to_jcal_json(component, dialect))


def serialize(
    value: Document | Component | list[Component],
    *,
    line_ending: str = "\r\n",
    fold_width: int | None = 75,
) -> str:
    """Serialize a Document, a single Component, or a list of Components."""
    if isinstance(value, Document):
        components = value.components
    elif isinstance(value, Component):
        components = [value]
    else:
        components = list(value)
    return _core_serialize(components, line_ending=line_ending, fold_width=fold_width)
