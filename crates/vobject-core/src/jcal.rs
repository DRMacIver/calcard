//! jCal (RFC 7265) and jCard (RFC 7095) conversion, compatible with the
//! dialect ical.js emits (which is the de-facto reference: its author
//! co-wrote RFC 7265). Notable conventions, matched against ical.js's own
//! expected-output test corpus:
//!
//! - PERIOD values are `[start, end]` arrays.
//! - Values that fail typed parsing degrade per type (invalid BOOLEAN →
//!   `false`, invalid FLOAT → `0.0`, invalid INTEGER → `0`, date family →
//!   the raw string).
//! - Single-element lists in RECUR collapse to scalars.
//! - vCard groups become a `group` parameter; jCard carries an (empty)
//!   subcomponent array like jCal.

use serde_json::{json, Map, Value as Json};

use crate::model::{Component, Property};
use crate::value::{
    self, Date, DateOrDateTime, DateTime, Dialect, Multiplicity, Time, TypeInfo, Until, Value,
    ValueType,
};

fn json_date(d: &Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

fn json_time(t: &Time) -> String {
    format!(
        "{:02}:{:02}:{:02}{}",
        t.hour,
        t.minute,
        t.second,
        if t.utc { "Z" } else { "" }
    )
}

fn json_datetime(dt: &DateTime) -> String {
    format!(
        "{}T{}",
        json_date(&dt.date),
        json_time(&dt.time)
    )
}

fn json_date_or_datetime(d: &DateOrDateTime) -> String {
    match d {
        DateOrDateTime::Date(d) => json_date(d),
        DateOrDateTime::DateTime(dt) => json_datetime(dt),
    }
}

fn json_utc_offset(seconds: i32) -> String {
    let total = seconds.unsigned_abs();
    let sign = if seconds < 0 { '-' } else { '+' };
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if s != 0 {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    } else {
        format!("{sign}{h:02}:{m:02}")
    }
}

fn number(f: f64) -> Json {
    if f == f.trunc() && f.abs() < 1e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// A list of numbers, collapsed to a scalar when singleton (recur parts).
fn scalar_or_list<T: Copy + Into<i64>>(items: &[T]) -> Json {
    if items.len() == 1 {
        json!(items[0].into())
    } else {
        Json::Array(items.iter().map(|i| json!((*i).into())).collect())
    }
}

fn recur_to_json(recur: &value::Recur) -> Json {
    let mut obj = Map::new();
    if let Some(rscale) = &recur.rscale {
        obj.insert("rscale".into(), json!(rscale));
    }
    if let Some(freq) = recur.freq {
        obj.insert("freq".into(), json!(freq.as_str()));
    }
    if let Some(skip) = recur.skip {
        obj.insert("skip".into(), json!(skip.as_str()));
    }
    if let Some(count) = recur.count {
        obj.insert("count".into(), json!(count));
    }
    if let Some(interval) = recur.interval {
        obj.insert("interval".into(), json!(interval));
    }
    if let Some(until) = &recur.until {
        let s = match until {
            Until::Date(d) => json_date(d),
            Until::DateTime(dt) => json_datetime(dt),
        };
        obj.insert("until".into(), json!(s));
    }
    if let Some(wkst) = recur.wkst {
        obj.insert("wkst".into(), json!(wkst.abbrev()));
    }
    if !recur.by_second.is_empty() {
        obj.insert("bysecond".into(), scalar_or_list(&recur.by_second));
    }
    if !recur.by_minute.is_empty() {
        obj.insert("byminute".into(), scalar_or_list(&recur.by_minute));
    }
    if !recur.by_hour.is_empty() {
        obj.insert("byhour".into(), scalar_or_list(&recur.by_hour));
    }
    if !recur.by_day.is_empty() {
        let days: Vec<Json> = recur.by_day.iter().map(|d| json!(d.to_string())).collect();
        obj.insert(
            "byday".into(),
            if days.len() == 1 {
                days.into_iter().next().unwrap()
            } else {
                Json::Array(days)
            },
        );
    }
    if !recur.by_month_day.is_empty() {
        obj.insert("bymonthday".into(), scalar_or_list(&recur.by_month_day));
    }
    if !recur.by_year_day.is_empty() {
        obj.insert("byyearday".into(), scalar_or_list(&recur.by_year_day));
    }
    if !recur.by_week_no.is_empty() {
        obj.insert("byweekno".into(), scalar_or_list(&recur.by_week_no));
    }
    if !recur.by_month.is_empty() {
        // RFC 7529: leap months are strings ("5L"), others plain numbers.
        let months: Vec<Json> = recur
            .by_month
            .iter()
            .map(|m| {
                if m.leap {
                    json!(m.to_string())
                } else {
                    json!(m.month)
                }
            })
            .collect();
        obj.insert(
            "bymonth".into(),
            if months.len() == 1 {
                months.into_iter().next().unwrap()
            } else {
                Json::Array(months)
            },
        );
    }
    if !recur.by_set_pos.is_empty() {
        obj.insert("bysetpos".into(), scalar_or_list(&recur.by_set_pos));
    }
    for (name, value) in &recur.extra {
        obj.insert(name.to_ascii_lowercase(), json!(value));
    }
    Json::Object(obj)
}

/// Convert vCard 4 compact date/time forms to jCard's dashed/coloned
/// representation, leaving already-punctuated or unrecognized segments
/// alone: `--0203` → `--02-03`, `20090808T1430-0500` →
/// `2009-08-08T14:30-05:00`.
fn vcard_datetime_to_jcard(raw: &str) -> String {
    fn all_digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    fn dashed_date(d: &str) -> String {
        if d.len() == 8 && all_digits(d) {
            format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8])
        } else if d.len() == 6 && d.starts_with("--") && all_digits(&d[2..]) {
            format!("--{}-{}", &d[2..4], &d[4..6])
        } else {
            d.to_string()
        }
    }
    fn coloned_zone(z: &str) -> String {
        let (sign, digits) = z.split_at(1);
        if digits.len() == 4 && all_digits(digits) {
            format!("{sign}{}:{}", &digits[0..2], &digits[2..4])
        } else {
            z.to_string()
        }
    }
    fn coloned_time(t: &str) -> String {
        // Split a trailing zone: 'Z', or '+'/'-' after at least two digits.
        let (body, zone) = if let Some(b) = t.strip_suffix(['Z', 'z']) {
            (b, "Z".to_string())
        } else if let Some(pos) = t.rfind(['+', '-']).filter(|&p| p >= 2) {
            (&t[..pos], coloned_zone(&t[pos..]))
        } else {
            (t, String::new())
        };
        let body = if all_digits(body) {
            match body.len() {
                6 => format!("{}:{}:{}", &body[0..2], &body[2..4], &body[4..6]),
                4 => format!("{}:{}", &body[0..2], &body[2..4]),
                _ => body.to_string(),
            }
        } else {
            body.to_string()
        };
        format!("{body}{zone}")
    }

    match raw.split_once(['T', 't']) {
        Some((d, t)) => {
            let date = if d.is_empty() {
                String::new()
            } else {
                dashed_date(d)
            };
            format!("{date}T{}", coloned_time(t))
        }
        None => dashed_date(raw),
    }
}

