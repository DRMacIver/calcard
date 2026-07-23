//! Typed interpretation of property values.
//!
//! The document model stores raw value text; this module parses raw values
//! into typed representations ([`Value`]) and serializes them back. The
//! type of a property is resolved from its `VALUE` parameter if present,
//! otherwise from a per-dialect registry of known property names
//! ([`default_type_info`]).

pub mod base64;
pub mod datetime;
pub mod duration;
pub mod recur;

use std::fmt;

pub use datetime::{Date, DateOrDateTime, DateTime, Time, UtcOffset, Weekday};
pub use duration::{Duration, Period, PeriodEnd};
pub use recur::{Frequency, Recur, RecurMonth, Skip, Until, WeekdayNum};

use crate::escape::{escape_text, split_unescaped, unescape_text};
use crate::model::Property;

/// An error interpreting a raw value as a typed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError {
    pub message: String,
}

impl ValueError {
    pub fn new(message: impl Into<String>) -> ValueError {
        ValueError {
            message: message.into(),
        }
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValueError {}

/// Which format's property registry applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    ICalendar,
    VCard4,
    VCard3,
}

/// The wire type of a value, per RFC 5545 §3.3 / RFC 6350 §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Binary,
    Boolean,
    CalAddress,
    Date,
    DateAndOrTime,
    DateTime,
    Duration,
    Float,
    Integer,
    LanguageTag,
    Period,
    /// vCard 3.0's PHONE-NUMBER type (RFC 2426); text semantics.
    PhoneNumber,
    Recur,
    Text,
    Time,
    Timestamp,
    Uri,
    UtcOffset,
    /// vCard 3.0's VCARD type (an embedded vCard, e.g. AGENT); text
    /// semantics.
    Vcard,
    Unknown,
}

impl ValueType {
    /// Resolve a VALUE parameter (case-insensitive).
    pub fn from_name(name: &str) -> Option<ValueType> {
        Some(match name.to_ascii_uppercase().as_str() {
            "BINARY" => ValueType::Binary,
            "BOOLEAN" => ValueType::Boolean,
            "CAL-ADDRESS" => ValueType::CalAddress,
            "DATE" => ValueType::Date,
            "DATE-AND-OR-TIME" => ValueType::DateAndOrTime,
            "DATE-TIME" => ValueType::DateTime,
            "DURATION" => ValueType::Duration,
            "FLOAT" => ValueType::Float,
            "INTEGER" => ValueType::Integer,
            "LANGUAGE-TAG" => ValueType::LanguageTag,
            "PERIOD" => ValueType::Period,
            "PHONE-NUMBER" => ValueType::PhoneNumber,
            "RECUR" => ValueType::Recur,
            "TEXT" => ValueType::Text,
            "TIME" => ValueType::Time,
            "TIMESTAMP" => ValueType::Timestamp,
            "URI" => ValueType::Uri,
            "UTC-OFFSET" => ValueType::UtcOffset,
            "VCARD" => ValueType::Vcard,
            "UNKNOWN" => ValueType::Unknown,
            _ => return None,
        })
    }

    /// The jCal/jCard type name (RFC 7265 / RFC 7095): lowercase.
    pub fn jcal_name(&self) -> &'static str {
        match self {
            ValueType::Binary => "binary",
            ValueType::Boolean => "boolean",
            ValueType::CalAddress => "cal-address",
            ValueType::Date => "date",
            ValueType::DateAndOrTime => "date-and-or-time",
            ValueType::DateTime => "date-time",
            ValueType::Duration => "duration",
            ValueType::Float => "float",
            ValueType::Integer => "integer",
            ValueType::LanguageTag => "language-tag",
            ValueType::Period => "period",
            ValueType::PhoneNumber => "phone-number",
            ValueType::Recur => "recur",
            ValueType::Text => "text",
            ValueType::Time => "time",
            ValueType::Timestamp => "timestamp",
            ValueType::Uri => "uri",
            ValueType::UtcOffset => "utc-offset",
            ValueType::Vcard => "vcard",
            ValueType::Unknown => "unknown",
        }
    }
}

