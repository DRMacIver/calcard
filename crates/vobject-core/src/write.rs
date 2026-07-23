//! Serialization of the model back to wire format.

use std::fmt;

use crate::contentline::is_lenient_name;
use crate::escape::{caret_encode, param_quoting, ParamQuoting};
use crate::fold::{fold_into, FOLD_WIDTH};
use crate::model::{Child, Component, Param, Property};

/// The model cannot be represented on the wire.
///
/// Only programmatically-built models can trip this: parsed models are
/// always writable (the parser never produces names with structural or
/// control characters, or values containing line breaks). Without these
/// checks an attacker-controlled string placed in a value would become an
/// arbitrary injected content line (e.g. `"x\r\nX-EVIL:y"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    pub message: String,
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WriteError {}

fn check_name(what: &str, name: &str) -> Result<(), WriteError> {
    if is_lenient_name(name) {
        return Ok(());
    }
    Err(WriteError {
        message: format!(
            "{what} {name:?} cannot be written unambiguously: it is empty or \
             contains structural or control characters"
        ),
    })
}

fn check_value(prop: &Property) -> Result<(), WriteError> {
    if prop.value.contains(['\r', '\n']) {
        return Err(WriteError {
            message: format!(
                "value of property {:?} contains a line break; encode it \
                 (e.g. TEXT values escape newlines as \\n) before writing",
                prop.name
            ),
        });
    }
    Ok(())
}

fn check_property(prop: &Property) -> Result<(), WriteError> {
    check_name("property name", &prop.name)?;
    if let Some(group) = &prop.group {
        check_name("group name", group)?;
    }
    for param in &prop.params {
        // Parameter values are caret-encoded on write and need no check.
        check_name("parameter name", &param.name)?;
    }
    check_value(prop)
}

#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// Line terminator; the RFCs require "\r\n".
    pub line_ending: String,
    /// Maximum octets per physical line, excluding the terminator.
    /// `None` disables folding entirely.
    pub fold_width: Option<usize>,
}

impl Default for WriteOptions {
    fn default() -> WriteOptions {
        WriteOptions {
            line_ending: "\r\n".to_string(),
            fold_width: Some(FOLD_WIDTH),
        }
    }
}

/// Serialize one property to a single (unfolded) content line, without
/// terminator.
pub fn property_line(prop: &Property) -> Result<String, WriteError> {
    check_property(prop)?;
    Ok(property_line_unchecked(prop))
}

fn property_line_unchecked(prop: &Property) -> String {
    let mut line = String::new();
    if let Some(group) = &prop.group {
        line.push_str(group);
        line.push('.');
    }
    line.push_str(&prop.name);
    for param in &prop.params {
        line.push(';');
        write_param(&mut line, param);
    }
    line.push(':');
    line.push_str(&prop.value);
    line
}

fn write_param(out: &mut String, param: &Param) {
    out.push_str(&param.name);
    if param.values.is_empty() {
        // A bare vCard 2.1 parameter round-trips as itself.
        return;
    }
    out.push('=');
    for (i, value) in param.values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let encoded = caret_encode(value);
        match param_quoting(&encoded) {
            ParamQuoting::Bare => out.push_str(&encoded),
            ParamQuoting::Quoted => {
                out.push('"');
                out.push_str(&encoded);
                out.push('"');
            }
        }
    }
}

fn emit_line(out: &mut String, line: &str, options: &WriteOptions) {
    match options.fold_width {
        Some(width) => fold_into(out, line, width, &options.line_ending),
        None => {
            out.push_str(line);
            out.push_str(&options.line_ending);
        }
    }
}

fn write_component_into(
    out: &mut String,
    comp: &Component,
    options: &WriteOptions,
) -> Result<(), WriteError> {
    check_name("component name", &comp.name)?;
    emit_line(out, &format!("BEGIN:{}", comp.name), options);
    for child in &comp.children {
        match child {
            Child::Property(p) => {
                check_property(p)?;
                emit_line(out, &property_line_unchecked(p), options);
            }
            Child::Component(c) => write_component_into(out, c, options)?,
        }
    }
    emit_line(out, &format!("END:{}", comp.name), options);
    Ok(())
}

/// Serialize a component (and its children) to wire format.
pub fn write_component(comp: &Component, options: &WriteOptions) -> Result<String, WriteError> {
    let mut out = String::new();
    write_component_into(&mut out, comp, options)?;
    Ok(out)
}

