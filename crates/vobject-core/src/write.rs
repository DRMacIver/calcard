//! Serialization of the model back to wire format.

use crate::escape::{caret_encode, param_quoting, ParamQuoting};
use crate::fold::{fold_into, FOLD_WIDTH};
use crate::model::{Child, Component, Param, Property};

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
pub fn property_line(prop: &Property) -> String {
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

fn write_component_into(out: &mut String, comp: &Component, options: &WriteOptions) {
    emit_line(out, &format!("BEGIN:{}", comp.name), options);
    for child in &comp.children {
        match child {
            Child::Property(p) => emit_line(out, &property_line(p), options),
            Child::Component(c) => write_component_into(out, c, options),
        }
    }
    emit_line(out, &format!("END:{}", comp.name), options);
}

/// Serialize a component (and its children) to wire format.
pub fn write_component(comp: &Component, options: &WriteOptions) -> String {
    let mut out = String::new();
    write_component_into(&mut out, comp, options);
    out
}

/// Serialize a sequence of top-level components.
pub fn write_document<'a>(
    components: impl IntoIterator<Item = &'a Component>,
    options: &WriteOptions,
) -> String {
    let mut out = String::new();
    for comp in components {
        write_component_into(&mut out, comp, options);
    }
    out
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
        let out = write_document(&parsed.components, &WriteOptions::default());
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
            property_line(&p),
            "item1.EMAIL;TYPE=work,pref:alice@example.com"
        );
    }

    #[test]
    fn param_needing_quotes() {
        let mut p = Property::new("DTSTART", "20260101T000000");
        p.params.push(Param::new("TZID", "US/Mountain: MST"));
        assert_eq!(
            property_line(&p),
            "DTSTART;TZID=\"US/Mountain: MST\":20260101T000000"
        );
    }

    #[test]
    fn param_with_quote_uses_caret_encoding() {
        let mut p = Property::new("X", "v");
        p.params.push(Param::new("A", "say \"hi\"\nok^"));
        assert_eq!(property_line(&p), "X;A=say ^'hi^'^nok^^:v");
    }

    #[test]
    fn bare_param_round_trips() {
        let mut p = Property::new("TEL", "+441234");
        p.params.push(Param::bare("HOME"));
        assert_eq!(property_line(&p), "TEL;HOME:+441234");
    }

    #[test]
    fn long_lines_are_folded() {
        let mut comp = Component::new("VCARD");
        comp.push_property(Property::new("NOTE", "x".repeat(200)));
        let out = write_component(&comp, &WriteOptions::default());
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
        let out = write_component(&comp, &options);
        assert!(out.split("\r\n").any(|l| l.len() > 75));
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

        let out = write_component(&comp, &WriteOptions::default());
        let parsed = parse(&out, &ParseOptions::strict()).unwrap();
        assert!(parsed.repairs.is_empty());
        assert_eq!(parsed.components[0], comp);
    }
}