/// How a property's value is composed at the top level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Multiplicity {
    /// One value.
    Single,
    /// Comma-separated list (e.g. CATEGORIES, EXDATE).
    CommaList,
    /// Semicolon-separated components (e.g. N, ADR, REQUEST-STATUS, GEO).
    /// When `comma_inner` is true, each component is itself a comma list
    /// (N and ADR); otherwise commas are ordinary characters.
    Structured { comma_inner: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeInfo {
    pub vtype: ValueType,
    pub multiplicity: Multiplicity,
}

const fn single(vtype: ValueType) -> TypeInfo {
    TypeInfo {
        vtype,
        multiplicity: Multiplicity::Single,
    }
}

const fn list(vtype: ValueType) -> TypeInfo {
    TypeInfo {
        vtype,
        multiplicity: Multiplicity::CommaList,
    }
}

const fn structured(vtype: ValueType) -> TypeInfo {
    TypeInfo {
        vtype,
        multiplicity: Multiplicity::Structured { comma_inner: false },
    }
}

const fn structured_lists(vtype: ValueType) -> TypeInfo {
    TypeInfo {
        vtype,
        multiplicity: Multiplicity::Structured { comma_inner: true },
    }
}

/// Default type info for a property name in a dialect. Unknown names get
/// `unknown`/single, matching RFC 7265's treatment.
pub fn default_type_info(name: &str, dialect: Dialect) -> TypeInfo {
    let upper = name.to_ascii_uppercase();
    match dialect {
        Dialect::ICalendar => icalendar_type_info(&upper),
        Dialect::VCard4 => vcard4_type_info(&upper),
        Dialect::VCard3 => vcard3_type_info(&upper),
    }
}

fn icalendar_type_info(name: &str) -> TypeInfo {
    use ValueType::*;
    match name {
        // Calendar / descriptive text
        "CALSCALE" | "METHOD" | "PRODID" | "VERSION" | "CLASS" | "COMMENT" | "DESCRIPTION"
        | "LOCATION" | "STATUS" | "SUMMARY" | "TRANSP" | "TZID" | "TZNAME" | "CONTACT"
        | "RELATED-TO" | "UID" | "ACTION" | "BUSYTYPE" | "NAME" | "COLOR" | "LOCATION-TYPE"
        | "PARTICIPANT-TYPE" | "RESOURCE-TYPE" | "PROXIMITY" => single(Text),
        "CATEGORIES" | "RESOURCES" => list(Text),
        "REQUEST-STATUS" => structured(Text),
        // Dates and times
        "COMPLETED" | "DTEND" | "DUE" | "DTSTART" | "RECURRENCE-ID" | "CREATED" | "DTSTAMP"
        | "LAST-MODIFIED" | "ACKNOWLEDGED" => single(DateTime),
        "EXDATE" | "RDATE" => list(DateTime),
        "FREEBUSY" => list(Period),
        "DURATION" | "TRIGGER" | "REFRESH-INTERVAL" => single(Duration),
        "TZOFFSETFROM" | "TZOFFSETTO" => single(UtcOffset),
        "RRULE" | "EXRULE" => single(Recur),
        // Numbers
        "PERCENT-COMPLETE" | "PRIORITY" | "REPEAT" | "SEQUENCE" => single(Integer),
        "GEO" => structured(Float),
        // References
        "ATTACH" | "TZURL" | "URL" | "IMAGE" | "CONFERENCE" | "SOURCE" | "STRUCTURED-DATA"
        | "STYLED-DESCRIPTION" | "CALENDAR-ADDRESS" | "LINK" => single(Uri),
        "ATTENDEE" | "ORGANIZER" => single(CalAddress),
        _ => single(Unknown),
    }
}

fn vcard4_type_info(name: &str) -> TypeInfo {
    use ValueType::*;
    match name {
        "FN" | "EMAIL" | "NOTE" | "PRODID" | "TITLE" | "ROLE" | "VERSION" | "KIND" | "XML"
        | "UID" | "EXPERTISE" | "HOBBY" | "INTEREST" | "ORG-DIRECTORY" | "TEL" => single(Text),
        "NICKNAME" | "CATEGORIES" => list(Text),
        "N" | "ADR" => structured_lists(Text),
        "GENDER" | "ORG" | "CLIENTPIDMAP" => structured(Text),
        "BDAY" | "ANNIVERSARY" | "DEATHDATE" => single(DateAndOrTime),
        "REV" => single(Timestamp),
        "LANG" => single(LanguageTag),
        "TZ" => single(Text),
        "SOURCE" | "PHOTO" | "IMPP" | "GEO" | "LOGO" | "MEMBER" | "RELATED" | "SOUND" | "URL"
        | "KEY" | "FBURL" | "CALADRURI" | "CALURI" => single(Uri),
        _ => single(Unknown),
    }
}

fn vcard3_type_info(name: &str) -> TypeInfo {
    use ValueType::*;
    match name {
        "FN" | "EMAIL" | "NOTE" | "PRODID" | "TITLE" | "ROLE" | "VERSION" | "MAILER" | "UID"
        | "LABEL" | "SORT-STRING" | "CLASS" | "PROFILE" | "SOURCE" | "NAME" => single(Text),
        "TEL" => single(PhoneNumber),
        "NICKNAME" | "CATEGORIES" => list(Text),
        "N" | "ADR" => structured_lists(Text),
        "ORG" => structured(Text),
        "BDAY" => single(Date),
        "REV" => single(DateTime),
        "GEO" => structured(Float),
        "TZ" => single(UtcOffset),
        "PHOTO" | "LOGO" | "SOUND" | "KEY" => single(Binary),
        "URL" => single(Uri),
        "AGENT" => single(Vcard),
        _ => single(Unknown),
    }
}

/// A typed property value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Binary(Vec<u8>),
    Boolean(bool),
    CalAddress(String),
    Date(Vec<Date>),
    /// DATE-TIME-typed values; individual entries may degrade to DATE in
    /// lenient real-world data (`DateOrDateTime` covers both).
    DateTime(Vec<DateOrDateTime>),
    Duration(Vec<Duration>),
    Float(Vec<f64>),
    Integer(Vec<i64>),
    Period(Vec<Period>),
    /// Boxed: `Recur` is much larger than the other variants.
    Recur(Box<Recur>),
    /// Unescaped text values; single-valued TEXT properties have exactly
    /// one element.
    Text(Vec<String>),
    /// Semicolon-separated components, each a (possibly singleton) comma
    /// list of unescaped text.
    Structured(Vec<Vec<String>>),
    Time(Vec<Time>),
    Uri(String),
    UtcOffset(UtcOffset),
    /// Raw text of a value whose type is unknown.
    Unknown(String),
}