/// Serialize a sequence of top-level components.
pub fn write_document<'a>(
    components: impl IntoIterator<Item = &'a Component>,
    options: &WriteOptions,
) -> Result<String, WriteError> {
    let mut out = String::new();
    for comp in components {
        write_component_into(&mut out, comp, options)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Param, Property};
    use crate::parse::{parse, ParseOptions};

    #[test]
    fn simple_round_trip() {
        let input = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Hi\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let parsed = parse(input, &ParseOptions::strict()).unwrap();
        let out = write_document(&parsed.components, &WriteOptions::default()).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn property_line_with_everything() {
        let mut p = Property::new("EMAIL", "alice@example.com");
        p.group = Some("item1".into());
        p.params.push(Param {
            name: "TYPE".into(),
            values: vec!["work".into(), "pref".into()],
        });
        assert_eq!(
            property_line(&p).unwrap(),
            "item1.EMAIL;TYPE=work,pref:alice@example.com"
        );
    }

    #[test]
    fn param_needing_quotes() {
        let mut p = Property::new("DTSTART", "20260101T000000");
        p.params.push(Param::new("TZID", "US/Mountain: MST"));
        assert_eq!(
            property_line(&p).unwrap(),
            "DTSTART;TZID=\"US/Mountain: MST\":20260101T000000"
        );
    }

    #[test]
    fn param_with_quote_uses_caret_encoding() {
        let mut p = Property::new("X", "v");
        p.params.push(Param::new("A", "say \"hi\"\nok^"));
        assert_eq!(property_line(&p).unwrap(), "X;A=say ^'hi^'^nok^^:v");
    }

    #[test]
    fn bare_param_round_trips() {
        let mut p = Property::new("TEL", "+441234");
        p.params.push(Param::bare("HOME"));
        assert_eq!(property_line(&p).unwrap(), "TEL;HOME:+441234");
    }

    #[test]
    fn long_lines_are_folded() {
        let mut comp = Component::new("VCARD");
        comp.push_property(Property::new("NOTE", "x".repeat(200)));
        let out = write_component(&comp, &WriteOptions::default()).unwrap();
        for line in out.split("\r\n") {
            assert!(line.len() <= 75);
        }
        // And it parses back identically.
        let parsed = parse(&out, &ParseOptions::strict()).unwrap();
        assert_eq!(parsed.components[0], comp);
    }

    #[test]
    fn folding_can_be_disabled() {
        let mut comp = Component::new("VCARD");
        comp.push_property(Property::new("NOTE", "x".repeat(200)));
        let options = WriteOptions {
            fold_width: None,
            ..WriteOptions::default()
        };
        let out = write_component(&comp, &options).unwrap();
        assert!(out.split("\r\n").any(|l| l.len() > 75));
    }

    #[test]
    fn write_rejects_line_break_injection_in_values() {
        // A programmatic value containing CRLF would otherwise serialize
        // as an arbitrary injected content line.
        for value in ["line1\r\nX-EVIL:injected", "a\nEND:VCARD", "bare\rcr"] {
            let mut comp = Component::new("VCARD");
            comp.push_property(Property::new("SUMMARY", value));
            assert!(write_component(&comp, &WriteOptions::default()).is_err());
        }
    }

    #[test]
    fn write_rejects_structural_characters_in_names() {
        for name in ["BAD:NAME", "BAD;NAME", "BAD\nNAME", ""] {
            let mut comp = Component::new("VCARD");
            comp.push_property(Property::new(name, "v"));
            assert!(
                write_component(&comp, &WriteOptions::default()).is_err(),
                "{name:?}"
            );
        }
        // Group and parameter names are line structure too.
        let mut comp = Component::new("VCARD");
        let mut p = Property::new("X", "v");
        p.group = Some("g\r\nEVIL".into());
        comp.push_property(p);
        assert!(write_component(&comp, &WriteOptions::default()).is_err());

        let mut comp = Component::new("VCARD");
        let mut p = Property::new("X", "v");
        p.params.push(Param::new("A;B", "v"));
        comp.push_property(p);
        assert!(write_component(&comp, &WriteOptions::default()).is_err());

        assert!(write_component(&Component::new("V:EVIL"), &WriteOptions::default()).is_err());
    }

    #[test]
    fn model_round_trip_with_tricky_params() {
        let mut comp = Component::new("VCARD");
        let mut p = Property::new("X-TEST", "value");
        p.params.push(Param::new("A", "with,comma"));
        p.params.push(Param::new("B", "with\"quote"));
        p.params.push(Param::new("C", "with\nnewline"));
        p.params.push(Param::new("D", "with^caret"));
        p.params.push(Param::new("E", ""));
        comp.push_property(p);

        let out = write_component(&comp, &WriteOptions::default()).unwrap();
        let parsed = parse(&out, &ParseOptions::strict()).unwrap();
        assert!(parsed.repairs.is_empty());
        assert_eq!(parsed.components[0], comp);
    }
}
