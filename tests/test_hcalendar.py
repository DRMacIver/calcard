"""Tests for hCalendar serialization in the py-vobject compatibility layer."""

import datetime
from html.parser import HTMLParser

import vobject.base
import vobject.hcalendar  # noqa: F401  (registers the HCALENDAR behavior)


class TagCollector(HTMLParser):
    def __init__(self):
        super().__init__()
        self.tags = []

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, attrs))


def _sample_hcal():
    cal = vobject.base.newFromBehavior("hcalendar")
    cal.add("vevent")
    cal.vevent.add("summary").value = "Web 2.0 Conference"
    cal.vevent.add("url").value = "http://www.web2con.com/"
    cal.vevent.add("dtstart").value = datetime.date(2006, 2, 27)
    cal.vevent.add("dtend").value = datetime.date(2006, 3, 1)
    cal.vevent.add("location").value = "Argent Hotel, San Francisco, CA"
    return cal


def _tags_by_class(html):
    parser = TagCollector()
    parser.feed(html)
    by_class = {}
    for tag, attrs in parser.tags:
        attr_dict = dict(attrs)
        if "class" in attr_dict:
            by_class[attr_dict["class"]] = (tag, attrs)
    return by_class


def test_hcalendar_serialization_structure():
    html = _sample_hcal().serialize()
    by_class = _tags_by_class(html)

    assert '<span class="summary">Web 2.0 Conference</span>' in html
    assert "Argent Hotel, San Francisco, CA" in html
    assert by_class["vevent"][0] == "span"
    assert by_class["url"][0] == "a"
    assert dict(by_class["url"][1])["href"] == "http://www.web2con.com/"
    assert by_class["location"][0] == "span"

    tag, attrs = by_class["dtstart"]
    assert tag == "abbr"
    assert attrs == [("class", "dtstart"), ("title", "20060227")]

    tag, attrs = by_class["dtend"]
    assert tag == "abbr"
    assert attrs == [("class", "dtend"), ("title", "20060301")]
    # The human-readable dtend text is the day before the exclusive end date.
    assert "February 28" in html


def test_hcalendar_attributes_are_valid_html():
    html = _sample_hcal().serialize()
    # No stray comma between attributes, e.g. <abbr class="dtstart", title=...>
    assert '",' not in html

    parser = TagCollector()
    parser.feed(html)
    for tag, attrs in parser.tags:
        for name, value in attrs:
            assert name.isidentifier() or "-" in name, (tag, name)
            assert value is not None, (tag, name)