fn unescape_lenient(s: &str) -> String {
    let mut repairs = Vec::new();
    unescape_text(s, Some(&mut repairs), 0).expect("lenient unescape is total")
}

fn parse_list<T>(
    raw: &str,
    multiplicity: Multiplicity,
    parse: impl Fn(&str) -> Result<T, ValueError>,
) -> Result<Vec<T>, ValueError> {
    match multiplicity {
        Multiplicity::Single => Ok(vec![parse(raw)?]),
        _ => split_unescaped(raw, ',').iter().map(|p| parse(p)).collect(),
    }
}

/// Parse a raw value as the given type.
pub fn parse_value(raw: &str, info: TypeInfo) -> Result<Value, ValueError> {
    use ValueType::*;
    Ok(match info.vtype {
        Text | PhoneNumber | Vcard => match info.multiplicity {
            Multiplicity::Single => Value::Text(vec![unescape_lenient(raw)]),
            Multiplicity::CommaList => Value::Text(
                split_unescaped(raw, ',')
                    .iter()
                    .map(|p| unescape_lenient(p))
                    .collect(),
            ),
            Multiplicity::Structured { comma_inner } => Value::Structured(
                split_unescaped(raw, ';')
                    .iter()
                    .map(|component| {
                        if comma_inner {
                            split_unescaped(component, ',')
                                .iter()
                                .map(|p| unescape_lenient(p))
                                .collect()
                        } else {
                            vec![unescape_lenient(component)]
                        }
                    })
                    .collect(),
            ),
        },
        DateTime | Timestamp => Value::DateTime(parse_list(raw, info.multiplicity, |s| {
            DateOrDateTime::parse(s.trim())
        })?),
        Date => Value::Date(parse_list(raw, info.multiplicity, |s| {
            datetime::Date::parse(s.trim())
        })?),
        Time => Value::Time(parse_list(raw, info.multiplicity, |s| {
            datetime::Time::parse(s.trim())
        })?),
        Duration => Value::Duration(parse_list(raw, info.multiplicity, |s| {
            duration::Duration::parse(s.trim())
        })?),
        Period => Value::Period(parse_list(raw, info.multiplicity, |s| {
            duration::Period::parse(s.trim())
        })?),
        Recur => Value::Recur(Box::new(recur::Recur::parse(raw)?)),
        Integer => Value::Integer(parse_list(raw, info.multiplicity, |s| {
            s.trim()
                .parse::<i64>()
                .map_err(|_| ValueError::new(format!("invalid INTEGER {s:?}")))
        })?),
        Float => {
            let parse_float = |s: &str| -> Result<f64, ValueError> {
                let f: f64 = s
                    .trim()
                    .parse()
                    .map_err(|_| ValueError::new(format!("invalid FLOAT {s:?}")))?;
                if !f.is_finite() {
                    return Err(ValueError::new(format!("non-finite FLOAT {s:?}")));
                }
                Ok(f)
            };
            match info.multiplicity {
                Multiplicity::Structured { .. } => {
                    // GEO: lat;lon
                    let parts: Vec<f64> = split_unescaped(raw, ';')
                        .iter()
                        .map(|p| parse_float(p))
                        .collect::<Result<_, _>>()?;
                    Value::Float(parts)
                }
                m => Value::Float(parse_list(raw, m, parse_float)?),
            }
        }
        Boolean => match raw.trim().to_ascii_uppercase().as_str() {
            "TRUE" => Value::Boolean(true),
            "FALSE" => Value::Boolean(false),
            _ => return Err(ValueError::new(format!("invalid BOOLEAN {raw:?}"))),
        },
        Binary => Value::Binary(base64::decode(raw)?),
        // Escaped commas appear in vCard URIs (RFC 6350 §3.4 requires
        // escaping them); unescaping is a no-op for clean iCalendar URIs.
        Uri => Value::Uri(unescape_lenient(raw)),
        CalAddress => Value::CalAddress(unescape_lenient(raw)),
        UtcOffset => Value::UtcOffset(datetime::UtcOffset::parse(raw.trim())?),
        LanguageTag => Value::Text(vec![raw.to_string()]),
        DateAndOrTime => {
            // vCard 4 partial forms are handled leniently: full forms parse
            // to dates/date-times; anything else is preserved as text.
            match DateOrDateTime::parse(raw.trim()) {
                Ok(d) => Value::DateTime(vec![d]),
                Err(_) => Value::Text(vec![raw.to_string()]),
            }
        }
        Unknown => Value::Unknown(raw.to_string()),
    })
}

