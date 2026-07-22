"""Clean-API ports of the useful coverage from py-vobject's test suite.

The scenarios (documents and expected semantics) are adapted from
py-vobject's tests (https://github.com/py-vobject/vobject, Apache-2.0;
see conformance/fixtures/pyvobject/LICENSE); the test code is rewritten
against the calcard API. Real-world regression documents referenced here
live in conformance/fixtures/pyvobject/.
"""

import datetime as dt
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest

import calcard
from calcard import Component, ParseError, Property, native_value

FIXTURES = Path(__file__).parent.parent / "conformance" / "fixtures" / "pyvobject"


def parse_one_line(line: str, *, strict: bool = True) -> Property:
    """Parse a single content line inside a wrapper component."""
    doc = calcard.parse(f"BEGIN:X\r\n{line}\r\nEND:X\r\n", strict=strict)
    (prop,) = doc.components[0].properties()
    return prop


def params_of(prop: Property) -> dict:
    return {p.name: p.values for p in prop.params}


# ---------------------------------------------------------------------------
# Content-line splitting (py-vobject's parseLine/parseParams cases)


def test_line_with_empty_value():
    prop = parse_one_line("BLAH:")
    assert (prop.name, prop.params, prop.value, prop.group) == ("BLAH", [], "", None)


def test_colon_inside_value_is_kept():
    prop = parse_one_line("RDATE:VALUE=DATE:19970304,19970504,19970704,19970904")
    assert prop.name == "RDATE"
    assert prop.value == "VALUE=DATE:19970304,19970504,19970704,19970904"
    assert prop.params == []


def test_quoted_param_with_colon():
    prop = parse_one_line(
        'DESCRIPTION;ALTREP="http://www.wiz.org":The Fall 98 Wild Wizards '
        "Conference - - Las Vegas, NV, USA"
    )
    assert params_of(prop) == {"ALTREP": ["http://www.wiz.org"]}
    assert prop.value == (
        "The Fall 98 Wild Wizards Conference - - Las Vegas, NV, USA"
    )


def test_bare_parameters_vcard21_style():
    # Strictly invalid (no '='); lenient keeps them as value-less params
    # with repairs recorded.
    doc = calcard.parse("BEGIN:X\r\nEMAIL;PREF;INTERNET:john@nowhere.com\r\nEND:X\r\n")
    assert doc.repairs
    (prop,) = doc.components[0].properties()
    assert [(p.name, p.values) for p in prop.params] == [("PREF", []), ("INTERNET", [])]
    assert prop.value == "john@nowhere.com"
    with pytest.raises(ParseError):
        calcard.parse("BEGIN:X\r\nEMAIL;PREF:x\r\nEND:X\r\n", strict=True)


def test_mixed_quoted_and_bare_param_values():
    prop = parse_one_line(
        'EMAIL;TYPE="blah",hah;INTERNET="DIGI",DERIDOO:john@nowhere.com'
    )
    assert params_of(prop) == {
        "TYPE": ["blah", "hah"],
        "INTERNET": ["DIGI", "DERIDOO"],
    }


def test_quoted_param_value_with_semicolons():
    prop = parse_one_line(
        'X;ALTREP="http://www.wiz.org;;",Blah,Foo;NEXT=Nope;BAR:v', strict=False
    )
    assert [(p.name, p.values) for p in prop.params] == [
        ("ALTREP", ["http://www.wiz.org;;", "Blah", "Foo"]),
        ("NEXT", ["Nope"]),
        ("BAR", []),
    ]


def test_group_prefix():
    prop = parse_one_line(
        "item1.ADR;type=HOME;type=pref:;;Reeperbahn 116;Hamburg;;20359;",
        strict=False,
    )
    assert prop.group == "item1"
    assert prop.name == "ADR"
    assert [(p.name, p.values) for p in prop.params] == [
        ("type", ["HOME"]),
        ("type", ["pref"]),
    ]
    assert prop.value == ";;Reeperbahn 116;Hamburg;;20359;"


def test_nameless_line_is_an_error():
    with pytest.raises(ParseError):
        calcard.parse("BEGIN:X\r\n:\r\nEND:X\r\n", strict=True)
    doc = calcard.parse("BEGIN:X\r\n:\r\nEND:X\r\n")
    assert doc.repairs
    assert doc.components[0].properties() == []


def test_folded_line_joins():
    prop = parse_one_line("STUFF:folded\r\n line")
    assert prop.value == "foldedline"


