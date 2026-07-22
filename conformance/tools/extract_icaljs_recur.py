#!/usr/bin/env python3
"""Extract RRULE expansion test cases from ical.js test sources.

Parses testRRULE(...) calls out of test/recur_iterator_test.js and
verifyFail('...') calls out of test/recur_test.js in a checkout of
https://github.com/kewisch/ical.js, emitting a machine-readable JSON
fixture at conformance/fixtures/icaljs/recur/cases.json.

Usage:
    python extract_icaljs_recur.py /path/to/ical.js [output.json]

No JavaScript is executed; the calls are regular enough for a small
recursive-descent parse of the literal arguments. Uses only the stdlib.
"""

import json
import re
import subprocess
import sys
from pathlib import Path


class ParseError(Exception):
    pass


class Parser:
    """Minimal parser for the JS literal subset used by the test calls:

    string literals ('...' or "..."), numbers, true/false, arrays of
    literals, and flat object literals with identifier keys. Skips // and
    /* */ comments and tolerates trailing commas.
    """

    def __init__(self, text, pos=0):
        self.text = text
        self.pos = pos

    def error(self, msg):
        line = self.text.count("\n", 0, self.pos) + 1
        raise ParseError(f"{msg} at line {line} (offset {self.pos})")

    def skip_ws(self):
        text, n = self.text, len(self.text)
        while self.pos < n:
            ch = text[self.pos]
            if ch in " \t\r\n":
                self.pos += 1
            elif text.startswith("//", self.pos):
                nl = text.find("\n", self.pos)
                self.pos = n if nl == -1 else nl + 1
            elif text.startswith("/*", self.pos):
                end = text.find("*/", self.pos + 2)
                if end == -1:
                    self.error("unterminated block comment")
                self.pos = end + 2
            else:
                return

    def expect(self, ch):
        self.skip_ws()
        if self.pos >= len(self.text) or self.text[self.pos] != ch:
            self.error(f"expected {ch!r}")
        self.pos += 1

    def parse_string(self):
        self.skip_ws()
        quote = self.text[self.pos]
        if quote not in "'\"":
            self.error("expected string literal")
        self.pos += 1
        out = []
        while True:
            if self.pos >= len(self.text):
                self.error("unterminated string")
            ch = self.text[self.pos]
            if ch == "\\":
                out.append(self.text[self.pos + 1])
                self.pos += 2
            elif ch == quote:
                self.pos += 1
                break
            else:
                out.append(ch)
                self.pos += 1
        # Handle string concatenation with '+'.
        self.skip_ws()
        if self.pos < len(self.text) and self.text[self.pos] == "+":
            self.pos += 1
            return "".join(out) + self.parse_string()
        return "".join(out)

    def parse_value(self):
        self.skip_ws()
        ch = self.text[self.pos]
        if ch in "'\"":
            return self.parse_string()
        if ch == "[":
            return self.parse_array()
        if ch == "{":
            return self.parse_object()
        m = re.match(r"true|false|null|-?\d+(\.\d+)?", self.text[self.pos:])
        if not m:
            self.error("expected value")
        tok = m.group(0)
        self.pos += len(tok)
        if tok == "true":
            return True
        if tok == "false":
            return False
        if tok == "null":
            return None
        return float(tok) if "." in tok else int(tok)

    def parse_array(self):
        self.expect("[")
        items = []
        while True:
            self.skip_ws()
            if self.text[self.pos] == "]":
                self.pos += 1
                return items
            items.append(self.parse_value())
            self.skip_ws()
            if self.text[self.pos] == ",":
                self.pos += 1

    def parse_object(self):
        self.expect("{")
        obj = {}
        while True:
            self.skip_ws()
            if self.text[self.pos] == "}":
                self.pos += 1
                return obj
            m = re.match(r"[A-Za-z_$][\w$]*", self.text[self.pos:])
            if not m:
                self.error("expected identifier key")
            key = m.group(0)
            self.pos += len(m.group(0))
            self.expect(":")
            obj[key] = self.parse_value()
            self.skip_ws()
            if self.text[self.pos] == ",":
                self.pos += 1