/// Convert one typed value into the jCal value slots (one or more array
/// elements following the type string).
fn value_to_json_slots(prop: &Property, info: TypeInfo, dialect: Dialect) -> Vec<Json> {
    // vCard 4 partial date/time forms are string transformations, not full
    // semantic parses. vCard 3 has no such types: values whose VALUE param
    // claims them pass through raw.
    if matches!(
        info.vtype,
        ValueType::DateAndOrTime | ValueType::Timestamp
    ) {
        return if dialect == Dialect::VCard4 {
            vec![json!(vcard_datetime_to_jcard(prop.value.trim()))]
        } else {
            vec![json!(prop.value.clone())]
        };
    }

    let parsed = value::parse_value(&prop.value, info);
    match parsed {
        Ok(Value::Text(items)) => items.into_iter().map(|s| json!(s)).collect(),
        Ok(Value::Structured(components)) => {
            // One slot. Components collapse: a singleton component is a
            // plain string, and a single-component value collapses to that
            // component directly (GENDER:M → "M").
            let rendered: Vec<Json> = components
                .into_iter()
                .map(|c| {
                    if c.len() == 1 {
                        json!(c.into_iter().next().unwrap())
                    } else {
                        Json::Array(c.into_iter().map(|s| json!(s)).collect())
                    }
                })
                .collect();
            if rendered.len() == 1 {
                vec![rendered.into_iter().next().unwrap()]
            } else {
                vec![Json::Array(rendered)]
            }
        }
        Ok(Value::Date(items)) => items.iter().map(|d| json!(json_date(d))).collect(),
        Ok(Value::DateTime(items)) => items
            .iter()
            .map(|d| json!(json_date_or_datetime(d)))
            .collect(),
        Ok(Value::Time(items)) => items.iter().map(|t| json!(json_time(t))).collect(),
        Ok(Value::Duration(items)) => items.iter().map(|d| json!(d.to_string())).collect(),
        Ok(Value::Period(items)) => items
            .iter()
            .map(|p| {
                let end = match &p.end {
                    value::PeriodEnd::End(dt) => json_datetime(dt),
                    value::PeriodEnd::Duration(d) => d.to_string(),
                };
                json!([json_datetime(&p.start), end])
            })
            .collect(),
        Ok(Value::Recur(r)) => vec![recur_to_json(&r)],
        Ok(Value::Integer(items)) => items.iter().map(|i| json!(i)).collect(),
        Ok(Value::Float(items)) => {
            if matches!(info.multiplicity, Multiplicity::Structured { .. }) {
                // GEO: one slot, [lat, lon].
                vec![Json::Array(items.iter().map(|f| number(*f)).collect())]
            } else {
                items.iter().map(|f| number(*f)).collect()
            }
        }
        Ok(Value::Boolean(b)) => vec![json!(b)],
        Ok(Value::Binary(_)) => vec![json!(prop.value.clone())],
        Ok(Value::Uri(s)) | Ok(Value::CalAddress(s)) | Ok(Value::Unknown(s)) => {
            vec![json!(s)]
        }
        Ok(Value::UtcOffset(o)) => vec![json!(json_utc_offset(o.seconds))],
        // Per-type degradation for unparseable values, matching ical.js.
        Err(_) => match info.vtype {
            ValueType::Boolean => vec![json!(false)],
            ValueType::Float => vec![number(0.0)],
            ValueType::Integer => vec![json!(0)],
            _ => vec![json!(prop.value.clone())],
        },
    }
}

