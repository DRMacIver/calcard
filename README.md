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
| `python/vobject` | The Python package |
| `tests/` | Python test suite (pytest + Hypothesis) |
| `conformance/` | Vendored reference test data and tooling |
| `DESIGN.md` | Architecture and roadmap |

## Development

Rust: `cargo test`. Python: `uv sync && uv run pytest`. After changing Rust
code, rebuild the extension with `uv sync --reinstall-package vobject-rs`.

## Status

Early development. The syntax layer (parsing, serialization, escaping,
folding) is complete and heavily tested; typed values, recurrence
expansion, and the py-vobject / icalendar compatibility layers are in
progress — see `DESIGN.md`.
