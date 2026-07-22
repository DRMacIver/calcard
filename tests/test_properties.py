"""Hypothesis property tests for the Python layer.

These mirror the hegel properties on the Rust core, exercised through the
bindings: totality of lenient parsing, the zero-repairs-iff-strict
invariant, serialization faithfulness, and escaping inverses.
"""

from hypothesis import given, settings, strategies as st

import vobject

# ---------------------------------------------------------------------------
# Input strategies

structural_lines = st.sampled_from(
    [
        "BEGIN:VCARD",
        "END:VCARD",
        "BEGIN:VCALENDAR",
        "END:VCALENDAR",
        "VERSION:2.0",
        "SUMMARY;TZID=\"a,b\";X=1,2:hello\\n world",
        "TEL;HOME;VOICE:+441234",
        "NOTE;ENCODING=QUOTED-PRINTABLE:soft=",
        " folded continuation",
        "",
    ]
)

documentish = st.lists(
    st.one_of(structural_lines, st.text(max_size=30)), max_size=15
).flatmap(
    lambda lines: st.lists(
        st.sampled_from(["\r\n", "\n"]), min_size=len(lines), max_size=len(lines)
    ).map(lambda endings: "".join(l + e for l, e in zip(lines, endings)))
)

any_input = st.one_of(st.text(max_size=400), documentish)


def _all_properties(component):
    out = []
    stack = [component]
    while stack:
        c = stack.pop()
        out.extend(c.properties())
        stack.extend(c.components())
    return out


def _qp_hazard(components):
    """Mirrors the writer/reparse ambiguity check from the Rust tests: the
    vCard 2.1 quoted-printable soft-break heuristic can re-join lines."""
    for component in components:
        for p in _all_properties(component):
            if "=" not in p.value:
                continue
            prefix = " ".join(
                [p.name] + [v for param in p.params for v in [param.name, *param.values]]
            )
            if "QUOTED-PRINTABLE" in prefix.upper():
                return True
    return False


# ---------------------------------------------------------------------------
# Properties

@given(any_input)
@settings(max_examples=300)
def test_lenient_parse_is_total(text):
    vobject.parse(text)  # must not raise


@given(any_input)
@settings(max_examples=300)
def test_zero_repairs_iff_strict(text):
    doc = vobject.parse(text)
    strict_doc = None
    try:
        strict_doc = vobject.parse(text, strict=True)
    except vobject.ParseError:
        pass

    if strict_doc is not None:
        assert doc.repairs == []
        assert len(strict_doc.components) == len(doc.components)
        for a, b in zip(strict_doc.components, doc.components):
            assert a == b
    else:
        assert doc.repairs != []


@given(any_input)
@settings(max_examples=300)
def test_serialization_is_faithful(text):
    first = vobject.parse(text)
    if _qp_hazard(first.components):
        return
    wire = first.serialize()
    second = vobject.parse(wire)
    assert len(second.components) == len(first.components)
    for a, b in zip(second.components, first.components):
        assert a == b


@given(any_input)
@settings(max_examples=200)
def test_serialized_lines_respect_fold_width(text):
    wire = vobject.parse(text).serialize()
    for line in wire.split("\r\n"):
        assert len(line.encode()) <= 75


@given(st.text().map(lambda s: s.replace("\r", "")))
def test_text_escape_round_trip(s):
    escaped = vobject.escape_text(s)
    assert "\n" not in escaped and "\r" not in escaped
    assert vobject.unescape_text(escaped) == s


@given(st.text())
def test_unescape_is_total(s):
    vobject.unescape_text(s)  # must not raise


@given(st.lists(st.text(max_size=30).map(lambda s: s.replace("\r", "")), min_size=1))
def test_split_unescaped_inverts_escaped_join(pieces):
    escaped = [vobject.escape_text(p) for p in pieces]
    joined = ",".join(escaped)
    assert vobject.split_unescaped(joined, ",") == escaped


@given(st.binary(max_size=300))
def test_bytes_input_is_total(data):
    vobject.parse(data)  # BOM/UTF-8/Latin-1 handling must never raise