fn property_to_jcal(prop: &Property, dialect: Dialect) -> Json {
    let vcard = matches!(dialect, Dialect::VCard3 | Dialect::VCard4);
    let mut params = Map::new();

    // vCard groups become a "group" parameter; iCalendar (which has no
    // group concept) keeps the prefix as part of the name, matching ical.js.
    let mut name = prop.name.to_ascii_lowercase();
    if let Some(group) = &prop.group {
        if vcard {
            params.insert("group".into(), json!(group.to_ascii_lowercase()));
        } else {
            name = format!("{}.{}", group.to_ascii_lowercase(), name);
        }
    }

    for param in &prop.params {
        if param.name.eq_ignore_ascii_case("VALUE") {
            continue;
        }
        let key = param.name.to_ascii_lowercase();
        // vCard TYPE/PID/SORT-AS are comma lists even inside quoted values.
        let mut values: Vec<String> = param.values.clone();
        if vcard && matches!(key.as_str(), "type" | "pid" | "sort-as") {
            values = values
                .iter()
                .flat_map(|v| v.split(',').map(|s| s.to_string()))
                .collect();
        }
        let value = match values.len() {
            0 => json!(""),
            1 => json!(values[0]),
            _ => Json::Array(values.iter().map(|v| json!(v)).collect()),
        };
        params.insert(key, value);
    }

    let mut info = prop.type_info(dialect);
    // Date/date-time shape inference: when no explicit VALUE parameter is
    // present, the value's own shape decides between DATE and DATE-TIME.
    if prop.param_value("VALUE").is_none() {
        let has_time = prop.value.contains(['T', 't']);
        match info.vtype {
            ValueType::DateTime if !has_time => info.vtype = ValueType::Date,
            ValueType::Date if has_time => info.vtype = ValueType::DateTime,
            _ => {}
        }
    }

    let mut entry = vec![
        json!(name),
        Json::Object(params),
        json!(info.vtype.jcal_name()),
    ];
    entry.extend(value_to_json_slots(prop, info, dialect));
    Json::Array(entry)
}

