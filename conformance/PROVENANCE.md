# Conformance fixture provenance

Vendored by `conformance/tools/vendor.py`. Do not edit files under
`conformance/fixtures/` by hand; re-run the script instead.

## libical

- Repository: https://github.com/libical/libical
- Commit: a5d080c9226ff207b3cbe9709032fdd761e1db88
- License: MPL-2.0 (dual MPL-2.0/LGPL-2.1; MPL chosen) (see `fixtures/libical/LICENSE`)
- `test-data/` fuzzer corpus -> `fuzz/` (49 files, OSS-Fuzz reproducers)
- `test-data/*.ics|*.vcf` -> `samples/` (35 files)
- RRULE expansion expectations -> `recur/` (5 files)
- representative VTIMEZONEs -> `timezones/` (8 files)

## icaljs

- Repository: https://github.com/kewisch/ical.js
- Commit: cd2ef47d5f1c834680ae4b6fa3ad57daa58edffc
- License: MPL-2.0 (see `fixtures/icaljs/LICENSE`)
- `test/parser/` -> `parser/` (52 files: ics/vcf inputs with expected jCal/jCard JSON)
- `samples/*.ics` -> `samples/` (18 files)
- `samples/timezones/` -> `samples/timezones/` (7 files)

## sabre-vobject

- Repository: https://github.com/sabre-io/vobject
- Commit: 2533eb0d67b2030e4f2bdacac972beb556d50e0d
- License: BSD-3-Clause (see `fixtures/sabre-vobject/LICENSE`)
- standalone fixtures -> `samples/` (3 files)
- (the bulk of sabre's test data is embedded in PHP test classes; see tools/extract_sabre.py once written)

