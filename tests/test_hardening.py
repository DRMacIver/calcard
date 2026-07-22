"""Regression tests for panics/aborts reachable through the Python API and
for the byte-decoding repair semantics."""

import pytest

import calcard
from calcard import Component, ParseError


def test_huge_rrule_interval_does_not_panic():
    from datetime import datetime

    # Used to panic the Rust core (PanicException escaping to Python).
    out = calcard.expand_rrule(
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
        calcard.serialize([root])


def test_cyclic_component_tree_raises_instead_of_aborting():
    a = Component("VCALENDAR")
    b = Component("VEVENT")
    a.children = [b]
    b.children = [a]
    with pytest.raises(ValueError, match="depth"):
        calcard.serialize([a])


def test_deep_component_equality_raises_instead_of_aborting():
    def deep_tree():
        root = Component("VCALENDAR")
        tip = root
        for _ in range(100_000):
            child = Component("VEVENT")
            tip.children = [*tip.children, child]
            tip = child
        return root

    with pytest.raises(ValueError, match="depth"):
        deep_tree() == deep_tree()


def test_cyclic_component_equality_raises_instead_of_aborting():
    a = Component("VCALENDAR")
    a.children = [a]
    b = Component("VCALENDAR")
    b.children = [b]
    with pytest.raises(ValueError, match="depth"):
        a == b
    with pytest.raises(ValueError, match="depth"):
        a == a


def test_max_depth_above_ceiling_is_rejected():
    # Depths beyond the default would let a parse build trees that the
    # recursive conversion, comparison, and serialization paths cannot
    # safely process (C stack overflow), so raising the cap is an error.
    with pytest.raises(ValueError, match="max_depth"):
        calcard.parse("BEGIN:VCARD\r\nEND:VCARD\r\n", max_depth=513)
    with pytest.raises(ValueError, match="max_depth"):
        calcard.parse(b"BEGIN:VCARD\r\nEND:VCARD\r\n", max_depth=100_000)


def test_max_depth_can_be_lowered():
    nested = "BEGIN:A\r\n" * 20 + "END:A\r\n" * 20
    assert calcard.parse(nested, max_depth=512).components
    with pytest.raises(ParseError):
        calcard.parse(nested, strict=True, max_depth=10)


def test_parse_error_line_attribute_is_always_present():
    with pytest.raises(ParseError) as excinfo:
        calcard.parse("BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n", strict=True)
    assert excinfo.value.line == 1
    # Errors raised away from any source line still expose the attribute.
    with pytest.raises(ParseError) as excinfo:
        calcard.parse_one("")
    assert excinfo.value.line is None


def test_non_utf8_bytes_record_repair_and_preserve_data():
    doc = calcard.parse(b"BEGIN:VCARD\r\nFN:Caf\xe9\r\nEND:VCARD\r\n")
    assert any("UTF-8" in r.message for r in doc.repairs)
    assert doc.components[0].prop("FN").value == "Café"


def test_non_utf8_bytes_rejected_in_strict_mode():
    with pytest.raises(ParseError):
        calcard.parse(b"BEGIN:VCARD\r\nFN:Caf\xe9\r\nEND:VCARD\r\n", strict=True)


def test_utf8_bytes_with_bom_are_clean():
    doc = calcard.parse(b"\xef\xbb\xbfBEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\n")
    assert doc.repairs == []
    assert doc.components[0].name == "VCARD"


def test_strict_rejects_malformed_delimiters():
    with pytest.raises(ParseError):
        calcard.parse("BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n", strict=True)
    doc = calcard.parse("BEGIN: VCALENDAR\r\nEND:VCALENDAR\r\n")
    assert doc.repairs, "lenient parse must record the normalization"
    assert doc.components[0].name == "VCALENDAR"