/// Dialect appropriate for a top-level component.
pub fn detect_dialect(comp: &Component) -> Dialect {
    if comp.is("VCARD") {
        match comp.prop("VERSION").map(|p| p.value.trim()) {
            Some("2.1") | Some("3.0") => Dialect::VCard3,
            _ => Dialect::VCard4,
        }
    } else {
        Dialect::ICalendar
    }
}

/// Convert a component tree to jCal/jCard JSON.
pub fn component_to_jcal(comp: &Component, dialect: Dialect) -> Json {
    let props: Vec<Json> = comp
        .properties()
        .map(|p| property_to_jcal(p, dialect))
        .collect();
    let subs: Vec<Json> = comp
        .components()
        .map(|c| component_to_jcal(c, dialect))
        .collect();
    json!([comp.name.to_ascii_lowercase(), props, subs])
}

/// Convert a top-level component, auto-detecting the dialect.
pub fn to_jcal(comp: &Component) -> Json {
    component_to_jcal(comp, detect_dialect(comp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse, ParseOptions};

    fn first(input: &str) -> Component {
        parse(input, &ParseOptions::lenient())
            .unwrap()
            .components
            .remove(0)
    }

    #[test]
    fn simple_component() {
        let comp = first(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART:20260722T160000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let j = to_jcal(&comp);
        assert_eq!(
            j,
            serde_json::json!([
                "vcalendar",
                [["version", {}, "text", "2.0"]],
                [["vevent", [["dtstart", {}, "date-time", "2026-07-22T16:00:00Z"]], []]]
            ])
        );
    }

    #[test]
    fn value_param_consumed_into_type() {
        let comp = first("BEGIN:VCALENDAR\r\nDTSTART;VALUE=DATE:20260722\r\nEND:VCALENDAR\r\n");
        let j = to_jcal(&comp);
        assert_eq!(j[1][0], serde_json::json!(["dtstart", {}, "date", "2026-07-22"]));
    }

    #[test]
    fn group_becomes_param() {
        let comp = first("BEGIN:VCARD\r\nVERSION:4.0\r\nITEM1.EMAIL:a@b.c\r\nEND:VCARD\r\n");
        let j = to_jcal(&comp);
        assert_eq!(
            j[1][1],
            serde_json::json!(["email", {"group": "item1"}, "text", "a@b.c"])
        );
    }

    #[test]
    fn multivalued_param_is_array() {
        let comp = first(
            "BEGIN:VCALENDAR\r\nATTENDEE;MEMBER=\"mailto:a@b\",\"mailto:c@d\":mailto:x@y\r\nEND:VCALENDAR\r\n",
        );
        let j = to_jcal(&comp);
        assert_eq!(
            j[1][0][1],
            serde_json::json!({"member": ["mailto:a@b", "mailto:c@d"]})
        );
    }
}
