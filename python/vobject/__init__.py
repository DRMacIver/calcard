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

__all__ = [
    "Component",
    "Document",
    "Param",
    "ParseError",
    "Property",
    "Repair",
    "escape_text",
    "parse",
    "parse_one",
    "serialize",
    "split_unescaped",
    "unescape_text",
]

__version__ = "0.1.0"

DEFAULT_MAX_DEPTH = 512


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