# ---------------------------------------------------------------------------
# Whole-document parsing


STANDARD = (
    "BEGIN:VCALENDAR\r\n"
    "CALSCALE:GREGORIAN\r\n"
    "X-WR-TIMEZONE;VALUE=TEXT:US/Pacific\r\n"
    "METHOD:PUBLISH\r\n"
    "PRODID:-//Apple Computer\\, Inc//iCal 1.0//EN\r\n"
    "X-WR-CALNAME;VALUE=TEXT:Example\r\n"
    "VERSION:2.0\r\n"
    "BEGIN:VEVENT\r\n"
    "SEQUENCE:5\r\n"
    "DTSTART;TZID=US/Pacific:20021028T140000\r\n"
    "RRULE:FREQ=Weekly;COUNT=10\r\n"
    "DTSTAMP:20021028T011706Z\r\n"
    "SUMMARY:Coffee with Jason\r\n"
    "UID:EC9439B1-FF65-11D6-9973-003065F99D04\r\n"
    "DTEND;TZID=US/Pacific:20021028T150000\r\n"
    "BEGIN:VALARM\r\n"
    "TRIGGER;VALUE=DURATION:-P1D\r\n"
    "ACTION:DISPLAY\r\n"
    "DESCRIPTION:Event reminder\\, with comma\\nand line feed\r\n"
    "END:VALARM\r\n"
    "END:VEVENT\r\n"
    "END:VCALENDAR\r\n"
)


def test_standard_document_import():
    doc = calcard.parse(STANDARD, strict=True)
    (cal,) = doc.calendars
    (event,) = cal.events
    assert event.summary == "Coffee with Jason"
    assert event.start == dt.datetime(2002, 10, 28, 14, 0, tzinfo=ZoneInfo("US/Pacific"))
    assert event.end == dt.datetime(2002, 10, 28, 15, 0, tzinfo=ZoneInfo("US/Pacific"))
    assert event.text("DTSTAMP") == [
        dt.datetime(2002, 10, 28, 1, 17, 6, tzinfo=dt.timezone.utc)
    ]
    (alarm,) = event.alarms
    assert alarm.trigger == dt.timedelta(days=-1)
    assert alarm.text("DESCRIPTION") == "Event reminder, with comma\nand line feed"
    assert doc.serialize() == STANDARD


def test_nonsensical_typed_value_surfaces_on_access_not_parse():
    # Lossless parsing accepts anything; interpreting the broken TRIGGER
    # raises.
    doc = calcard.parse(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "BEGIN:VALARM\r\nTRIGGER:a20021028120000\r\nEND:VALARM\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
        strict=True,
    )
    trigger = doc.components[0].comp("VEVENT").comp("VALARM").prop("TRIGGER")
    with pytest.raises(ParseError):
        native_value(trigger)


def test_bad_property_names():
    bad = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "X-BAD/SLASH:TRUE\r\nX-BAD_UNDERSCORE:TRUE\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    with pytest.raises(ParseError):
        calcard.parse(bad, strict=True)
    doc = calcard.parse(bad)
    assert doc.repairs
    event = doc.components[0].comp("VEVENT")
    # Lossless: nonstandard names are kept verbatim (py-vobject renamed
    # them), and the slash name is unparseable in either mode.
    names = [p.name for p in event.properties()]
    assert "X-BAD_UNDERSCORE" in names


def test_quoted_printable_soft_breaks_join():
    qp = (
        "BEGIN:VCARD\r\n"
        "VERSION:2.1\r\n"
        "N;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=E9=BB=84;=E4=B8=96=E5=8B=87;;;\r\n"
        "URL;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=68=74=74=70=3A=2F=2F=77=65=69=62=6F=2E=63=6F=6D=2F=33=30=39=34=39=30=\r\n"
        "=30=34=33=33=3F=E9=97=AA=E9=97=AA=48=E7=BA=A2=E6=98=9F\r\n"
        "END:VCARD\r\n"
    )
    doc = calcard.parse(qp)
    assert any("quoted-printable" in r.message for r in doc.repairs)
    (card,) = doc.components
    (url,) = card.props("URL")
    # The soft line break is joined into a single logical value.
    assert url.value.endswith("=30=34=33=33=3F=E9=97=AA=E9=97=AA=48=E7=BA=A2=E6=98=9F")
    assert len(card.props("N")) == 1


