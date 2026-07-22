"""Run the vendored conformance corpus through the Python bindings.

The heavy assertions live in the Rust conformance tests; this sweep checks
that the same guarantees hold through the binding layer (which converts the
whole tree to Python objects and back).
"""

from pathlib import Path

import pytest

import vobject

FIXTURES = Path(__file__).parent.parent / "conformance" / "fixtures"


def corpus():
    files = []
    for path in sorted(FIXTURES.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix in (".ics", ".vcf") or (
            path.parent.name == "fuzz" and path.name != "LICENSE"
        ):
            files.append(path)
    assert len(files) >= 140, f"suspiciously small corpus: {len(files)} files"
    return files


@pytest.mark.parametrize("path", corpus(), ids=lambda p: str(p.relative_to(FIXTURES)))
def test_corpus_file(path):
    doc = vobject.parse(path.read_bytes())
    wire = doc.serialize()
    reparsed = vobject.parse(wire)
    assert len(reparsed.components) == len(doc.components)
