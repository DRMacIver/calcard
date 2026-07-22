#!/usr/bin/env python3
"""Vendor conformance test data from reference implementations.

Copies test fixtures from libical, ical.js, and sabre/vobject into
conformance/fixtures/, recording provenance (repo, commit, path) and each
project's license alongside the data.

Usage:
    python3 conformance/tools/vendor.py [--cache DIR]

With --cache, existing clones in DIR/<name> are reused; otherwise shallow
clones are made into a temporary directory.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "fixtures"


@dataclass
class Source:
    name: str
    url: str
    license_file: str
    license_name: str


SOURCES = [
    Source("libical", "https://github.com/libical/libical", "LICENSE.txt", "MPL-2.0 (dual MPL-2.0/LGPL-2.1; MPL chosen)"),
    Source("icaljs", "https://github.com/kewisch/ical.js", "LICENSE", "MPL-2.0"),
    Source("sabre-vobject", "https://github.com/sabre-io/vobject", "LICENSE", "BSD-3-Clause"),
]

# A small representative subset of libical's generated VTIMEZONE corpus;
# the full ~600-zone tree is mechanically regenerable from tzdata.
LIBICAL_ZONES = [
    "America/New_York",
    "Europe/London",
    "Europe/Dublin",  # negative DST
    "Australia/Lord_Howe",  # 30-minute DST
    "Asia/Kathmandu",  # +05:45 offset
    "Pacific/Chatham",  # +12:45 offset
    "Pacific/Apia",  # skipped a day in 2011
    "Etc/GMT+9",
]


def run(*args: str, cwd: Path | None = None) -> str:
    return subprocess.run(
        args, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def clone(source: Source, cache: Path | None, tmp: Path) -> Path:
    if cache is not None:
        cached = cache / source.name
        if (cached / ".git").is_dir():
            return cached
    dest = tmp / source.name
    print(f"cloning {source.url} ...")
    run("git", "clone", "--depth", "1", source.url, str(dest))
    return dest


def copy_tree(src: Path, dest: Path, pattern: str = "*") -> int:
    count = 0
    for path in sorted(src.rglob(pattern)):
        if path.is_file():
            rel = path.relative_to(src)
            target = dest / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
            count += 1
    return count


def copy_files(paths: list[Path], dest: Path) -> int:
    dest.mkdir(parents=True, exist_ok=True)
    count = 0
    for path in sorted(paths):
        if path.is_file():
            shutil.copy2(path, dest / path.name)
            count += 1
    return count


def vendor_icaljs(repo: Path, out: Path) -> list[str]:
    lines = []
    n = copy_tree(repo / "test" / "parser", out / "parser")
    lines.append(f"- `test/parser/` -> `parser/` ({n} files: ics/vcf inputs with expected jCal/jCard JSON)")
    n = copy_files([p for p in (repo / "samples").iterdir() if p.suffix == ".ics"], out / "samples")
    lines.append(f"- `samples/*.ics` -> `samples/` ({n} files)")
    n = copy_tree(repo / "samples" / "timezones", out / "samples" / "timezones")
    lines.append(f"- `samples/timezones/` -> `samples/timezones/` ({n} files)")
    lines.append(
        "- (recurrence expectations: tools/extract_icaljs_recur.py produces "
        "`recur/cases.json`)"
    )
    return lines


def vendor_libical(repo: Path, out: Path) -> list[str]:
    lines = []
    test_data = repo / "test-data"

    fuzz = [
        p
        for p in test_data.iterdir()
        if p.is_file()
        and (
            p.name.startswith(("fuzz", "vcardfuzz", "timefuzz", "timezonefuzz", "poc-"))
            or p.name in ("crash.ics", "malloc.ics")
        )
    ]
    n = copy_files(fuzz, out / "fuzz")
    lines.append(f"- `test-data/` fuzzer corpus -> `fuzz/` ({n} files, OSS-Fuzz reproducers)")

    samples = [
        p
        for p in test_data.iterdir()
        if p.is_file()
        and p.suffix in (".ics", ".vcf")
        and p.name not in ("crash.ics", "malloc.ics")
    ]
    n = copy_files(samples, out / "samples")
    lines.append(f"- `test-data/*.ics|*.vcf` -> `samples/` ({n} files)")

    recur_files = [
        repo / "src" / "test" / "icalrecur_test.txt",
        repo / "src" / "test" / "icalrecur_test_rscale.txt",
        repo / "src" / "test" / "icalrecur_test_rscale_withicu.txt",
        repo / "src" / "test" / "icalrecur_test_rscale_withicu_dangi.txt",
        test_data / "recur.txt",
    ]
    n = copy_files([p for p in recur_files if p.exists()], out / "recur")
    lines.append(f"- RRULE expansion expectations -> `recur/` ({n} files)")

    zone_files = []
    for zone in LIBICAL_ZONES:
        p = test_data / "zoneinfo2025c" / f"{zone.replace('/', '-')}.ics"
        if not p.exists():
            candidates = list((test_data).glob(f"zoneinfo*/{zone}.ics")) + list(
                (test_data).glob(f"zoneinfo*/**/{zone.split('/')[-1]}.ics")
            )
            p = candidates[0] if candidates else p
        if p.exists():
            zone_files.append(p)
    if not zone_files:
        # Layout differs between releases; fall back to a glob search.
        for zone in LIBICAL_ZONES:
            hits = sorted(test_data.rglob(f"{zone.split('/')[-1]}.ics"))
            zone_files.extend(hits[:1])
    n = copy_files(zone_files, out / "timezones")
    lines.append(f"- representative VTIMEZONEs -> `timezones/` ({n} files)")
    return lines


def vendor_sabre(repo: Path, out: Path) -> list[str]:
    lines = []
    files = [
        repo / "tests" / "VObject" / "issue64.vcf",
        repo / "tests" / "VObject" / "issue153.vcf",
        repo / "tests" / "VObject" / "RecurrenceIterator" / "UntilRespectsTimezoneTest.ics",
    ]
    n = copy_files([p for p in files if p.exists()], out / "samples")
    lines.append(f"- standalone fixtures -> `samples/` ({n} files)")
    lines.append(
        "- (the bulk of sabre's test data is embedded in PHP test classes; "
        "tools/extract_sabre.py extracts it into `sabre-vobject/extracted/`)"
    )
    return lines


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", type=Path, default=None)
    args = parser.parse_args()

    vendorers = {
        "libical": vendor_libical,
        "icaljs": vendor_icaljs,
        "sabre-vobject": vendor_sabre,
    }

    provenance = [
        "# Conformance fixture provenance",
        "",
        "Vendored by `conformance/tools/vendor.py`. Do not edit files under",
        "`conformance/fixtures/` by hand; re-run the script instead.",
        "",
    ]

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        for source in SOURCES:
            repo = clone(source, args.cache, tmp)
            commit = run("git", "rev-parse", "HEAD", cwd=repo)
            out = FIXTURES / source.name
            if out.exists():
                shutil.rmtree(out)
            out.mkdir(parents=True)
            shutil.copy2(repo / source.license_file, out / "LICENSE")
            lines = vendorers[source.name](repo, out)
            provenance.append(f"## {source.name}")
            provenance.append("")
            provenance.append(f"- Repository: {source.url}")
            provenance.append(f"- Commit: {commit}")
            provenance.append(f"- License: {source.license_name} (see `fixtures/{source.name}/LICENSE`)")
            provenance.extend(lines)
            provenance.append("")
            print(f"{source.name}: done ({commit[:12]})")

    (ROOT / "PROVENANCE.md").write_text("\n".join(provenance) + "\n")
    print("wrote", ROOT / "PROVENANCE.md")
    return 0


if __name__ == "__main__":
    sys.exit(main())
