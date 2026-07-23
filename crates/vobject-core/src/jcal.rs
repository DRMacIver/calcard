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

use crate::escape::escape_text;
use crate::model::{Component, Param, Property};
use crate::value::{
    self, default_type_info, Date, DateOrDateTime, DateTime, Dialect, Multiplicity, Time, TypeInfo,
    Until, Value, ValueType,
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
    format!("{}T{}", json_date(&dt.date), json_time(&dt.time))
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
    if matches!(info.vtype, ValueType::DateAndOrTime | ValueType::Timestamp) {
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
            // component directly (GENDER:M → "M") — but only when the lone
            // component is a scalar: a single component holding a comma
            // list (N:Philip,Paul) keeps its wrapping array so it stays
            // distinguishable from a two-component value.
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
            if rendered.len() == 1 && !matches!(rendered[0], Json::Array(_)) {
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
    // jCal/jCard allow arbitrary value type names (RFC 7265 §3.5, RFC 7095
    // §3.4): an unrecognized VALUE parameter travels as the type string,
    // with unknown (raw) semantics, rather than being silently dropped.
    let mut type_label: Option<String> = None;
    if let Some(vp) = prop.param_value("VALUE") {
        if !vp.is_empty() && ValueType::from_name(vp).is_none() {
            info = TypeInfo {
                vtype: ValueType::Unknown,
                multiplicity: Multiplicity::Single,
            };
            type_label = Some(vp.to_ascii_lowercase());
        }
    }
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
        json!(type_label.as_deref().unwrap_or(info.vtype.jcal_name())),
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

// ---------------------------------------------------------------------------
// Reading

/// Maximum component nesting depth accepted when reading jCal, matching the
/// wire parser's cap (fuzz inputs nest tens of thousands of levels deep; a
/// recursive walk without a cap is a stack-overflow abort). String input is
/// additionally bounded earlier by serde_json's own recursion limit (128),
/// which this cap backstops for callers passing pre-built values.
pub const MAX_DEPTH: usize = 512;

/// An error reading a jCal/jCard document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JcalError {
    pub message: String,
}

impl JcalError {
    fn new(message: impl Into<String>) -> JcalError {
        JcalError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JcalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for JcalError {}

/// Parse a jCal (RFC 7265) / jCard (RFC 7095) document, accepting the
/// dialect [`to_jcal`] writes. The top level may be a single document
/// (`["vcalendar", props, comps]`, or jCard's two-element
/// `["vcard", props]`) or an array of several such documents.
///
/// Reading maps the JSON back to the lossless document model, recording a
/// `VALUE` parameter whenever the value type could not otherwise be
/// reproduced, so round-trips preserve typing. The writer's documented
/// degradations (unparseable values carried as raw strings) are read back
/// verbatim.
pub fn from_jcal(json: &str) -> Result<Vec<Component>, JcalError> {
    let value: Json =
        serde_json::from_str(json).map_err(|e| JcalError::new(format!("invalid JSON: {e}")))?;
    from_jcal_value(&value)
}

/// [`from_jcal`] on an already-parsed JSON value.
pub fn from_jcal_value(value: &Json) -> Result<Vec<Component>, JcalError> {
    let arr = value
        .as_array()
        .ok_or_else(|| JcalError::new("expected a jCal array"))?;
    match arr.first() {
        Some(Json::String(_)) => Ok(vec![document_component(value)?]),
        Some(Json::Array(_)) => arr.iter().map(document_component).collect(),
        Some(other) => Err(JcalError::new(format!(
            "expected a component name or a nested document, found {other}"
        ))),
        None => Err(JcalError::new("empty jCal document")),
    }
}

/// Parse one top-level component, choosing the dialect the way the writer
/// does ([`detect_dialect`]): a vCard's own VERSION decides which
/// property-type registry applies.
fn document_component(v: &Json) -> Result<Component, JcalError> {
    let arr = v
        .as_array()
        .ok_or_else(|| JcalError::new("malformed jCal component"))?;
    let name = arr.first().and_then(Json::as_str).unwrap_or_default();
    let dialect = if name.eq_ignore_ascii_case("vcard") {
        let version = arr.get(1).and_then(Json::as_array).and_then(|props| {
            props.iter().find_map(|p| {
                let entry = p.as_array()?;
                if entry.first()?.as_str()?.eq_ignore_ascii_case("version") {
                    entry.get(3).map(json_scalar_to_string)
                } else {
                    None
                }
            })
        });
        match version.as_deref().map(str::trim) {
            Some("2.1") | Some("3.0") => Dialect::VCard3,
            _ => Dialect::VCard4,
        }
    } else {
        Dialect::ICalendar
    };
    component_from_json(v, dialect, 0)
}

fn component_from_json(v: &Json, dialect: Dialect, depth: usize) -> Result<Component, JcalError> {
    if depth > MAX_DEPTH {
        return Err(JcalError::new("jCal components nested too deeply"));
    }
    let arr = v
        .as_array()
        .ok_or_else(|| JcalError::new("malformed jCal component"))?;
    // jCal components are [name, properties, components]; RFC 7095 jCard
    // omits the (always empty) subcomponent array.
    if !(2..=3).contains(&arr.len()) {
        return Err(JcalError::new(
            "malformed jCal component: expected [name, properties, components]",
        ));
    }
    let name = arr[0]
        .as_str()
        .ok_or_else(|| JcalError::new("component name must be a string"))?;
    let props = arr[1]
        .as_array()
        .ok_or_else(|| JcalError::new("component properties must be an array"))?;
    let mut comp = Component::new(name.to_ascii_uppercase());
    for p in props {
        comp.push_property(property_from_json(p, dialect)?);
    }
    if let Some(subs) = arr.get(2) {
        let subs = subs
            .as_array()
            .ok_or_else(|| JcalError::new("component subcomponents must be an array"))?;
        for c in subs {
            comp.push_component(component_from_json(c, dialect, depth + 1)?);
        }
    }
    Ok(comp)
}

fn json_scalar_to_string(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.to_string(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

fn property_from_json(v: &Json, dialect: Dialect) -> Result<Property, JcalError> {
    let entry = v
        .as_array()
        .ok_or_else(|| JcalError::new("malformed jCal property"))?;
    if entry.len() < 3 {
        return Err(JcalError::new(
            "malformed jCal property: expected [name, params, type, value…]",
        ));
    }
    let raw_name = entry[0]
        .as_str()
        .ok_or_else(|| JcalError::new("property name must be a string"))?;
    let params_obj = entry[1]
        .as_object()
        .ok_or_else(|| JcalError::new("property parameters must be an object"))?;
    let type_name = entry[2]
        .as_str()
        .ok_or_else(|| JcalError::new("property value type must be a string"))?;
    let slots = &entry[3..];
    if slots.is_empty() {
        return Err(JcalError::new(format!(
            "property {raw_name:?} has no value"
        )));
    }

    let vcard = matches!(dialect, Dialect::VCard3 | Dialect::VCard4);

    // vCard groups travel as a "group" parameter; iCalendar spells them as
    // a name prefix (both matching the writer).
    let mut group = None;
    let name = if vcard {
        raw_name.to_ascii_uppercase()
    } else {
        match raw_name.split_once('.') {
            Some((g, rest)) => {
                group = Some(g.to_string());
                rest.to_ascii_uppercase()
            }
            None => raw_name.to_ascii_uppercase(),
        }
    };

    let mut params: Vec<Param> = Vec::new();
    for (key, pvalue) in params_obj {
        if vcard && key.eq_ignore_ascii_case("group") {
            let scalar = match pvalue {
                Json::Array(items) => items.first().unwrap_or(&Json::Null),
                other => other,
            };
            group = Some(json_scalar_to_string(scalar));
            continue;
        }
        let values = match pvalue {
            Json::Array(items) => items.iter().map(json_scalar_to_string).collect(),
            // "" is how the writer spells a bare (valueless) param
            // (TEL;HOME). A genuinely empty single value (TEL;HOME=:)
            // writes the same "", so the two wire forms are ambiguous in
            // jCal; we read the one whose wire serialization round-trips
            // through this library ("HOME" re-parses as bare, whereas
            // reading [""] would serialize as "HOME=:").
            Json::String(s) if s.is_empty() => Vec::new(),
            other => vec![json_scalar_to_string(other)],
        };
        params.push(Param {
            name: key.to_ascii_uppercase(),
            values,
        });
    }

    let default = default_type_info(&name, dialect);
    let declared = ValueType::from_name(type_name);
    // An unrecognized type name reads its values as raw wire text.
    let effective = declared.unwrap_or(ValueType::Unknown);

    let mut force_value_param = false;
    let raw = raw_from_slots(slots, effective, default, dialect, &mut force_value_param);

    // Record the value type when the writer would not reproduce it from the
    // property alone: it differs from the registry default, or writing
    // without a VALUE parameter would shape-infer a different date type.
    if let Some(declared) = declared {
        let has_time = raw.contains(['T', 't']);
        let inferred = match default.vtype {
            ValueType::DateTime if !has_time => ValueType::Date,
            ValueType::Date if has_time => ValueType::DateTime,
            v => v,
        };
        if declared != default.vtype || declared != inferred || force_value_param {
            params.push(Param::new("VALUE", type_name.to_ascii_uppercase()));
        }
    } else if !type_name.is_empty() {
        // An unrecognized type name is the writer's spelling of an
        // unrecognized VALUE parameter; record it so re-writing reproduces
        // the same type string.
        params.push(Param::new("VALUE", type_name.to_ascii_uppercase()));
    }

    Ok(Property {
        group,
        name,
        params,
        value: raw,
    })
}

fn join_slots(slots: &[Json], f: impl Fn(&Json) -> String) -> String {
    slots.iter().map(f).collect::<Vec<_>>().join(",")
}

/// Reconstruct the raw wire value from the jCal value slots. The inverse of
/// [`value_to_json_slots`], on that function's image; unrecognized shapes
/// degrade to their verbatim text so reading stays total.
fn raw_from_slots(
    slots: &[Json],
    effective: ValueType,
    default: TypeInfo,
    dialect: Dialect,
    force_value_param: &mut bool,
) -> String {
    use ValueType::*;
    let structured = matches!(default.multiplicity, Multiplicity::Structured { .. });
    match effective {
        Text | PhoneNumber | Vcard if structured => structured_text_raw(&slots[0]),
        // iCalendar URIs (RFC 5545 §3.3.13) have no backslash escaping;
        // vCard values, URIs included, escape commas (RFC 6350 §3.4).
        Uri | CalAddress if dialect == Dialect::ICalendar => {
            join_slots(slots, json_scalar_to_string)
        }
        Text | PhoneNumber | Vcard | Uri | CalAddress => {
            join_slots(slots, |s| escape_text(&json_scalar_to_string(s)))
        }
        Date | DateTime | Time | Timestamp | DateAndOrTime => join_slots(slots, |s| match s {
            Json::String(text) => date_slot_to_wire(text, effective, dialect),
            other => json_scalar_to_string(other),
        }),
        UtcOffset => join_slots(slots, |s| match s {
            Json::String(text) => verified_wire(text, text.replace(':', ""), effective, dialect),
            other => json_scalar_to_string(other),
        }),
        Boolean => match &slots[0] {
            Json::Bool(true) => "TRUE".to_string(),
            Json::Bool(false) => "FALSE".to_string(),
            other => json_scalar_to_string(other),
        },
        Float if structured && default.vtype == Float => match &slots[0] {
            Json::Array(items) => items
                .iter()
                .map(float_slot_to_string)
                .collect::<Vec<_>>()
                .join(";"),
            n @ Json::Number(_) => {
                // A bare number where the registry expects lat;lon means the
                // writer saw an explicit VALUE=FLOAT (single multiplicity);
                // record it so re-writing takes the same path.
                *force_value_param = true;
                float_slot_to_string(n)
            }
            other => json_scalar_to_string(other),
        },
        Float => join_slots(slots, float_slot_to_string),
        Integer | Binary | Duration | LanguageTag | Unknown => {
            join_slots(slots, json_scalar_to_string)
        }
        Period => join_slots(slots, period_slot_to_wire),
        Recur => recur_slot_to_wire(&slots[0]),
    }
}

/// One structured (semicolon-separated) text value, undoing the writer's
/// collapses: a bare string is a singleton component of a single-component
/// value; array entries are components, themselves strings or comma lists.
fn structured_text_raw(slot: &Json) -> String {
    fn component(entry: &Json) -> String {
        match entry {
            Json::Array(items) => items
                .iter()
                .map(|i| escape_text(&json_scalar_to_string(i)))
                .collect::<Vec<_>>()
                .join(","),
            other => escape_text(&json_scalar_to_string(other)),
        }
    }
    match slot {
        Json::Array(entries) => entries.iter().map(component).collect::<Vec<_>>().join(";"),
        other => component(other),
    }
}

fn float_slot_to_string(v: &Json) -> String {
    match v {
        Json::Number(n) => value::format_float(n.as_f64().unwrap_or(0.0)),
        other => json_scalar_to_string(other),
    }
}

fn period_slot_to_wire(v: &Json) -> String {
    match v {
        Json::Array(pair) => {
            let start = pair
                .first()
                .map(json_scalar_to_string)
                .unwrap_or_default()
                .replace(['-', ':'], "");
            let end = pair.get(1).map(json_scalar_to_string).unwrap_or_default();
            let end = if end.starts_with(['P', 'p', '+', '-']) {
                end
            } else {
                end.replace(['-', ':'], "")
            };
            format!("{start}/{end}")
        }
        // Raw fallback for unparseable period values.
        other => json_scalar_to_string(other),
    }
}

fn recur_slot_to_wire(v: &Json) -> String {
    let obj = match v {
        Json::Object(obj) => obj,
        // Raw fallback for unparseable recur values.
        other => return json_scalar_to_string(other),
    };
    let mut parts: Vec<String> = Vec::new();
    for (key, pv) in obj {
        let name = key.to_ascii_uppercase();
        let joined = match pv {
            Json::Array(items) => items
                .iter()
                .map(json_scalar_to_string)
                .collect::<Vec<_>>()
                .join(","),
            other => json_scalar_to_string(other),
        };
        let value = if name == "UNTIL" {
            joined.replace(['-', ':'], "")
        } else {
            joined
        };
        parts.push(format!("{name}={value}"));
    }
    let wire = parts.join(";");
    // Canonicalize part order (the model's canonical form).
    match value::Recur::parse(&wire) {
        Ok(r) => r.to_string(),
        Err(_) => wire,
    }
}

/// Compact a dashed date, respecting vCard truncation forms: leading
/// dashes are markers (`--02-03` → `--0203`), and `1985-04` keeps its
/// dash per RFC 6350. Unrecognized shapes pass through.
fn compact_date(d: &str) -> String {
    let b = d.as_bytes();
    if d.len() == 10 && b[4] == b'-' && b[7] == b'-' {
        format!("{}{}{}", &d[0..4], &d[5..7], &d[8..10])
    } else if d.len() == 7 && d.starts_with("--") && b[4] == b'-' {
        format!("--{}{}", &d[2..4], &d[5..7])
    } else {
        d.to_string()
    }
}

/// Convert a date-family value slot back to wire form. vCard 3.0's own
/// wire format is the extended (dashed) ISO form, so its date family
/// passes through verbatim; elsewhere the dashed/coloned jCal text is
/// compacted, but only when the writer would render the compacted value
/// back to exactly this slot — unparseable wire values travel through
/// jCal as raw strings and must return verbatim.
pub(crate) fn date_slot_to_wire(text: &str, vtype: ValueType, dialect: Dialect) -> String {
    if dialect == Dialect::VCard3 {
        return text.to_string();
    }
    let looks_like_datetime = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'T' | 't' | 'Z' | 'z' | ':' | '+' | '-'));
    if !looks_like_datetime {
        return text.to_string();
    }
    let candidate = match text.split_once(['T', 't']) {
        // Colons in a time part are only separators; dashes there are zone
        // signs or truncation markers and must survive.
        Some((d, t)) => format!("{}T{}", compact_date(d), t.replace(':', "")),
        None => {
            if text.contains(':') {
                // A bare time value.
                text.replace(':', "")
            } else {
                compact_date(text)
            }
        }
    };
    verified_wire(text, candidate, vtype, dialect)
}

/// Accept `candidate` as the wire form of a slot only if the writer would
/// render it back to exactly `text`; otherwise keep the slot text verbatim
/// (the raw-fallback path, which the writer reproduces as-is).
pub(crate) fn verified_wire(
    text: &str,
    candidate: String,
    vtype: ValueType,
    dialect: Dialect,
) -> String {
    if candidate == text {
        return candidate;
    }
    let probe = Property::new("X-CHECK", candidate.clone());
    let info = TypeInfo {
        vtype,
        multiplicity: Multiplicity::Single,
    };
    let slots = value_to_json_slots(&probe, info, dialect);
    if slots.len() == 1 && slots[0].as_str() == Some(text) {
        candidate
    } else {
        text.to_string()
    }
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
                [[
                    "vevent",
                    [["dtstart", {}, "date-time", "2026-07-22T16:00:00Z"]],
                    []
                ]]
            ])
        );
    }

    #[test]
    fn value_param_consumed_into_type() {
        let comp = first("BEGIN:VCALENDAR\r\nDTSTART;VALUE=DATE:20260722\r\nEND:VCALENDAR\r\n");
        let j = to_jcal(&comp);
        assert_eq!(
            j[1][0],
            serde_json::json!(["dtstart", {}, "date", "2026-07-22"])
        );
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