/// Serialize a typed value back to raw text with correct escaping.
pub fn serialize_value(value: &Value) -> String {
    fn join<T: ToString>(items: &[T], sep: &str) -> String {
        items
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(sep)
    }
    match value {
        Value::Text(items) => items
            .iter()
            .map(|s| escape_text(s))
            .collect::<Vec<_>>()
            .join(","),
        Value::Structured(components) => components
            .iter()
            .map(|c| {
                c.iter()
                    .map(|s| escape_text(s))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(";"),
        Value::Date(items) => join(items, ","),
        Value::DateTime(items) => join(items, ","),
        Value::Time(items) => join(items, ","),
        Value::Duration(items) => join(items, ","),
        Value::Period(items) => join(items, ","),
        Value::Recur(r) => r.to_string(),
        Value::Integer(items) => join(items, ","),
        Value::Float(items) => {
            let formatted: Vec<String> = items.iter().map(|f| format_float(*f)).collect();
            formatted.join(";")
        }
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Binary(data) => base64::encode(data),
        Value::Uri(s) | Value::CalAddress(s) | Value::Unknown(s) => s.clone(),
        Value::UtcOffset(o) => o.to_string(),
    }
}

/// Format a float the way the RFCs write them (no exponent, no trailing
/// zeros beyond what's needed, but keep at least "x.y" style when there is
/// a fraction).
pub fn format_float(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

impl Property {
    /// The resolved type info for this property: the VALUE parameter if
    /// present and recognized, else the registry default. The multiplicity
    /// always comes from the registry (VALUE=DATE on an EXDATE keeps its
    /// list nature).
    pub fn type_info(&self, dialect: Dialect) -> TypeInfo {
        let default = default_type_info(&self.name, dialect);
        match self.param_value("VALUE").and_then(ValueType::from_name) {
            Some(vtype) => TypeInfo {
                vtype,
                multiplicity: if vtype == default.vtype || matches!(vtype, ValueType::Text) {
                    // A VALUE param merely restating the registry default
                    // (GEO;VALUE=FLOAT), or a VALUE=TEXT override on a
                    // structured property, keeps the registry multiplicity.
                    default.multiplicity
                } else {
                    match default.multiplicity {
                        Multiplicity::Structured { .. } => Multiplicity::Single,
                        m => m,
                    }
                },
            },
            None => default,
        }
    }

    /// Parse this property's value as its resolved type.
    pub fn typed_value(&self, dialect: Dialect) -> Result<Value, ValueError> {
        parse_value(&self.value, self.type_info(dialect))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Param;

    fn prop(name: &str, value: &str) -> Property {
        Property::new(name, value)
    }

    #[test]
    fn registry_defaults() {
        let info = default_type_info("DTSTART", Dialect::ICalendar);
        assert_eq!(info.vtype, ValueType::DateTime);
        assert_eq!(
            default_type_info("categories", Dialect::ICalendar).multiplicity,
            Multiplicity::CommaList
        );
        assert_eq!(
            default_type_info("X-ANYTHING", Dialect::ICalendar).vtype,
            ValueType::Unknown
        );
        assert_eq!(
            default_type_info("N", Dialect::VCard4).multiplicity,
            Multiplicity::Structured { comma_inner: true }
        );
    }

    #[test]
    fn value_param_overrides() {
        let mut p = prop("DTSTART", "20260722");
        p.params.push(Param::new("VALUE", "DATE"));
        assert_eq!(p.type_info(Dialect::ICalendar).vtype, ValueType::Date);
        assert_eq!(
            p.typed_value(Dialect::ICalendar).unwrap(),
            Value::Date(vec![Date::new(2026, 7, 22).unwrap()])
        );
    }

    #[test]
    fn redundant_value_param_keeps_multiplicity() {
        // GEO;VALUE=FLOAT restates the registry default; it must not
        // collapse the structured lat;lon multiplicity to Single.
        let plain = prop("GEO", "37.386013;-122.082932")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(plain, Value::Float(vec![37.386013, -122.082932]));
        let mut with_param = prop("GEO", "37.386013;-122.082932");
        with_param.params.push(Param::new("VALUE", "FLOAT"));
        assert_eq!(with_param.typed_value(Dialect::ICalendar).unwrap(), plain);

        // A genuinely overriding VALUE param still gets Single.
        let mut overridden = prop("GEO", "geo:37.386013,-122.082932");
        overridden.params.push(Param::new("VALUE", "URI"));
        let info = overridden.type_info(Dialect::ICalendar);
        assert_eq!(info.vtype, ValueType::Uri);
        assert_eq!(info.multiplicity, Multiplicity::Single);
    }

    #[test]
    fn datetime_property() {
        let v = prop("DTSTART", "20260722T160000Z")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        match v {
            Value::DateTime(items) => {
                assert_eq!(items.len(), 1);
                assert!(matches!(items[0], DateOrDateTime::DateTime(dt) if dt.utc()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn multivalued_exdate() {
        let v = prop("EXDATE", "20260101T000000Z,20260102T000000Z")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert!(matches!(v, Value::DateTime(items) if items.len() == 2));
    }

    #[test]
    fn text_unescaping_and_lists() {
        let v = prop("SUMMARY", "a\\,b\\nc")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(v, Value::Text(vec!["a,b\nc".to_string()]));

        let v = prop("CATEGORIES", "one,two\\,half,three")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(
            v,
            Value::Text(vec![
                "one".to_string(),
                "two,half".to_string(),
                "three".to_string()
            ])
        );
    }

    #[test]
    fn structured_values() {
        let v = prop("REQUEST-STATUS", "2.0;Success")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(
            v,
            Value::Structured(vec![vec!["2.0".into()], vec!["Success".into()]])
        );

        let v = prop("N", "Public;John;Quinlan;Mr.;Esq.")
            .typed_value(Dialect::VCard4)
            .unwrap();
        assert!(matches!(v, Value::Structured(c) if c.len() == 5));

        let v = prop("N", "Stevenson;John;Philip,Paul;Dr.;Jr.,M.D.")
            .typed_value(Dialect::VCard4)
            .unwrap();
        match v {
            Value::Structured(c) => {
                assert_eq!(c[2], vec!["Philip".to_string(), "Paul".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn geo_is_structured_floats() {
        let v = prop("GEO", "37.386013;-122.082932")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(v, Value::Float(vec![37.386013, -122.082932]));
    }

    #[test]
    fn numbers_and_booleans() {
        assert_eq!(
            prop("PRIORITY", "5")
                .typed_value(Dialect::ICalendar)
                .unwrap(),
            Value::Integer(vec![5])
        );
        assert!(prop("PRIORITY", "five")
            .typed_value(Dialect::ICalendar)
            .is_err());
        let mut p = prop("X-B", "TRUE");
        p.params.push(Param::new("VALUE", "BOOLEAN"));
        assert_eq!(
            p.typed_value(Dialect::ICalendar).unwrap(),
            Value::Boolean(true)
        );
    }

    #[test]
    fn duration_and_trigger() {
        assert_eq!(
            prop("DURATION", "PT1H")
                .typed_value(Dialect::ICalendar)
                .unwrap(),
            Value::Duration(vec![Duration::parse("PT1H").unwrap()])
        );
        // TRIGGER;VALUE=DATE-TIME overrides duration default.
        let mut p = prop("TRIGGER", "20260722T120000Z");
        p.params.push(Param::new("VALUE", "DATE-TIME"));
        assert!(matches!(
            p.typed_value(Dialect::ICalendar).unwrap(),
            Value::DateTime(_)
        ));
    }

    #[test]
    fn binary_round_trip() {
        let mut p = prop("ATTACH", "Zm9vYmFy");
        p.params.push(Param::new("VALUE", "BINARY"));
        let v = p.typed_value(Dialect::ICalendar).unwrap();
        assert_eq!(v, Value::Binary(b"foobar".to_vec()));
        assert_eq!(serialize_value(&v), "Zm9vYmFy");
    }

    #[test]
    fn unknown_properties_keep_raw() {
        let v = prop("X-WEIRD", "anything; unescaped, goes")
            .typed_value(Dialect::ICalendar)
            .unwrap();
        assert_eq!(v, Value::Unknown("anything; unescaped, goes".to_string()));
    }

    #[test]
    fn serialize_round_trips() {
        let cases: Vec<(&str, &str, Dialect)> = vec![
            ("SUMMARY", "a\\,b\\nc", Dialect::ICalendar),
            ("CATEGORIES", "one,two\\,half", Dialect::ICalendar),
            (
                "EXDATE",
                "20260101T000000Z,20260102T000000Z",
                Dialect::ICalendar,
            ),
            ("DURATION", "PT1H30M", Dialect::ICalendar),
            ("GEO", "37.386013;-122.082932", Dialect::ICalendar),
            ("RRULE", "FREQ=WEEKLY;BYDAY=MO,WE", Dialect::ICalendar),
            ("TZOFFSETFROM", "-0500", Dialect::ICalendar),
            ("N", "Public;John;Quinlan;Mr.;Esq.", Dialect::VCard4),
            (
                "FREEBUSY",
                "19970101T180000Z/PT1H,19970102T180000Z/PT1H",
                Dialect::ICalendar,
            ),
        ];
        for (name, raw, dialect) in cases {
            let p = prop(name, raw);
            let v = p.typed_value(dialect).unwrap();
            assert_eq!(serialize_value(&v), raw, "{name}:{raw}");
        }
    }

    #[test]
    fn float_formatting() {
        assert_eq!(format_float(37.0), "37");
        assert_eq!(format_float(37.386013), "37.386013");
        assert_eq!(format_float(-0.5), "-0.5");
    }
}