def extract_testrrule_cases(source, filename):
    """Extract all testRRULE('RULE', {...}) calls."""
    cases = []
    # Match call sites whose first argument is a string literal; this
    # excludes the helper definition and its internal forwarding call.
    for m in re.finditer(r"testRRULE(?:\.only)?\(\s*(?=['\"])", source):
        line = source.count("\n", 0, m.start()) + 1
        parser = Parser(source, m.end())
        rrule = parser.parse_string()
        parser.expect(",")
        options = parser.parse_object()
        parser.expect(")")

        unknown = set(options) - {
            "dtStart", "dates", "byCount", "until", "noInstance", "max", "only",
        }
        if unknown:
            raise ParseError(
                f"{filename}:{line}: unknown option keys {sorted(unknown)}"
            )

        case = {
            "rrule": rrule,
            "source_file": filename,
            "source_line": line,
        }
        # The testRRULE helper defaults dtStart to dates[0].
        if "dtStart" in options:
            case["dtstart"] = options["dtStart"]
        else:
            case["dtstart"] = options["dates"][0]
            case["dtstart_implicit"] = True
        if options.get("noInstance"):
            # The rule is valid but produces no occurrences at all.
            case["no_instances"] = True
        else:
            case["dates"] = options["dates"]
        if "byCount" in options:
            case["by_count"] = options["byCount"]
        if "until" in options:
            case["until"] = options["until"]
        if "max" in options:
            case["max"] = options["max"]
        # A finite rule is expected to be exhausted after the listed dates
        # (the JS helper iterates one past the end for finite rules).
        case["finite"] = bool(options.get("byCount") or options.get("until"))
        cases.append(case)
    return cases


def extract_verifyfail_cases(source, filename):
    """Extract verifyFail('RULE', /error/) calls with string arguments."""
    cases = []
    for m in re.finditer(r"verifyFail\(\s*(?=['\"])", source):
        line = source.count("\n", 0, m.start()) + 1
        parser = Parser(source, m.end())
        rrule = parser.parse_string()
        case = {
            "rrule": rrule,
            "source_file": filename,
            "source_line": line,
            "invalid": True,
        }
        parser.skip_ws()
        if source[parser.pos] == ",":
            parser.pos += 1
            parser.skip_ws()
            if source[parser.pos] == "/":
                end = source.index("/", parser.pos + 1)
                case["error_pattern"] = source[parser.pos + 1:end]
        cases.append(case)
    return cases


def main():
    if len(sys.argv) < 2:
        sys.exit(f"usage: {sys.argv[0]} /path/to/ical.js [output.json]")
    repo = Path(sys.argv[1])
    default_out = (
        Path(__file__).resolve().parent.parent
        / "fixtures" / "icaljs" / "recur" / "cases.json"
    )
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else default_out

    iterator_src = (repo / "test" / "recur_iterator_test.js").read_text()
    recur_src = (repo / "test" / "recur_test.js").read_text()

    cases = extract_testrrule_cases(iterator_src, "test/recur_iterator_test.js")
    cases += extract_verifyfail_cases(recur_src, "test/recur_test.js")

    try:
        commit = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        commit = None

    doc = {
        "source": {
            "repository": "https://github.com/kewisch/ical.js",
            "commit": commit,
            "files": ["test/recur_iterator_test.js", "test/recur_test.js"],
        },
        "cases": cases,
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(doc, indent=2) + "\n")

    n_expand = sum(1 for c in cases if "dates" in c)
    n_noinst = sum(1 for c in cases if c.get("no_instances"))
    n_invalid = sum(1 for c in cases if c.get("invalid"))
    print(f"wrote {out_path}")
    print(f"total cases:        {len(cases)}")
    print(f"  with dates:       {n_expand}")
    print(f"  no_instances:     {n_noinst}")
    print(f"  invalid (parse):  {n_invalid}")
    print(f"  finite (count):   {sum(1 for c in cases if c.get('by_count'))}")
    print(f"  finite (until):   {sum(1 for c in cases if c.get('until'))}")


if __name__ == "__main__":
    main()
