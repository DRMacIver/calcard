//! xCal (RFC 6321) and xCard (RFC 6351) conversion.
//!
//! Writing reuses the jCal conversion (RFC 7265 §3.1 defines jCal as a
//! direct mapping of xCal, so the typed value model is shared) and renders
//! the XML shape: property elements named after the property, a
//! `<parameters>` block, and one element per value named after the value
//! type. Structured values use the RFCs' named child elements
//! (REQUEST-STATUS `code`/`description`/`data`, GEO
//! `latitude`/`longitude`, xCard N/ADR component names).
//!
//! Reading maps the XML back to the lossless document model, recording a
//! `VALUE` parameter whenever the value element's type differs from the
//! property's default, so wire round-trips preserve typing.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde_json::Value as Json;

use crate::escape::escape_text;
use crate::jcal;
use crate::model::{Component, Param, Property};
use crate::value::{default_type_info, Dialect, ValueType};

pub const XCAL_NS: &str = "urn:ietf:params:xml:ns:icalendar-2.0";
pub const XCARD_NS: &str = "urn:ietf:params:xml:ns:vcard-4.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    pub message: String,
}

impl XmlError {
    fn new(message: impl Into<String>) -> XmlError {
        XmlError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for XmlError {}

/// Named components for structured values, per RFC 6321 §3.6 / 6351 §3.4.
fn structured_field_names(prop_name: &str, dialect: Dialect) -> Option<&'static [&'static str]> {
    let vcard = matches!(dialect, Dialect::VCard4 | Dialect::VCard3);
    match prop_name.to_ascii_uppercase().as_str() {
        "REQUEST-STATUS" => Some(&["code", "description", "data"]),
        // vCard 4 GEO is a URI; iCalendar and vCard 3 use lat;lon.
        "GEO" if dialect != Dialect::VCard4 => Some(&["latitude", "longitude"]),
        "N" if vcard => Some(&["surname", "given", "additional", "prefix", "suffix"]),
        "ADR" if vcard => Some(&[
            "pobox", "ext", "street", "locality", "region", "code", "country",
        ]),
        "GENDER" if vcard => Some(&["sex", "identity"]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Writing

type W = Writer<Vec<u8>>;

/// xCal element names come from vobject names; only the RFC name grammar
/// — widened by '_' (a valid XML NameChar that real-world lenient data
/// uses, e.g. `oppo_recent_call`) and '.' for iCalendar group prefixes —
/// produces well-formed XML. Lenient wire parsing can retain names
/// outside it, which cannot be represented in xCal and must be rejected
/// rather than emitted broken.
fn check_name(name: &str) -> Result<(), XmlError> {
    let mut chars = name.chars();
    let valid = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_')
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(XmlError::new(format!(
            "name {name:?} cannot be represented as an XML element"
        )))
    }
}

/// XML 1.0 cannot carry most control characters even escaped.
fn check_text(text: &str) -> Result<(), XmlError> {
    match text
        .chars()
        .find(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        Some(c) => Err(XmlError::new(format!(
            "control character {c:?} cannot be represented in XML"
        ))),
        None => Ok(()),
    }
}

fn text_el(w: &mut W, name: &str, text: &str) -> Result<(), XmlError> {
    check_name(name)?;
    check_text(text)?;
    w.create_element(name)
        .write_text_content(BytesText::new(text))
        .map_err(|e| XmlError::new(e.to_string()))?;
    Ok(())
}

fn write_json_value(w: &mut W, name: &str, value: &Json) -> Result<(), XmlError> {
    match value {
        Json::String(s) => text_el(w, name, s),
        Json::Bool(b) => text_el(w, name, if *b { "true" } else { "false" }),
        Json::Number(n) => text_el(w, name, &n.to_string()),
        other => text_el(w, name, &other.to_string()),
    }
}

fn write_property(w: &mut W, prop_json: &Json, dialect: Dialect) -> Result<(), XmlError> {
    let entry = prop_json
        .as_array()
        .ok_or_else(|| XmlError::new("malformed jCal property"))?;
    let name = entry[0].as_str().unwrap_or_default();
    let params = entry[1].as_object();
    let type_name = entry[2].as_str().unwrap_or("unknown");
    let values = &entry[3..];

    check_name(name)?;
    w.write_event(Event::Start(BytesStart::new(name)))
        .map_err(|e| XmlError::new(e.to_string()))?;

    if let Some(params) = params {
        if !params.is_empty() {
            w.write_event(Event::Start(BytesStart::new("parameters")))
                .map_err(|e| XmlError::new(e.to_string()))?;
            for (pname, pvalue) in params {
                check_name(pname)?;
                w.write_event(Event::Start(BytesStart::new(pname)))
                    .map_err(|e| XmlError::new(e.to_string()))?;
                // Parameter values are text unless the RFC types them; text
                // is always accepted on read, so write text uniformly.
                match pvalue {
                    Json::Array(items) => {
                        for item in items {
                            write_json_value(w, "text", item)?;
                        }
                    }
                    other => write_json_value(w, "text", other)?,
                }
                w.write_event(Event::End(BytesEnd::new(pname)))
                    .map_err(|e| XmlError::new(e.to_string()))?;
            }
            w.write_event(Event::End(BytesEnd::new("parameters")))
                .map_err(|e| XmlError::new(e.to_string()))?;
        }
    }

    let field_names = structured_field_names(name, dialect);
    match (type_name, field_names) {
        ("recur", _) => {
            // One <recur> element with one child per part; list parts
            // repeat the element. An unparseable value travels as raw
            // element text (mirroring jCal's raw-string fallback).
            match values.first() {
                Some(Json::Object(obj)) => {
                    w.write_event(Event::Start(BytesStart::new("recur")))
                        .map_err(|e| XmlError::new(e.to_string()))?;
                    for (part, pv) in obj {
                        match pv {
                            Json::Array(items) => {
                                for item in items {
                                    write_json_value(w, part, item)?;
                                }
                            }
                            other => write_json_value(w, part, other)?,
                        }
                    }
                    w.write_event(Event::End(BytesEnd::new("recur")))
                        .map_err(|e| XmlError::new(e.to_string()))?;
                }
                Some(other) => write_json_value(w, "recur", other)?,
                None => text_el(w, "recur", "")?,
            }
        }
        ("period", _) => {
            for value in values {
                match value {
                    Json::Array(pair) => {
                        w.write_event(Event::Start(BytesStart::new("period")))
                            .map_err(|e| XmlError::new(e.to_string()))?;
                        text_el(w, "start", pair[0].as_str().unwrap_or_default())?;
                        let end = pair[1].as_str().unwrap_or_default();
                        if end.starts_with(['P', 'p', '+', '-']) {
                            text_el(w, "duration", end)?;
                        } else {
                            text_el(w, "end", end)?;
                        }
                        w.write_event(Event::End(BytesEnd::new("period")))
                            .map_err(|e| XmlError::new(e.to_string()))?;
                    }
                    // Raw fallback for unparseable period values.
                    other => write_json_value(w, "period", other)?,
                }
            }
        }
        (_, Some(fields)) => {
            // Structured value: the single jCal slot is an array (or a
            // collapsed scalar) whose entries map onto named elements;
            // multi-valued entries repeat the element.
            let slot = values.first().cloned().unwrap_or(Json::Null);
            let components: Vec<Json> = match slot {
                Json::Array(items) => items,
                other => vec![other],
            };
            for (i, component) in components.iter().enumerate() {
                let field = fields.get(i).copied().unwrap_or("text");
                match component {
                    Json::Array(items) => {
                        for item in items {
                            write_json_value(w, field, item)?;
                        }
                    }
                    other => write_json_value(w, field, other)?,
                }
            }
        }
        _ => {
            for value in values {
                write_json_value(w, type_name, value)?;
            }
        }
    }

    w.write_event(Event::End(BytesEnd::new(name)))
        .map_err(|e| XmlError::new(e.to_string()))?;
    Ok(())
}

fn write_component(
    w: &mut W,
    comp_json: &Json,
    dialect: Dialect,
    vcard: bool,
) -> Result<(), XmlError> {
    let entry = comp_json
        .as_array()
        .ok_or_else(|| XmlError::new("malformed jCal component"))?;
    let name = entry[0].as_str().unwrap_or_default();
    let props = entry[1].as_array().cloned().unwrap_or_default();
    let comps = entry[2].as_array().cloned().unwrap_or_default();

    check_name(name)?;
    w.write_event(Event::Start(BytesStart::new(name)))
        .map_err(|e| XmlError::new(e.to_string()))?;
    if vcard {
        // xCard has no <properties> wrapper.
        for p in &props {
            write_property(w, p, dialect)?;
        }
    } else {
        if !props.is_empty() {
            w.write_event(Event::Start(BytesStart::new("properties")))
                .map_err(|e| XmlError::new(e.to_string()))?;
            for p in &props {
                write_property(w, p, dialect)?;
            }
            w.write_event(Event::End(BytesEnd::new("properties")))
                .map_err(|e| XmlError::new(e.to_string()))?;
        }
        if !comps.is_empty() {
            w.write_event(Event::Start(BytesStart::new("components")))
                .map_err(|e| XmlError::new(e.to_string()))?;
            for c in &comps {
                write_component(w, c, dialect, false)?;
            }
            w.write_event(Event::End(BytesEnd::new("components")))
                .map_err(|e| XmlError::new(e.to_string()))?;
        }
    }
    w.write_event(Event::End(BytesEnd::new(name)))
        .map_err(|e| XmlError::new(e.to_string()))?;
    Ok(())
}

/// Serialize components to an xCal (`<icalendar>`) or xCard (`<vcards>`)
/// document, chosen by the components' kind. An XML document has a single
/// root, so a stream mixing vCards with iCalendar components cannot be
/// represented and is an error.
pub fn to_xml(components: &[Component]) -> Result<String, XmlError> {
    let vcard = components.first().is_some_and(|c| c.is("VCARD"));
    if components.iter().any(|c| c.is("VCARD") != vcard) {
        return Err(XmlError::new(
            "cannot mix vCard and iCalendar components in one xCal/xCard document",
        ));
    }
    let mut w = Writer::new(Vec::new());
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
        .map_err(|e| XmlError::new(e.to_string()))?;
    let (root, ns) = if vcard {
        ("vcards", XCARD_NS)
    } else {
        ("icalendar", XCAL_NS)
    };
    let mut root_el = BytesStart::new(root);
    root_el.push_attribute(("xmlns", ns));
    w.write_event(Event::Start(root_el))
        .map_err(|e| XmlError::new(e.to_string()))?;
    for comp in components {
        let dialect = jcal::detect_dialect(comp);
        let json = jcal::component_to_jcal(comp, dialect);
        write_component(&mut w, &json, dialect, vcard)?;
    }
    w.write_event(Event::End(BytesEnd::new(root)))
        .map_err(|e| XmlError::new(e.to_string()))?;
    String::from_utf8(w.into_inner()).map_err(|e| XmlError::new(e.to_string()))
}

// ---------------------------------------------------------------------------
// Reading

#[derive(Debug)]
struct XNode {
    name: String,
    text: String,
    children: Vec<XNode>,
}

/// Nesting cap for the XML reader, mirroring the wire parser's default
/// `max_depth` rationale: no real document nests anywhere near this, and an
/// uncapped depth bomb would blow the stack in the recursive tree walk and
/// node drop (a hard abort, not a catchable panic).
const MAX_XML_DEPTH: usize = 512;

fn parse_tree(xml: &str) -> Result<XNode, XmlError> {
    let mut reader = Reader::from_str(xml);
    // Text is kept verbatim: trimming would eat significant whitespace
    // around entity references (which arrive as separate events). Leaf
    // value text is used as-is; structural elements never read their text.
    let mut stack: Vec<XNode> = vec![XNode {
        name: String::new(),
        text: String::new(),
        children: Vec::new(),
    }];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if stack.len() > MAX_XML_DEPTH {
                    return Err(XmlError::new(
                        "element nesting exceeds the supported depth limit",
                    ));
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                stack.push(XNode {
                    name,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                stack.last_mut().unwrap().children.push(XNode {
                    name,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::End(_)) => {
                let done = stack.pop().unwrap();
                stack
                    .last_mut()
                    .ok_or_else(|| XmlError::new("unbalanced XML"))?
                    .children
                    .push(done);
            }
            Ok(Event::Text(t)) => {
                let text = t.decode().map_err(|e| XmlError::new(e.to_string()))?;
                stack.last_mut().unwrap().text.push_str(&text);
            }
            Ok(Event::GeneralRef(e)) => {
                let name = String::from_utf8_lossy(e.as_ref()).to_string();
                let resolved = match name.as_str() {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "amp" => Some('&'),
                    "apos" => Some('\''),
                    "quot" => Some('"'),
                    _ => e
                        .resolve_char_ref()
                        .map_err(|err| XmlError::new(err.to_string()))?,
                };
                match resolved {
                    Some(c) => stack.last_mut().unwrap().text.push(c),
                    None => {
                        return Err(XmlError::new(format!("unknown entity reference &{name};")))
                    }
                }
            }
            Ok(Event::CData(t)) => {
                stack
                    .last_mut()
                    .unwrap()
                    .text
                    .push_str(&String::from_utf8_lossy(&t));
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(XmlError::new(e.to_string())),
        }
    }
    let mut root = stack.pop().ok_or_else(|| XmlError::new("empty document"))?;
    if !stack.is_empty() {
        return Err(XmlError::new("unbalanced XML"));
    }
    if root.children.len() != 1 {
        return Err(XmlError::new("expected a single root element"));
    }
    Ok(root.children.remove(0))
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

/// Convert a value element's dashed/coloned text back to wire form.
fn value_text_to_wire(type_name: &str, text: &str, dialect: Dialect) -> String {
    match type_name {
        // vCard 3.0's own wire format is the extended (dashed) ISO form,
        // so its date family passes through verbatim.
        "date" | "date-time" | "time" | "timestamp" | "date-and-or-time"
            if dialect == Dialect::VCard3 =>
        {
            text.to_string()
        }
        "date" | "date-time" | "time" | "timestamp" | "date-and-or-time" => {
            // Unparseable wire values travel through jCal/xCal as raw
            // strings; only transform text that is shaped like a jCal
            // date/time (digits and date-time punctuation alone).
            let looks_like_datetime = !text.is_empty()
                && text.chars().all(|c| {
                    c.is_ascii_digit() || matches!(c, 'T' | 't' | 'Z' | 'z' | ':' | '+' | '-')
                });
            if !looks_like_datetime {
                return text.to_string();
            }
            match text.split_once(['T', 't']) {
                // Colons in a time part are only separators; dashes there
                // are zone signs or truncation markers and must survive.
                Some((d, t)) => format!("{}T{}", compact_date(d), t.replace(':', "")),
                None => {
                    if text.contains(':') {
                        // A bare time value.
                        text.replace(':', "")
                    } else {
                        compact_date(text)
                    }
                }
            }
        }
        "utc-offset" => text.replace(':', ""),
        "text" | "cal-address" | "uri" | "phone-number" | "vcard" => escape_text(text),
        // Unknown values are raw wire text in both directions (RFC 7265
        // §5); escaping them would double-escape.
        "unknown" => text.to_string(),
        "binary" | "duration" | "boolean" | "integer" | "float" | "language-tag" => {
            text.to_string()
        }
        _ => text.to_string(),
    }
}

fn recur_node_to_wire(node: &XNode) -> String {
    let mut parts: Vec<String> = Vec::new();
    // Group repeated elements (e.g. several <byday>) into one comma list.
    let mut i = 0;
    while i < node.children.len() {
        let name = node.children[i].name.to_ascii_uppercase();
        let mut values = vec![node.children[i].text.clone()];
        let mut j = i + 1;
        while j < node.children.len() && node.children[j].name.to_ascii_uppercase() == name {
            values.push(node.children[j].text.clone());
            j += 1;
        }
        let wire_name = name.replace('-', "");
        let value = if wire_name == "UNTIL" {
            values.join(",").replace(['-', ':'], "")
        } else {
            values.join(",")
        };
        parts.push(format!("{wire_name}={value}"));
        i = j;
    }
    let joined = parts.join(";");
    // Canonicalize part order (XML carries no reliable ordering).
    match crate::value::Recur::parse(&joined) {
        Ok(r) => r.to_string(),
        Err(_) => joined,
    }
}

fn property_from_node(node: &XNode, dialect: Dialect) -> Result<Property, XmlError> {
    // iCalendar-side groups are written as a dotted element name
    // (`item1.x-email`); split it back apart. (vCard groups travel in the
    // `group` parameter instead and override below.)
    let (element_group, element_name) = match node.name.split_once('.') {
        Some((g, n)) => (Some(g.to_string()), n),
        None => (None, node.name.as_str()),
    };
    let name = element_name.to_ascii_uppercase();
    let mut params: Vec<Param> = Vec::new();
    let mut group = element_group;

    let mut value_nodes: Vec<&XNode> = Vec::new();
    for child in &node.children {
        if child.name == "parameters" {
            for pnode in &child.children {
                let pname = pnode.name.to_ascii_uppercase();
                let values: Vec<String> = if pnode.children.is_empty() {
                    vec![pnode.text.clone()]
                } else {
                    pnode.children.iter().map(|v| v.text.clone()).collect()
                };
                if pname == "GROUP" {
                    group = values.into_iter().next();
                } else {
                    params.push(Param {
                        name: pname,
                        values,
                    });
                }
            }
        } else {
            value_nodes.push(child);
        }
    }

    if value_nodes.is_empty() {
        return Err(XmlError::new(format!("property {name} has no value")));
    }

    let default = default_type_info(&name, dialect);
    let first_type = value_nodes[0].name.as_str();

    let raw = if first_type == "recur" {
        if value_nodes[0].children.is_empty() {
            // Raw fallback carried as element text.
            value_nodes[0].text.clone()
        } else {
            recur_node_to_wire(value_nodes[0])
        }
    } else if first_type == "period" {
        let mut periods = Vec::new();
        for pnode in &value_nodes {
            if pnode.children.is_empty() {
                periods.push(pnode.text.clone());
                continue;
            }
            let start = pnode
                .children
                .iter()
                .find(|c| c.name == "start")
                .map(|c| c.text.replace(['-', ':'], ""))
                .unwrap_or_default();
            let end = pnode
                .children
                .iter()
                .find(|c| c.name == "end" || c.name == "duration")
                .map(|c| {
                    if c.name == "end" {
                        c.text.replace(['-', ':'], "")
                    } else {
                        c.text.clone()
                    }
                })
                .unwrap_or_default();
            periods.push(format!("{start}/{end}"));
        }
        periods.join(",")
    } else if let Some(fields) = structured_field_names(&node.name, dialect) {
        // Structured: gather by field name, preserving field order. Only
        // fields up to the last one actually present in the XML become
        // components (absent trailing fields were never written; N/ADR
        // writers emit their empty components explicitly, so full width
        // survives).
        let mut by_field: Vec<Vec<String>> = vec![Vec::new(); fields.len()];
        let mut last_present = 0;
        for vnode in &value_nodes {
            if let Some(idx) = fields.iter().position(|f| *f == vnode.name) {
                by_field[idx].push(vnode.text.clone());
                last_present = last_present.max(idx);
            }
        }
        by_field.truncate(last_present + 1);
        by_field
            .iter()
            .map(|values| {
                values
                    .iter()
                    .map(|v| escape_text(v))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join(";")
    } else {
        value_nodes
            .iter()
            .map(|v| value_text_to_wire(first_type, &v.text, dialect))
            .collect::<Vec<_>>()
            .join(",")
    };

    // Record the value type when it differs from the property's default
    // (structured values infer their type from the named field elements
    // instead of a type-named element).
    let declared = ValueType::from_name(&first_type.to_ascii_uppercase().replace('_', "-"));
    if let Some(declared) = declared {
        if declared != default.vtype && structured_field_names(&node.name, dialect).is_none() {
            params.push(Param::new("VALUE", first_type.to_ascii_uppercase()));
        }
    }

    Ok(Property {
        group,
        name,
        params,
        value: raw,
    })
}

fn component_from_node(node: &XNode, dialect: Dialect) -> Result<Component, XmlError> {
    let mut comp = Component::new(node.name.to_ascii_uppercase());
    for child in &node.children {
        match child.name.as_str() {
            "properties" => {
                for p in &child.children {
                    comp.push_property(property_from_node(p, dialect)?);
                }
            }
            "components" => {
                for c in &child.children {
                    comp.push_component(component_from_node(c, dialect)?);
                }
            }
            // xCard: properties sit directly under <vcard>.
            _ => comp.push_property(property_from_node(child, dialect)?),
        }
    }
    Ok(comp)
}

/// Parse an xCal (`<icalendar>`) or xCard (`<vcards>`) document.
pub fn from_xml(xml: &str) -> Result<Vec<Component>, XmlError> {
    let root = parse_tree(xml)?;
    match root.name.as_str() {
        "icalendar" => root
            .children
            .iter()
            .map(|c| component_from_node(c, Dialect::ICalendar))
            .collect(),
        "vcards" => root
            .children
            .iter()
            .map(|c| {
                // The card's own VERSION decides which property-type
                // registry applies.
                let version = c
                    .children
                    .iter()
                    .find(|n| n.name == "version")
                    .and_then(|n| n.children.first())
                    .map(|v| v.text.trim().to_string());
                let dialect = match version.as_deref() {
                    Some("2.1") | Some("3.0") => Dialect::VCard3,
                    _ => Dialect::VCard4,
                };
                component_from_node(c, dialect)
            })
            .collect(),
        other => Err(XmlError::new(format!(
            "expected icalendar or vcards root, found {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse, ParseOptions};

    fn components(input: &str) -> Vec<Component> {
        parse(input, &ParseOptions::lenient()).unwrap().components
    }

    #[test]
    fn rfc6321_style_document() {
        let comps = components(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//x//EN\r\nBEGIN:VEVENT\r\nUID:a1\r\nDTSTART;TZID=US/Eastern:20060102T120000\r\nDURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        let xml = to_xml(&comps).unwrap();
        assert!(xml.contains("<icalendar xmlns=\"urn:ietf:params:xml:ns:icalendar-2.0\">"));
        assert!(xml.contains("<dtstart><parameters><tzid><text>US/Eastern</text></tzid></parameters><date-time>2006-01-02T12:00:00</date-time></dtstart>"));
        assert!(xml.contains("<recur><freq>DAILY</freq><count>5</count></recur>"));

        let back = from_xml(&xml).unwrap();
        assert_eq!(back, comps);
    }

    #[test]
    fn xcard_document() {
        let comps = components(
            "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Alice Example\r\nN:Example;Alice;;;\r\nITEM1.EMAIL:a@b.c\r\nEND:VCARD\r\n",
        );
        let xml = to_xml(&comps).unwrap();
        assert!(xml.contains("<vcards xmlns=\"urn:ietf:params:xml:ns:vcard-4.0\">"));
        assert!(xml.contains("<n><surname>Example</surname><given>Alice</given><additional></additional><prefix></prefix><suffix></suffix></n>"));
        assert!(xml.contains("<group><text>item1</text></group>"));

        let back = from_xml(&xml).unwrap();
        // Group casing normalizes to lowercase through xCard.
        let mut expected = comps.clone();
        for p in expected[0].properties_mut() {
            if let Some(g) = &p.group {
                p.group = Some(g.to_ascii_lowercase());
            }
        }
        assert_eq!(back, expected);
    }

    #[test]
    fn value_type_round_trips_via_value_param() {
        let comps =
            components("BEGIN:VCALENDAR\r\nDTSTART;VALUE=DATE:20260722\r\nEND:VCALENDAR\r\n");
        let xml = to_xml(&comps).unwrap();
        assert!(xml.contains("<date>2026-07-22</date>"));
        let back = from_xml(&xml).unwrap();
        assert_eq!(
            back[0].prop("DTSTART").unwrap().param_value("VALUE"),
            Some("DATE")
        );
        assert_eq!(back[0].prop("DTSTART").unwrap().value, "20260722");
    }

    #[test]
    fn structured_and_multivalued() {
        let comps = components(
            "BEGIN:VCALENDAR\r\nREQUEST-STATUS:2.0;Success\r\nCATEGORIES:one,two\\,half\r\nGEO:37.386013;-122.082932\r\nFREEBUSY:19970101T180000Z/PT1H\r\nEND:VCALENDAR\r\n",
        );
        let xml = to_xml(&comps).unwrap();
        assert!(xml.contains("<code>2.0</code><description>Success</description>"));
        assert!(xml.contains("<text>one</text><text>two,half</text>"));
        assert!(xml.contains("<latitude>37.386013</latitude><longitude>-122.082932</longitude>"));
        assert!(xml.contains(
            "<period><start>1997-01-01T18:00:00Z</start><duration>PT1H</duration></period>"
        ));

        let back = from_xml(&xml).unwrap();
        assert_eq!(back, comps);
    }

    #[test]
    fn rejects_garbage() {
        assert!(from_xml("").is_err());
        assert!(from_xml("<unbalanced>").is_err());
        assert!(from_xml("<other/>").is_err());
    }
}
