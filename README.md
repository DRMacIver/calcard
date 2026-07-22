# calcard

A robust, heavily-tested implementation of the vobject family of formats —
iCalendar (RFC 5545) and vCard (RFC 6350 / RFC 2426 / vCard 2.1, with
RFC 6868 parameter encoding) — as a standalone Rust crate
(`vobject-core`) with Python bindings (the `calcard` package).

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
import calcard

doc = calcard.parse(open("calendar.ics", "rb").read())
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
| `crates/vobject-py` | PyO3 bindings (`calcard._core`) |
| `python/calcard` | The Python package |
| `tests/` | Python test suite (pytest + Hypothesis) |
| `conformance/` | Vendored reference test data and tooling |
| `PORTING.md` | Porting notes for py-vobject and icalendar users |

## Development

Rust: `cargo test`. Python: `uv sync && uv run pytest`. After changing
Rust code, rebuild the extension with `uv sync --reinstall-package
calcard`. CI (`.github/workflows/ci.yml`) runs both.

## Typed views and recurrence

```python
doc = calcard.parse(text)
for cal in doc.calendars:
    for event in cal.events:
        print(event.summary, event.start, event.end)
        for occurrence in event.occurrences(limit=10):
            print("  ", occurrence)
```

`calcard.to_jcal()` / `from_jcal()` handle jCal (RFC 7265) / jCard
(RFC 7095), verified against ical.js's expected outputs;
`calcard.to_xcal()` / `from_xcal()` handle xCal (RFC 6321) / xCard
(RFC 6351). `calcard.expand_rrule()` exposes the RRULE engine, validated
against libical's icalrecur expectations, including RSCALE (RFC 7529)
non-Gregorian rules via ICU4X and RFC 5545 DST semantics for zone-aware
starts.

## Timezones

The Rust core deliberately bundles no timezone database: recurrence
expansion in the core is timezone-naive ("floating"), which sidesteps
bundled-tzdata staleness and the hazard of two timezone databases (a
bundled one and the host's) disagreeing inside one process. Timezone
*policy* belongs to the embedding layer: the Python API resolves `TZID`
parameters through the host's `zoneinfo` and implements RFC 5545
local-time DST semantics for zone-aware recurrence expansion.

A `TZID` that `zoneinfo` cannot resolve (Outlook-style names, custom
TZIDs) is interpreted through the document's own `VTIMEZONE` component —
the RFC-conformant, database-free path: `calcard.timezones` builds a
real `tzinfo` (PEP 495 fold semantics included) by expanding the
STANDARD/DAYLIGHT onset rules through the core RRULE engine, validated
property-based against host zoneinfo over libical's generated zone
files. `zoneinfo` stays preferred for names it knows, since real-world
`VTIMEZONE` copies are often stale. A TZID neither can resolve emits a
`TimezoneResolutionWarning` and yields a naive datetime.

## Porting from py-vobject or icalendar

See `PORTING.md` for a mapping of the common py-vobject and icalendar
idioms onto the calcard API. Useful coverage from py-vobject's test
suite is ported in `tests/test_ported_pyvobject.py`, with its real-world
regression documents kept under `conformance/fixtures/pyvobject/`.

## Status

Feature-complete: lossless strict/lenient syntax layer, typed values,
jCal/jCard and xCal/xCard, RRULE expansion (including RSCALE and DST-aware
zone expansion), and a clean typed Python API, all backed by the
cross-implementation conformance corpus.
