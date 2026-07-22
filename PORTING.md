# Porting to calcard

calcard's API is deliberately small: a lossless `Component`/`Property`
model, lenient-by-default parsing that reports every `Repair`, typed
component views, and native-value conversion. This guide maps the common
py-vobject and icalendar idioms onto it.

Throughout: `import calcard`.

## Coming from py-vobject

| py-vobject | calcard |
| --- | --- |
| `vobject.readOne(text)` | `calcard.parse_one(text)` |
| `vobject.readComponents(stream)` | `calcard.parse(stream.read()).components` |
| `component.serialize()` | `doc.serialize()` / `calcard.serialize([component])` |
| `cal.vevent` (attribute access) | `cal.comp("VEVENT")`, or typed: `doc.calendars[0].events[0]` |
| `cal.vevent_list` | `cal.comps("VEVENT")` |
| `event.summary.value` | `event.prop("SUMMARY").value` (raw) or typed `Event(...).summary` |
| `event.dtstart.value` (native datetime) | typed `event.start`, or `calcard.native_value(prop)` |
| `component.add("summary").value = s` | typed setter `event.summary = s`, or append a `Property` to `component.children` |
| `prop.params["CN"]` | `{p.name: p.values for p in prop.params}` |
| `prop.group` | `prop.group` (same idea) |
| `cal.vevent.getrruleset()` | `event.occurrences(limit=...)` or `calcard.expand_rrule(rule, dtstart)` |
| `vobject.iCalendar()` | `Component("VCALENDAR", [Property("VERSION", "2.0")])` |
| `serialize()` inventing VTIMEZONEs for used TZIDs | explicit: `Calendar(...).add_missing_timezones()` (or `calcard.add_missing_timezones(component)`) before serializing |
| behaviors / `transformToNative` | not needed: parsing is lossless, typed interpretation happens on access via `native_value` |
| `ignoreUnreadable=True` | lenient parsing is the default; inspect `doc.repairs` |
| validation errors at parse time | strict grammar: `parse(text, strict=True)`; value-level problems raise when the value is interpreted |
| `ics_diff` / `change_tz` scripts | no equivalent; diff on the model or shift datetimes via the typed setters |
| hCalendar output | no equivalent |

Notes:

- py-vobject decoded QUOTED-PRINTABLE and renamed nonstandard property
  names (`X_A` → `X-A`); calcard keeps both verbatim (soft line breaks
  are joined, with a repair recorded) — losslessness is the contract.
- py-vobject auto-inserted PRODID/VERSION when serializing behaviors;
  calcard serializes exactly what is in the model.
- py-vobject also rewrote TZIDs to the zone's current abbreviation
  (`Europe/Berlin` → `TZID:CET`), which is where its ambiguous-
  abbreviation cache bugs (PST: Pacific or Philippine?) come from;
  calcard keeps the IANA name from the datetime's tzinfo and holds no
  global timezone state.

## Coming from icalendar

| icalendar | calcard |
| --- | --- |
| `Calendar.from_ical(text)` | `calcard.parse_one(text)` (or `.parse` for streams of components) |
| `cal.to_ical()` | `doc.serialize().encode()` |
| `Event()` / `cal.add_component(...)` | `Component("VEVENT", children)`; append to `parent.children` |
| `cal["prodid"]` / `cal.add("prodid", ...)` | `cal.prop("PRODID").value` / append a `Property` |
| `event.decoded("dtstart")` | typed `event.start`, or `calcard.native_value(prop)` |
| `vDatetime`, `vDate`, `vDuration`, ... | plain Python types out of `native_value` (datetime/date/timedelta/...); wire text via `Property.value` |
| `event["rrule"]` (`vRecur` dict) | `event.rrule` (rule text) and `event.occurrences()` |
| `cal.to_jcal()` / `from_jcal` | `calcard.to_jcal(component)` / `calcard.from_jcal(data)` |
| `cal.add_missing_timezones()` | `Calendar(...).add_missing_timezones()`, or `calcard.add_missing_timezones(component)` / `calcard.vtimezone_from_tzinfo(tz, start=..., end=...)` |
| `icalendar.use_pytz()` | not applicable: aware datetimes use `zoneinfo` |

## History

Earlier revisions of this project shipped API-compatible adaptations of
both libraries (able to run their upstream test suites). That compat
layer was dropped in favour of these porting notes; the useful test
coverage was ported to the clean API (`tests/test_ported_pyvobject.py`,
with real-world regression documents kept under
`conformance/fixtures/pyvobject/`).
