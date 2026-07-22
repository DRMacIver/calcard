#!/usr/bin/env python3
r"""Extract embedded vObject test data from sabre/vobject's PHP test suite.

Scans tests/VObject/**/*.php for PHP heredoc/nowdoc strings and for
single/double-quoted multi-line strings assigned to variables, and writes
any that look like vCard/iCalendar data (contain "BEGIN:" or "VERSION:")
to fixture files under conformance/fixtures/sabre-vobject/extracted/.

PHP semantics honoured:
  - <<<'X' (nowdoc): body copied verbatim.
  - <<<X / <<<"X" (heredoc): escape sequences are processed conservatively
    (\\, \n, \r, \t only -- \\ handling is required so that e.g. "\\n" in
    the PHP source correctly becomes a literal backslash-n, which is valid
    inside iCalendar TEXT values). Bodies containing variable interpolation
    ($var or {$...}) are skipped and recorded in the manifest.
  - PHP 7.3 flexible heredoc: the closing marker's indentation is stripped
    from every body line.
  - The newline immediately preceding the closing marker is not part of
    the string.
  - Double-quoted strings: same escapes plus \" and \$; interpolating
    strings are skipped. Single-quoted strings: only \\ and \' are escapes.

Usage: extract_sabre.py --repo /path/to/sabre-vobject/clone
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

HEREDOC_OPEN = re.compile(r"<<<\s*(?:'(?P<now>[A-Za-z_][A-Za-z0-9_]*)'"
                          r'|"(?P<dq>[A-Za-z_][A-Za-z0-9_]*)"'
                          r"|(?P<plain>[A-Za-z_][A-Za-z0-9_]*))\s*$")
QUOTED_ASSIGN = re.compile(r"^\s*\$\w+\s*=\s*(['\"])")
INTERPOLATION = re.compile(r"(?<!\\)(?:\\\\)*(\$[A-Za-z_{]|\{\$)")


def looks_like_vobject(text: str) -> bool:
    return "BEGIN:" in text or "VERSION:" in text


def has_interpolation(text: str) -> bool:
    # Walk the string so that backslash-escaped dollars are not counted.
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "\\":
            i += 2
            continue
        if c == "$" and i + 1 < n and (text[i + 1].isalpha() or text[i + 1] in "_{"):
            return True
        if c == "{" and i + 1 < n and text[i + 1] == "$":
            return True
        i += 1
    return False


def convert_heredoc_escapes(text: str) -> str:
    """Conservative escape conversion for interpolating heredoc bodies."""
    out = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "\\" and i + 1 < n:
            nxt = text[i + 1]
            if nxt == "\\":
                out.append("\\")
                i += 2
                continue
            if nxt == "n":
                out.append("\n")
                i += 2
                continue
            if nxt == "r":
                out.append("\r")
                i += 2
                continue
            if nxt == "t":
                out.append("\t")
                i += 2
                continue
            # Leave any other sequence literal (matches PHP for unknown
            # escapes; \$ etc. are rare enough in fixture data to ignore).
        out.append(c)
        i += 1
    return "".join(out)


def convert_double_quoted_escapes(text: str) -> str:
    r"""Escape conversion for double-quoted strings: heredoc set + \" and \$."""
    out = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "\\" and i + 1 < n:
            nxt = text[i + 1]
            mapped = {"\\": "\\", "n": "\n", "r": "\r", "t": "\t",
                      '"': '"', "$": "$"}.get(nxt)
            if mapped is not None:
                out.append(mapped)
                i += 2
                continue
        out.append(c)
        i += 1
    return "".join(out)


def convert_single_quoted_escapes(text: str) -> str:
    out = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "\\" and i + 1 < n and text[i + 1] in ("\\", "'"):
            out.append(text[i + 1])
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out)


