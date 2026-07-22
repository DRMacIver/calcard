"""Regression tests for panics/aborts reachable through the Python API and
for the byte-decoding repair semantics."""

import pytest

import vobject
from vobject import Component, ParseError


def test_huge_rrule_interval_does_not_panic():
    from datetime import datetime

    # Used to panic the Rust core (PanicException escaping to Python).
    out = vobject.expand_rrule(
        "FREQ=DAILY;INTERVAL=10000000;COUNT=5", datetime(2026, 1, 1, 10, 0)
    )
    assert len(out) == 1


def test_deep_component_tree_raises_instead_of_aborting():
    root = Component("VCALENDAR")
    tip = root
    for _ in range(100_000):
        child = Component("VEVENT")
        tip.children = [*tip.children, child]
        tip = child
    with pytest.raises(ValueError, match="depth"):
        vobject.serialize([root])


def test_cyclic_component_tree_raises_instead_of_aborting():
    a = Component("VCALENDAR")
    b = Component("VEVENT")
    a.children = [b]
    b.children = [a]
    with pytest.raises(ValueError, match="depth"):
        vobject.serialize([a])


def test_non_utf8_bytes_record_repair_and_preserve_data():
    doc = vobject.parse(b"BEGIN:VCARD\r\nFN:Caf\xe9\r\nEND:VCARD\r\n")
    assert any("UTF-8" in r.message for r in doc.repairs)
    assert doc.components[0].prop("FN").value == "Café"


def test_non_utf8_bytes_rejected_in_strict_mode():
    with pytest.raises(ParseError):
        vobject.parse(b"BEGIN:VCARD\r\nFN:Caf\xe9\r\nEND:VCARD\r\n", strict=True)


def test_utf8_bytes_with_bom_are_clean():
    doc = vobject.parse(b"\xef\xbb\xbfBEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\n")
    assert doc.repairs == []
    assert doc.components[0].name == "VCARD"


def test_strict_rejects_malformed_delimiters():
    with pytest.raises(ParseError):
        vobject.parse("BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n", strict=True)
    doc = vobject.parse("BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n")
    assert doc.repairs, "lenient parse must record the normalization"
    assert doc.components[0].name == "VCALENDAR"
