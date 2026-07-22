# vobject

A robust, heavily-tested implementation of the vobject family of formats —
iCalendar (RFC 5545) and vCard (RFC 6350 / RFC 2426 / vCard 2.1, with
RFC 6868 parameter encoding) — as a standalone Rust crate
(`vobject-core`) with Python bindings (`vobject`).

## Design principles

- **Robustness first.** Real-world calendar and contact data is frequently
  malformed. Lenient parsing (the default) recovers from breakage — bare LF
  line endings, vCard 2.1 bare parameters and quoted-printable soft line
  breaks, unterminated components, stray quotes, control characters,
  pathological nesting — and reports every recovery as a `Repair`. Strict
  mode enforces the RFC grammars exactly. Zero repairs in lenient mode is
  the same thing as strict validity.
- **Lossless round-trips.** The document model preserves property order,
  unknown properties and parameters, vCard groups, and the interleaving of
  properties with subcomponents.
- **Tested against the ecosystem.** The conformance corpus is vendored from
  libical, ical.js, and sabre/vobject (see `conformance/`), including
  libical's OSS-Fuzz crash reproducers and RRULE expansion expectations.
  Property-based tests run under hegel (Rust) and Hypothesis (Python).

## Python quick start

```python
import vobject

doc = vobject.parse(open("calendar.ics", "rb").read())
if doc.repairs:
    print("input needed fixing:", *doc.repairs, sep="\n  ")

for cal in doc:
    for event in cal.comps("VEVENT"):
        print(event.prop("SUMMARY").value)

text = doc.serialize()
```

## Rust quick start

```rust
use vobject_core::{parse, write_document, ParseOptions};

let parsed = parse(input, &ParseOptions::lenient())?;
for repair in &parsed.repairs {
    eprintln!("repaired: {repair}");
}
let out = write_document(&parsed.components, &Default::default());
```

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/vobject-core` | Pure-Rust implementation (no Python dependencies) |
| `crates/vobject-py` | PyO3 bindings (`vobject._core`) |
| `python/vobject` | The Python package (clean API + py-vobject compat modules) |
| `python/icalendar` | Vendored icalendar compat package, incl. its upstream test suite |
| `tests/` | Python test suite (pytest + Hypothesis) |
| `tests_upstream/` | Vendored py-vobject upstream test suite |
| `conformance/` | Vendored reference test data and tooling |
| `DESIGN.md` | Architecture and roadmap |

## Development

Rust: `cargo test`. Python: `uv sync && uv run pytest` (the clean-API
suite; the compatibility suites run with
`uv run pytest tests_upstream/pyvobject python/icalendar/tests`). After
changing Rust code, rebuild the extension with
`uv sync --reinstall-package vobject-rs`. CI (`.github/workflows/ci.yml`)
runs all four suites.

## Typed views and recurrence

```python
doc = vobject.parse(text)
for cal in doc.calendars:
    for event in cal.events:
        print(event.summary, event.start, event.end)
        for occurrence in event.occurrences(limit=10):
            print("  ", occurrence)
```

`vobject.to_jcal()` / `from_jcal()` handle jCal (RFC 7265) / jCard
(RFC 7095), verified against ical.js's expected outputs;
`vobject.to_xcal()` / `from_xcal()` handle xCal (RFC 6321) / xCard
(RFC 6351). `vobject.expand_rrule()` exposes the RRULE engine, validated
against libical's icalrecur expectations, including RSCALE (RFC 7529)
non-Gregorian rules via ICU4X and RFC 5545 DST semantics for zone-aware
starts.

## Compatibility layers

The distribution is a drop-in replacement for two established libraries;
both upstream test suites run against it:

- **py-vobject** — `vobject.readOne` / `readComponents`, behaviors,
  `vobject.base` / `.icalendar` / `.vcard`, and the `ics_diff` /
  `change_tz` scripts (58/58 upstream tests; requires the `compat` extra).
- **icalendar** — a full `icalendar` package (7.2.2 API): `Calendar`,
  `Event`, the `v*` property types, jCal, alarms (15,972 upstream tests).

See `DESIGN.md` for the compat-layer policy.

## Status

Feature-complete: lossless strict/lenient syntax layer, typed values,
jCal/jCard and xCal/xCard, RRULE expansion (including RSCALE and DST-aware
zone expansion), clean typed Python API, and both compatibility layers,
all backed by the cross-implementation conformance corpus. `DESIGN.md`
tracks the roadmap and remaining ideas.