def scan_file(path: Path) -> list[dict]:
    """Return candidate strings found in a PHP file.

    Each candidate is a dict with keys: line (1-based, opener line),
    text (converted content) or skip_reason.
    """
    source = path.read_text(encoding="utf-8")
    lines = source.split("\n")
    results: list[dict] = []
    i = 0
    nlines = len(lines)
    while i < nlines:
        line = lines[i]
        m = HEREDOC_OPEN.search(line)
        if m:
            marker = m.group("now") or m.group("dq") or m.group("plain")
            is_nowdoc = m.group("now") is not None
            close_re = re.compile(
                r"^(\s*)" + re.escape(marker) + r"(?![A-Za-z0-9_])")
            body_lines = []
            j = i + 1
            indent = None
            while j < nlines:
                cm = close_re.match(lines[j])
                if cm:
                    indent = cm.group(1)
                    break
                body_lines.append(lines[j])
                j += 1
            if indent is None:
                # Unterminated heredoc (shouldn't happen in valid PHP).
                i += 1
                continue
            if indent:
                stripped = []
                for bl in body_lines:
                    if bl.startswith(indent):
                        stripped.append(bl[len(indent):])
                    else:
                        # PHP strips up to len(indent) leading whitespace.
                        stripped.append(bl.lstrip()[:len(bl)])
                body_lines = stripped
            body = "\n".join(body_lines)
            entry = {"line": i + 1, "kind": "nowdoc" if is_nowdoc else "heredoc"}
            if is_nowdoc:
                entry["text"] = body
            elif has_interpolation(body):
                if looks_like_vobject(body):
                    entry["skip_reason"] = "interpolation"
            else:
                entry["text"] = body
            if "text" in entry or "skip_reason" in entry:
                results.append(entry)
            i = j + 1
            continue

        qm = QUOTED_ASSIGN.match(line)
        if qm:
            quote = qm.group(1)
            start_line = i
            # Character-scan from just after the opening quote, across lines.
            pos = qm.end()
            raw_chars: list[str] = []
            row, col = i, pos
            closed = False
            while row < nlines:
                text_row = lines[row]
                while col < len(text_row):
                    c = text_row[col]
                    if c == "\\" and col + 1 < len(text_row):
                        raw_chars.append(c)
                        raw_chars.append(text_row[col + 1])
                        col += 2
                        continue
                    if c == "\\" and col + 1 == len(text_row):
                        # Backslash at end of line: escapes nothing that
                        # matters here (newline follows).
                        raw_chars.append(c)
                        col += 1
                        continue
                    if c == quote:
                        closed = True
                        break
                    raw_chars.append(c)
                    col += 1
                if closed:
                    break
                raw_chars.append("\n")
                row += 1
                col = 0
            if not closed:
                i += 1
                continue
            rest = lines[row][col + 1:].strip()
            raw = "".join(raw_chars)
            entry = {"line": start_line + 1, "kind": "quoted"}
            if not rest.startswith(";"):
                # Concatenation or other continuation: partial data.
                if looks_like_vobject(raw):
                    entry["skip_reason"] = "concatenation"
                    results.append(entry)
                i = row + 1
                continue
            if quote == '"':
                if has_interpolation(raw):
                    if looks_like_vobject(raw):
                        entry["skip_reason"] = "interpolation"
                        results.append(entry)
                    i = row + 1
                    continue
                entry["text"] = convert_double_quoted_escapes(raw)
            else:
                entry["text"] = convert_single_quoted_escapes(raw)
            results.append(entry)
            i = row + 1
            continue

        i += 1

    # Apply the vobject heuristic to extracted texts, and require the
    # content to be multi-line (quoted single-line scraps are not fixtures).
    final = []
    for entry in results:
        if "skip_reason" in entry:
            final.append(entry)
            continue
        text = entry["text"]
        if not looks_like_vobject(text):
            continue
        if entry["kind"] == "quoted" and "\n" not in text and "\r" not in text:
            continue
        # Convert conservative escapes for heredoc bodies now (after the
        # interpolation check, which ran on the raw body).
        if entry["kind"] == "heredoc":
            entry["text"] = convert_heredoc_escapes(text)
        final.append(entry)
    return final


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True,
                        help="Path to a sabre/vobject clone")
    parser.add_argument("--out", default=None,
                        help="Output directory (default: "
                             "conformance/fixtures/sabre-vobject/extracted "
                             "relative to this script's repo)")
    args = parser.parse_args()

    repo = Path(args.repo)
    tests_root = repo / "tests" / "VObject"
    if not tests_root.is_dir():
        raise SystemExit(f"error: {tests_root} is not a directory")

    if args.out:
        out_root = Path(args.out)
    else:
        out_root = (Path(__file__).resolve().parent.parent
                    / "fixtures" / "sabre-vobject" / "extracted")
    out_root.mkdir(parents=True, exist_ok=True)

    manifest: dict = {"files": {}, "skipped": []}
    extracted = 0
    skipped = 0

    for php_file in sorted(tests_root.rglob("*.php")):
        rel = php_file.relative_to(tests_root)
        class_dir = rel.with_suffix("")  # e.g. Component/VCardTest
        entries = scan_file(php_file)
        counter = 0
        source_rel = str(php_file.relative_to(repo))
        for entry in entries:
            if "skip_reason" in entry:
                skipped += 1
                manifest["skipped"].append({
                    "source_file": source_rel,
                    "line_number": entry["line"],
                    "reason": entry["skip_reason"],
                })
                continue
            counter += 1
            text = entry["text"]
            ext = "vcf" if "BEGIN:VCARD" in text else "ics"
            out_path = out_root / class_dir / f"{counter:03d}.{ext}"
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_bytes(text.encode("utf-8"))
            extracted += 1
            manifest["files"][str(out_path.relative_to(out_root))] = {
                "source_file": source_rel,
                "line_number": entry["line"],
            }

    manifest_path = out_root / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True)
                             + "\n", encoding="utf-8")

    print(f"extracted {extracted} fixture files to {out_root}")
    print(f"skipped {skipped} strings (see manifest.json)")


if __name__ == "__main__":
    main()
