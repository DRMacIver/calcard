# vobject — design and roadmap

A robust, well-tested implementation of the vobject family of formats
(iCalendar / RFC 5545, vCard / RFC 6350 + vCard 3.0 / RFC 2426 + vCard 2.1)
as a standalone Rust crate with Python bindings.

## Goals

1. **Robustness first.** Real-world calendar and contact data is frequently
   malformed. The parser must have a well-defined strict mode *and* a
   recovery mode that degrades gracefully (like sabre/vobject), never
   panicking, and always reporting what was repaired.
2. **Round-trip fidelity.** Parsing and re-serializing a document should
   preserve everything semantically meaningful, including unknown
   properties, parameters, ordering, and vCard property groups.
3. **Conformance via reference suites.** Test data vendored from libical,
   ical.js, and sabre/vobject forms a cross-implementation conformance
   corpus.
4. **Property-based testing.** hegel-rust on the Rust core, Hypothesis on
   the Python layer. Key properties: parse∘serialize round-trips,
   serialize∘parse round-trips on the model, folding/unfolding inverses,
   escaping inverses, and "never panics on arbitrary bytes".
5. **Clean modern Python API** as the primary interface, plus compatibility
   modules that can run the py-vobject and icalendar test suites.

## Layout

```
Cargo.toml                  workspace
crates/vobject-core/        pure-Rust implementation, no Python deps
crates/vobject-py/          PyO3 bindings (cdylib, built by maturin)
python/vobject/             Python package (clean API + compat layers)
python/vobject/compat/      py-vobject and icalendar compatibility modules
tests/                      Python test suite (pytest + Hypothesis)
conformance/fixtures/       vendored test data from reference implementations
conformance/tools/          scripts that fetch/refresh vendored data
```

## Rust core (`vobject-core`)

Layered:

- `syntax`: content-line lexer — unfolding, name/group/params/value
  splitting, escaping (RFC 5545 §3.3.11 TEXT, RFC 6868 caret encoding,
  parameter quoting), folding at 75 octets (UTF-8 safe). Handles vCard 2.1
  quirks: bare parameter values (`TEL;HOME;VOICE:...`), QUOTED-PRINTABLE
  soft line breaks.
- `model`: `Document`, `Component { name, properties, components }`,
  `Property { group, name, params, value: RawValue }`, `Param`. This layer
  is lossless: the raw text of every value is preserved.
- `value`: typed value parsing/serialization on top of the raw model:
  TEXT (incl. structured/multi-valued), DATE, TIME, DATE-TIME, DURATION,
  PERIOD, RECUR, UTC-OFFSET, BINARY, BOOLEAN, INTEGER, FLOAT, URI,
  LANGUAGE-TAG. Value type selection driven by a registry of known
  properties + the VALUE parameter.
- `parse`: BEGIN/END tree building with `ParseOptions { level: Strict |
  Lenient }`; lenient mode records `Repair`s (e.g. unclosed component,
  stray END, bad line) instead of failing.
- `write`: canonical serializer (folding, escaping, deterministic
  parameter formatting) with options (line ending, fold width, vCard 2.1
  compat).
- `rrule` (later phase): RRULE occurrence expansion, validated against
  libical's icalrecur expected-output data.

Errors are typed, carry line numbers, and never panic on any input —
enforced by fuzz-style property tests.

## Python distribution

Distribution name `vobject`, built with maturin (mixed Rust/Python layout).
`vobject._core` is the compiled module; the public package is Python.

- `vobject` — the clean modern API. Design sketch (to be refined against
  the reference-API research):
  - `vobject.parse(text_or_bytes) -> Document`, `vobject.parse_one(...)`
  - Typed components: `Calendar`, `Event`, `Todo`, `Journal`, `Alarm`,
    `Timezone`, `Card`; generic `Component` for everything else.
  - Properties are rich objects; `.value` returns native Python types
    (str, datetime/date with zoneinfo tzinfo, timedelta, …); raw text
    always accessible.
  - Iteration/`in`/indexing follow modern Python conventions; no magic
    attribute soup, but convenient named accessors on typed components
    (`event.start`, `event.uid`, `card.fn`).
  - `document.serialize() -> str` / `.serialize_bytes()`.
- `vobject.compat.pyvobject` — py-vobject-compatible API
  (`readComponents`, `readOne`, attribute access, behaviors). Upstream test
  suite is run against it via a `sys.modules["vobject"]` alias harness in
  `conformance/`.
- `vobject.compat.icalendar` — icalendar-compatible API (`Calendar.from_ical`,
  prop types, `to_ical`). Same aliasing harness for its upstream suite.

## Testing strategy

1. **Rust unit tests** per module, written alongside the code.
2. **hegel-rust property tests** in `crates/vobject-core/tests/`.
3. **Conformance corpus** (`conformance/fixtures/`): vendored from libical
   (incl. icalrecur expansion expectations), ical.js (parser cases with
   expected jCal), sabre/vobject (edge cases, broken inputs). Run from
   both Rust and Python.
4. **Python tests** with pytest + Hypothesis, including cross-checks of the
   Python-visible behavior against the compat targets.
5. **Upstream suites**: harnesses that fetch py-vobject and icalendar at
   pinned versions and run their own tests against our compat modules.

## Roadmap / status

- [x] Workspace scaffolding
- [ ] Content-line lexer + escaping + folding (strict & lenient)
- [ ] Component tree parser/serializer, lossless round-trip
- [ ] Typed values (text, date/time, duration, period, utc-offset, recur…)
- [ ] hegel-rust property tests; fuzz-ish "never panic" tests
- [ ] Vendored conformance corpus + runner
- [ ] PyO3 bindings + clean Python API
- [ ] Hypothesis tests for Python layer
- [ ] py-vobject compat module + upstream suite harness
- [ ] icalendar compat module + upstream suite harness
- [ ] RRULE expansion engine validated against libical data