def test_unicode_summary_round_trips():
    text = (
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"
        "SUMMARY:The title こんにちはキティ\r\n"
        "LOCATION:こんにちはキティ\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    doc = calcard.parse(text, strict=True)
    event = doc.calendars[0].events[0]
    assert event.summary == "The title こんにちはキティ"
    again = calcard.parse(doc.serialize(), strict=True)
    assert again.components == doc.components


def test_multiline_description_unfolds_and_unescapes():
    journal = (
        "BEGIN:VJOURNAL\r\n"
        "DESCRIPTION:1. Staff meeting: Participants include Joe\\,\r\n"
        "  Lisa\\, and Bob. Aurora project plans were reviewed.\r\n"
        "  Next meeting on Tuesday.\\n2. Telephone Conference: fine.\r\n"
        "END:VJOURNAL\r\n"
    )
    doc = calcard.parse(journal, strict=True)
    description = doc.components[0].prop("DESCRIPTION")
    value = native_value(description)
    assert "Joe, Lisa, and Bob" in value
    assert "Tuesday.\n2." in value
    again = calcard.parse(doc.serialize(), strict=True)
    assert native_value(again.components[0].prop("DESCRIPTION")) == value


def test_scratch_build_serializes_and_reparses():
    event = Component(
        "VEVENT",
        [
            Property("UID", "Not very random UID"),
            Property("DTSTART", "20060509T000000"),
            Property("DESCRIPTION", "Test event"),
            Property(
                "ATTENDEE",
                "mailto:froelich@example.com",
                params=[calcard.Param("CN", ["Fröhlich"])],
            ),
        ],
    )
    cal = Component("VCALENDAR", [Property("VERSION", "2.0"), event])
    wire = calcard.serialize([cal])
    doc = calcard.parse(wire, strict=True)
    parsed_event = doc.calendars[0].events[0]
    assert parsed_event.start == dt.datetime(2006, 5, 9, 0, 0)
    attendee = parsed_event.component.prop("ATTENDEE")
    assert params_of(attendee) == {"CN": ["Fröhlich"]}


def test_categories_native_list():
    prop = parse_one_line("CATEGORIES:Random category,Other category")
    assert native_value(prop) == ["Random category", "Other category"]


def test_request_status_structured_value():
    prop = parse_one_line("REQUEST-STATUS:5.1;Service unavailable")
    value = native_value(prop)
    assert value == [["5.1"], ["Service unavailable"]]


def test_vtodo_document():
    text = (
        "BEGIN:VCALENDAR\r\n"
        "VERSION:2.0\r\n"
        "BEGIN:VTODO\r\n"
        "UID:20070313T123432Z-456553@example.com\r\n"
        "DTSTAMP:20070313T123432Z\r\n"
        "DUE;VALUE=DATE:20070501\r\n"
        "SUMMARY:Submit Quebec Income Tax Return for 2006\r\n"
        "CLASS:CONFIDENTIAL\r\n"
        "CATEGORIES:FAMILY,FINANCE\r\n"
        "STATUS:NEEDS-ACTION\r\n"
        "END:VTODO\r\n"
        "END:VCALENDAR\r\n"
    )
    doc = calcard.parse(text, strict=True)
    (todo,) = doc.calendars[0].todos
    assert todo.due == dt.date(2007, 5, 1)
    assert todo.status == "NEEDS-ACTION"
    assert todo.text("CATEGORIES") == ["FAMILY", "FINANCE"]

    todo.component.children = todo.component.children + [
        Property("COMPLETED", "20150505T133000")
    ]
    again = calcard.parse(doc.serialize(), strict=True)
    assert again.calendars[0].todos[0].text("COMPLETED") == [
        dt.datetime(2015, 5, 5, 13, 30)
    ]


VCARD_WITH_GROUPS = (
    "home.begin:vcard\r\n"
    "version:3.0\r\n"
    "fn:Meister Berger\r\n"
    "n:Berger;Meister\r\n"
    "note:The Mayor of the great city of\r\n"
    "  Goerlitz in the great country of Germany.\\nNext line.\r\n"
    "email;internet:mb@goerlitz.de\r\n"
    "home.tel;type=fax,voice;type=msg:+49 3581 123456\r\n"
    "END:VCARD\r\n"
)


def test_vcard_with_groups():
    doc = calcard.parse(VCARD_WITH_GROUPS)
    assert doc.repairs  # group on BEGIN, bare params: lenient territory
    (card,) = doc.components
    assert card.name.lower() == "vcard"
    tel = card.prop("tel")
    assert tel.group == "home"
    assert tel.value == "+49 3581 123456"
    note = native_value(card.prop("note"), "vcard4")
    assert note == (
        "The Mayor of the great city of Goerlitz in the great country of "
        "Germany.\nNext line."
    )
    # Groups survive re-serialization.
    wire = doc.serialize()
    assert "home.TEL" in wire or "home.tel" in wire


def test_vcard30_structured_org_round_trips():
    text = (
        "BEGIN:VCARD\r\n"
        "VERSION:3.0\r\n"
        "FN:Daffy Duck Knudson (with Bugs Bunny and Mr. Pluto)\r\n"
        "N:Knudson;Daffy Duck (with Bugs Bunny and Mr. Pluto)\r\n"
        "ADR;type=HOME:;;Haight Street 512\\;\\nEscape\\, Test;Novosibirsk;;80214;Gnuland\r\n"
        "ORG:University of Novosibirsk;Department of Octopus Parthenogenesis\r\n"
        "END:VCARD\r\n"
    )
    doc = calcard.parse(text, strict=True)
    card = doc.components[0]
    org = native_value(card.prop("ORG"), "vcard4")
    assert org == [
        ["University of Novosibirsk"],
        ["Department of Octopus Parthenogenesis"],
    ]
    for _ in range(3):
        doc = calcard.parse(doc.serialize(), strict=True)
        assert native_value(doc.components[0].prop("ORG"), "vcard4") == org


def test_date_valued_until_includes_final_day():
    # Folded-name lines plus a date-valued UNTIL that must include 12/28.
    recurrence = (
        "BEGIN:VCALENDAR\r\nVERSION\r\n :2.0\r\nBEGIN:VEVENT\r\n"
        "UID\r\n :70922B3051D34A9E852570EC00022388\r\n"
        "RRULE\r\n :FREQ=MONTHLY;UNTIL=20061228;INTERVAL=1;BYDAY=4TH\r\n"
        "DTSTART\r\n :20060126T230000Z\r\n"
        "END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
    doc = calcard.parse(recurrence)
    (event,) = doc.calendars[0].events
    dates = event.occurrences()
    assert dates[0] == dt.datetime(2006, 1, 26, 23, 0, tzinfo=dt.timezone.utc)
    assert dates[1] == dt.datetime(2006, 2, 23, 23, 0, tzinfo=dt.timezone.utc)
    assert dates[-1] == dt.datetime(2006, 12, 28, 23, 0, tzinfo=dt.timezone.utc)


def test_freebusy_periods():
    text = (
        "BEGIN:VFREEBUSY\r\nUID:test\r\n"
        "DTSTART:20060216T010000Z\r\nDTEND:20060216T030000Z\r\n"
        "FREEBUSY:20060216T010000Z/PT1H\r\n"
        "FREEBUSY:20060216T010000Z/20060216T030000Z\r\n"
        "END:VFREEBUSY\r\n"
    )
    doc = calcard.parse(text, strict=True)
    fb = doc.components[0]
    start = dt.datetime(2006, 2, 16, 1, 0, tzinfo=dt.timezone.utc)
    periods = [native_value(p) for p in fb.props("FREEBUSY")]
    assert periods == [
        [(start, dt.timedelta(hours=1))],
        [(start, dt.datetime(2006, 2, 16, 3, 0, tzinfo=dt.timezone.utc))],
    ]
    assert calcard.parse(doc.serialize(), strict=True).components == doc.components


# ---------------------------------------------------------------------------
# Real-world regression documents (radicale issues, via py-vobject)


@pytest.mark.parametrize(
    "name",
    sorted(p.name for p in FIXTURES.glob("*.ics")),
)
def test_radicale_documents_parse_and_round_trip(name):
    text = (FIXTURES / name).read_text()
    doc = calcard.parse(text)
    assert doc.components, name
    # Lenient reparse of our own output reproduces the model.
    again = calcard.parse(doc.serialize())
    assert again.components == doc.components


def test_radicale_1587_geo_survives():
    # Upstream's regression: float GEO coordinates must serialize exactly
    # (py-vobject once emitted repr() noise); losslessness gives it to us,
    # but keep the assertion.
    doc = calcard.parse((FIXTURES / "radicale_1587.ics").read_text())
    geo = doc.components[0].prop("GEO")
    assert geo.value == "37.386013;-122.082932"
    assert "GEO:37.386013;-122.082932" in doc.serialize()
