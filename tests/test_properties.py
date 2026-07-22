"""Hypothesis property tests for the Python layer.

These mirror the hegel properties on the Rust core, exercised through the
bindings: totality of lenient parsing, the zero-repairs-iff-strict
invariant, serialization faithfulness, and escaping inverses; plus
round-trip properties of the typed datetime setters.
"""

import datetime as dt
from zoneinfo import ZoneInfo

from hypothesis import assume, given, settings, strategies as st

import calcard

# ---------------------------------------------------------------------------
# Input strategies

structural_lines = st.sampled_from(
    [
        "BEGIN:VCARD",
        "END:VCARD",
        "BEGIN:VCALENDAR",
        "END:VCALENDAR",
        "VERSION:2.0",
        'SUMMARY;TZID="a,b";X=1,2:hello\\n world',
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
    ).map(lambda endings: "".join(line + e for line, e in zip(lines, endings)))
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
                [p.name]
                + [v for param in p.params for v in [param.name, *param.values]]
            )
            if "QUOTED-PRINTABLE" in prefix.upper():
                return True
    return False


# ---------------------------------------------------------------------------
# Properties


@given(any_input)
@settings(max_examples=300)
def test_lenient_parse_is_total(text):
    calcard.parse(text)  # must not raise


@given(any_input)
@settings(max_examples=300)
def test_zero_repairs_iff_strict(text):
    doc = calcard.parse(text)
    strict_doc = None
    try:
        strict_doc = calcard.parse(text, strict=True)
    except calcard.ParseError:
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
    first = calcard.parse(text)
    if _qp_hazard(first.components):
        return
    wire = first.serialize()
    second = calcard.parse(wire)
    assert len(second.components) == len(first.components)
    for a, b in zip(second.components, first.components):
        assert a == b


@given(any_input)
@settings(max_examples=200)
def test_serialized_lines_respect_fold_width(text):
    wire = calcard.parse(text).serialize()
    for line in wire.split("\r\n"):
        assert len(line.encode()) <= 75


@given(st.text().map(lambda s: s.replace("\r", "")))
def test_text_escape_round_trip(s):
    escaped = calcard.escape_text(s)
    assert "\n" not in escaped and "\r" not in escaped
    assert calcard.unescape_text(escaped) == s


@given(st.text())
def test_unescape_is_total(s):
    calcard.unescape_text(s)  # must not raise


@given(st.lists(st.text(max_size=30).map(lambda s: s.replace("\r", "")), min_size=1))
def test_split_unescaped_inverts_escaped_join(pieces):
    escaped = [calcard.escape_text(p) for p in pieces]
    joined = ",".join(escaped)
    assert calcard.split_unescaped(joined, ",") == escaped


@given(st.binary(max_size=300))
def test_bytes_input_is_total(data):
    calcard.parse(data)  # BOM/UTF-8/Latin-1 handling must never raise


# ---------------------------------------------------------------------------
# Typed datetime setters

_EVENT_SHELL = (
    "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
)

_naive_datetimes = st.datetimes(
    min_value=dt.datetime(1900, 1, 1),
    max_value=dt.datetime(2100, 12, 31, 23, 59, 59),
).map(lambda d: d.replace(microsecond=0, fold=0))

_fixed_offsets = st.integers(min_value=-14 * 60, max_value=14 * 60).map(
    lambda minutes: dt.timezone(dt.timedelta(minutes=minutes))
)

_named_zones = st.sampled_from(
    [
        "Europe/London",
        "America/New_York",
        "Asia/Kolkata",
        "Pacific/Auckland",
        "Australia/Lord_Howe",
    ]
).map(ZoneInfo)

_tzinfos = st.one_of(st.none(), st.just(dt.timezone.utc), _fixed_offsets, _named_zones)


@st.composite
def _wire_representable_datetimes(draw):
    """Naive / UTC / ZoneInfo / fixed-offset datetimes whose instant the
    wire format can represent (it carries wall time plus zone or offset,
    so ambiguous and imaginary local times of named zones are skipped)."""
    naive = draw(_naive_datetimes)
    tz = draw(_tzinfos)
    if tz is None:
        return naive
    value = naive.replace(tzinfo=tz)
    assume(value.utcoffset() == value.replace(fold=1).utcoffset())
    round_trip = value.astimezone(dt.timezone.utc).astimezone(tz)
    assume(round_trip.replace(tzinfo=None) == naive)
    return value


@given(_wire_representable_datetimes())
@settings(max_examples=200)
def test_typed_datetime_setter_preserves_the_moment(value):
    doc = calcard.parse(_EVENT_SHELL)
    event = doc.calendars[0].events[0]
    event.start = value
    reparsed = calcard.parse(doc.serialize(), strict=True)
    for got in (event.start, reparsed.calendars[0].events[0].start):
        if value.tzinfo is None:
            # Naive stays floating with the same wall time.
            assert got == value
            assert got.tzinfo is None
        else:
            # Aware never silently changes the instant.
            assert got.tzinfo is not None
            assert got == value


_extra_params = st.lists(
    st.tuples(
        st.sampled_from(["X-FOO", "X-BAR", "X-RELATED", "LANGUAGE", "X-APPLE-TRAVEL"]),
        st.lists(
            st.text(
                alphabet="abcdefghijklmnopqrstuvwxyz0123456789-",
                min_size=1,
                max_size=8,
            ),
            min_size=1,
            max_size=3,
        ),
    ),
    max_size=4,
    unique_by=lambda item: item[0],
)


@given(_extra_params, _wire_representable_datetimes())
@settings(max_examples=200)
def test_datetime_setter_never_loses_unmanaged_params(extra, value):
    doc = calcard.parse(_EVENT_SHELL)
    event = doc.calendars[0].events[0]
    event.component.children = event.component.children + [
        calcard.Property(
            "DTEND",
            "20260722T160000Z",
            params=[calcard.Param(name, values) for name, values in extra],
        )
    ]
    event.end = value
    prop = event.component.prop("DTEND")
    assert {p.name: tuple(p.values) for p in prop.params if p.name in dict(extra)} == {
        name: tuple(values) for name, values in extra
    }
    # The managed parameters appear at most once each.
    names = [p.name.upper() for p in prop.params]
    assert names.count("TZID") <= 1
    assert names.count("VALUE") <= 1
