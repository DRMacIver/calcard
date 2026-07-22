"""Regression tests for bugs fixed in the py-vobject compatibility layer.

Covers:

* Debug ``print()`` calls in ``calcard.compat.pyvobject.icalendar`` that fired during normal
  serialization and ``getrruleset()`` use.
* ``calcard.compat.pyvobject.base.foldOneLine`` attempting a bytes write into a text buffer on
  every call (masked by a broad ``except Exception``).
"""

import datetime
import io

import calcard.compat.pyvobject
import calcard.compat.pyvobject.base
from calcard.compat.pyvobject.base import foldOneLine


def _calendar_with_rrule():
    cal = calcard.compat.pyvobject.iCalendar()
    event = cal.add("vevent")
    event.add("uid").value = "recurrence-test@example.com"
    event.add("summary").value = "Weekly sync"
    event.add("dtstart").value = datetime.datetime(2026, 1, 5, 9, 0)
    event.add("dtstamp").value = datetime.datetime(2026, 1, 1, 0, 0)
    event.add("rrule").value = "FREQ=WEEKLY;COUNT=3"
    return cal


def test_serialize_with_rrule_writes_nothing_to_stdout(capsys):
    cal = _calendar_with_rrule()
    serialized = cal.serialize()
    assert "RRULE:FREQ=WEEKLY;COUNT=3" in serialized
    captured = capsys.readouterr()
    assert captured.out == ""


def test_getrruleset_writes_nothing_to_stdout(capsys):
    cal = _calendar_with_rrule()
    rruleset = cal.vevent.getrruleset(addRDate=True)
    dates = list(rruleset)
    assert dates[0] == datetime.datetime(2026, 1, 5, 9, 0)
    assert len(dates) == 3
    captured = capsys.readouterr()
    assert captured.out == ""


class ByteRejectingStringIO(io.StringIO):
    """A text buffer that records and rejects any attempt to write bytes."""

    def __init__(self):
        super().__init__()
        self.bytes_write_attempts = 0

    def write(self, s):
        if isinstance(s, bytes):
            self.bytes_write_attempts += 1
            raise AssertionError("bytes handed to a text buffer")
        return super().write(s)


def _fold(line, line_length=75):
    buf = ByteRejectingStringIO()
    foldOneLine(buf, line, line_length)
    assert buf.bytes_write_attempts == 0
    return buf.getvalue()


def _unfold(folded):
    assert folded.endswith("\r\n")
    return folded[:-2].replace("\r\n ", "")


def test_fold_one_line_short_line_writes_str_only():
    assert _fold("SUMMARY:Tea") == "SUMMARY:Tea\r\n"


def test_fold_one_line_long_line_default_width():
    line = "DESCRIPTION:" + "x" * 200
    folded = _fold(line)
    assert _unfold(folded) == line
    for physical in folded[:-2].split("\r\n"):
        assert len(physical.encode("utf-8")) <= 75


def test_fold_one_line_nonstandard_width_multibyte():
    line = "SUMMARY:" + "é" * 40
    folded = _fold(line, line_length=30)
    assert _unfold(folded) == line
    for physical in folded[:-2].split("\r\n"):
        assert len(physical.encode("utf-8")) <= 30
