//! Content-line parsing: one logical line → a [`Property`].
//!
//! Grammar (RFC 5545 §3.1 / RFC 6350 §3.3):
//!
//! ```text
//! contentline = [group "."] name *(";" param) ":" value
//! param       = param-name "=" param-value *("," param-value)
//! param-value = paramtext / DQUOTE quoted-string DQUOTE
//! ```
//!
//! Lenient mode additionally accepts vCard 2.1 bare parameters
//! (`TEL;HOME;VOICE:...`), stray or unterminated quotes, control characters,
//! and unusual property names, recording a [`Repair`] for each.

use crate::error::{ErrorKind, Location, ParseError, Repair, RepairKind};
use crate::escape::caret_decode;
use crate::model::{Param, Property};

/// Is this a valid strict-grammar name (property, group, or parameter)?
fn is_strict_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Lenient names: anything non-empty without structural or control
/// characters. Underscores in particular occur in the wild (Lotus Notes).
fn is_lenient_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| !c.is_control() && !matches!(c, ';' | ':' | '=' | ',' | '"'))
}

struct LineParser<'a, 'r> {
    rest: &'a str,
    line: usize,
    repairs: Option<&'r mut Vec<Repair>>,
}

impl<'a, 'r> LineParser<'a, 'r> {
    fn lenient(&self) -> bool {
        self.repairs.is_some()
    }

    fn err(&self, kind: ErrorKind) -> ParseError {
        ParseError {
            location: Location { line: self.line },
            kind,
        }
    }

    fn repair(&mut self, kind: RepairKind) {
        self.repairs
            .as_deref_mut()
            .expect("repair() only called in lenient mode")
            .push(Repair {
                location: Location { line: self.line },
                kind,
            });
    }

    fn peek(&self) -> Option<char> {
        self.rest.chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let mut chars = self.rest.chars();
        let c = chars.next();
        self.rest = chars.as_str();
        c
    }

    /// Take the longest prefix whose characters are not in `stop`.
    fn take_until(&mut self, stop: &[char]) -> &'a str {
        let end = self
            .rest
            .char_indices()
            .find(|(_, c)| stop.contains(c))
            .map(|(i, _)| i)
            .unwrap_or(self.rest.len());
        let (head, tail) = self.rest.split_at(end);
        self.rest = tail;
        head
    }

    fn parse(mut self) -> Result<Property, ParseError> {
        // Name section: everything up to the first ';' or ':'.
        let name_section = self.take_until(&[';', ':']);

        // A group prefix is everything before the first '.'. (RFC 6350
        // groups do not nest; a '.' later in the name is not valid in
        // either grammar, so first-dot splitting loses nothing.)
        let (group, name) = match name_section.split_once('.') {
            Some((g, n)) => (Some(g), n),
            None => (None, name_section),
        };

        let strict_ok = is_strict_name(name) && group.map_or(true, is_strict_name);
        if self.lenient() {
            if !(is_lenient_name(name) && group.map_or(true, is_lenient_name)) {
                return Err(self.err(ErrorKind::InvalidName(name_section.to_string())));
            }
            if !strict_ok {
                self.repair(RepairKind::NonstandardName(name_section.to_string()));
            }
        } else if !strict_ok {
            return Err(self.err(ErrorKind::InvalidName(name_section.to_string())));
        }

        let mut params = Vec::new();
        loop {
            match self.bump() {
                Some(':') => break,
                Some(';') => {
                    if let Some(param) = self.parse_param()? {
                        params.push(param);
                    }
                }
                Some(_) => unreachable!("take_until stopped on ';' or ':'"),
                None => return Err(self.err(ErrorKind::MissingColon)),
            }
        }

        let value = self.rest;
        let value = self.check_text(value)?;

        Ok(Property {
            group: group.map(|g| g.to_string()),
            name: name.to_string(),
            params,
            value,
        })
    }

    /// Parse one parameter, positioned just after a ';'. Returns None if the
    /// parameter was empty and dropped in lenient mode (e.g. `X;;Y:v`).
    fn parse_param(&mut self) -> Result<Option<Param>, ParseError> {
        let name = self.take_until(&['=', ';', ':', '"']);
        match self.peek() {
            Some('=') => {
                self.bump();
            }
            Some(';') | Some(':') => {
                // vCard 2.1 bare parameter: TEL;HOME;VOICE:...
                if !self.lenient() {
                    return Err(self.err(ErrorKind::InvalidParamName(name.to_string())));
                }
                if name.is_empty() {
                    // `X;;Y:v` — an empty parameter; drop it.
                    self.repair(RepairKind::DroppedLine(ErrorKind::InvalidParamName(
                        String::new(),
                    )));
                    return Ok(None);
                }
                if !is_lenient_name(name) {
                    return Err(self.err(ErrorKind::InvalidParamName(name.to_string())));
                }
                self.repair(RepairKind::BareParameter(name.to_string()));
                return Ok(Some(Param::bare(name)));
            }
            Some('"') => {
                return Err(self.err(ErrorKind::InvalidParamName(name.to_string())));
            }
            Some(_) => unreachable!("take_until stopped on a delimiter"),
            None => return Err(self.err(ErrorKind::MissingColon)),
        }

        if self.lenient() {
            if !is_lenient_name(name) {
                return Err(self.err(ErrorKind::InvalidParamName(name.to_string())));
            }
            if !is_strict_name(name) {
                self.repair(RepairKind::NonstandardName(name.to_string()));
            }
        } else if !is_strict_name(name) {
            return Err(self.err(ErrorKind::InvalidParamName(name.to_string())));
        }

        let mut values = Vec::new();
        loop {
            values.push(self.parse_param_value()?);
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                _ => break,
            }
        }

        Ok(Some(Param {
            name: name.to_string(),
            values,
        }))
    }

    /// Parse a single parameter value (quoted or bare), leaving the cursor
    /// on the terminating ',', ';', or ':' (or at end of input).
    fn parse_param_value(&mut self) -> Result<String, ParseError> {
        if self.peek() == Some('"') {
            let after_quote = &self.rest[1..];
            match after_quote.find('"') {
                Some(end) => {
                    let inner = &after_quote[..end];
                    self.rest = &after_quote[end + 1..];
                    let inner = self.check_param_text(inner)?;
                    // Anything between the closing quote and the next
                    // delimiter is out-of-grammar; in lenient mode append it.
                    let mut out = inner;
                    if !matches!(self.peek(), Some(',') | Some(';') | Some(':') | None) {
                        if !self.lenient() {
                            return Err(self.err(ErrorKind::InvalidParamValue(
                                self.rest.to_string(),
                            )));
                        }
                        self.repair(RepairKind::KeptStrayQuote);
                        let extra = self.take_until(&[',', ';', ':']);
                        out.push_str(&caret_decode(extra));
                    }
                    Ok(out)
                }
                None => {
                    if !self.lenient() {
                        return Err(self.err(ErrorKind::UnterminatedQuote));
                    }
                    // Reparse from the opening quote treating '"' as an
                    // ordinary character.
                    self.repair(RepairKind::ClosedUnterminatedQuote);
                    let raw = self.take_until(&[',', ';', ':']);
                    self.check_param_text(raw)
                }
            }
        } else {
            let raw = self.take_until(&[',', ';', ':']);
            if raw.contains('"') {
                if !self.lenient() {
                    return Err(self.err(ErrorKind::InvalidParamValue(raw.to_string())));
                }
                self.repair(RepairKind::KeptStrayQuote);
            }
            self.check_param_text(raw)
        }
    }

    /// Validate characters of a parameter value and caret-decode it.
    ///
    /// The RFC's stricter SAFE-CHAR/QSAFE-CHAR alphabets need no extra
    /// checking here: the characters they exclude beyond controls (DQUOTE,
    /// and `;:,` in unquoted values) are structural, so the line parser has
    /// already either consumed them or diagnosed them (KeptStrayQuote).
    fn check_param_text(&mut self, s: &str) -> Result<String, ParseError> {
        let s = self.check_text(s)?;
        Ok(caret_decode(&s))
    }

    /// Reject (strict) or keep-with-repair (lenient) control characters.
    /// HTAB is always permitted.
    fn check_text(&mut self, s: &str) -> Result<String, ParseError> {
        for c in s.chars() {
            if c.is_control() && c != '\t' {
                if !self.lenient() {
                    return Err(self.err(ErrorKind::ControlCharacter(c)));
                }
                self.repair(RepairKind::KeptControlCharacter(c));
            }
        }
        Ok(s.to_string())
    }
}

/// Parse one logical content line.
///
/// `repairs == None` selects strict mode. In lenient mode some inputs are
/// still unparseable (no colon at all, empty name); the caller decides how
/// to handle the error (the tree parser drops the line with a repair).
pub fn parse_content_line(
    line: &str,
    line_no: usize,
    repairs: Option<&mut Vec<Repair>>,
) -> Result<Property, ParseError> {
    LineParser {
        rest: line,
        line: line_no,
        repairs,
    }
    .parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict(line: &str) -> Result<Property, ParseError> {
        parse_content_line(line, 1, None)
    }

    fn lenient(line: &str) -> (Property, Vec<Repair>) {
        let mut repairs = Vec::new();
        let p = parse_content_line(line, 1, Some(&mut repairs)).unwrap();
        (p, repairs)
    }

    #[test]
    fn simple() {
        let p = strict("SUMMARY:Hello, world").unwrap();
        assert_eq!(p.name, "SUMMARY");
        assert_eq!(p.value, "Hello, world");
        assert!(p.params.is_empty());
        assert!(p.group.is_none());
    }

    #[test]
    fn empty_value() {
        let p = strict("X-EMPTY:").unwrap();
        assert_eq!(p.value, "");
    }

    #[test]
    fn group() {
        let p = strict("item1.EMAIL;TYPE=INTERNET:alice@example.com").unwrap();
        assert_eq!(p.group.as_deref(), Some("item1"));
        assert_eq!(p.name, "EMAIL");
        assert_eq!(p.param_value("TYPE"), Some("INTERNET"));
        assert_eq!(p.value, "alice@example.com");
    }

    #[test]
    fn multiple_params_and_values() {
        let p = strict("TEL;TYPE=home,voice;PREF=1:+441234").unwrap();
        assert_eq!(p.params.len(), 2);
        assert_eq!(
            p.param_values("TYPE").collect::<Vec<_>>(),
            vec!["home", "voice"]
        );
        assert_eq!(p.param_value("PREF"), Some("1"));
    }

    #[test]
    fn quoted_param_value() {
        let p = strict("DTSTART;TZID=\"US/Mountain: MST\":20260101T000000").unwrap();
        assert_eq!(p.param_value("TZID"), Some("US/Mountain: MST"));
        assert_eq!(p.value, "20260101T000000");
    }

    #[test]
    fn quoted_value_containing_all_delimiters() {
        let p = strict("X;A=\"a,b;c:d\":v").unwrap();
        assert_eq!(p.param_value("A"), Some("a,b;c:d"));
        assert_eq!(p.value, "v");
    }

    #[test]
    fn mixed_quoted_and_bare_values() {
        let p = strict("X;A=one,\"two,2\",three:v").unwrap();
        assert_eq!(
            p.param_values("A").collect::<Vec<_>>(),
            vec!["one", "two,2", "three"]
        );
    }

    #[test]
    fn empty_param_values() {
        let p = strict("X;A=:v").unwrap();
        assert_eq!(p.param_value("A"), Some(""));
        let p = strict("X;A=a,,b:v").unwrap();
        assert_eq!(p.param_values("A").collect::<Vec<_>>(), vec!["a", "", "b"]);
    }

    #[test]
    fn caret_decoding_applied_to_params() {
        let p = strict("X;A=one^ntwo^^three^'four:v").unwrap();
        assert_eq!(p.param_value("A"), Some("one\ntwo^three\"four"));
    }

    #[test]
    fn value_keeps_colons_and_escapes_raw() {
        let p = strict("DESCRIPTION:see http://example.com\\, ok?").unwrap();
        assert_eq!(p.value, "see http://example.com\\, ok?");
    }

    #[test]
    fn strict_rejects_missing_colon() {
        assert_eq!(strict("NOCOLON").unwrap_err().kind, ErrorKind::MissingColon);
        assert_eq!(
            strict("X;A=b").unwrap_err().kind,
            ErrorKind::MissingColon
        );
    }

    #[test]
    fn strict_rejects_bad_names() {
        assert!(matches!(
            strict(":value").unwrap_err().kind,
            ErrorKind::InvalidName(_)
        ));
        assert!(matches!(
            strict("BAD/SLASH:value").unwrap_err().kind,
            ErrorKind::InvalidName(_)
        ));
        assert!(matches!(
            strict("X_UNDER:value").unwrap_err().kind,
            ErrorKind::InvalidName(_)
        ));
    }

    #[test]
    fn lenient_accepts_underscore_names() {
        let (p, repairs) = lenient("X_UNDER:value");
        assert_eq!(p.name, "X_UNDER");
        // Nonstandard names are kept but recorded, preserving the invariant
        // that zero repairs means the input was strictly conformant.
        assert!(repairs
            .iter()
            .any(|r| matches!(r.kind, RepairKind::NonstandardName(_))));
    }

    #[test]
    fn lenient_still_rejects_nameless() {
        let mut repairs = Vec::new();
        assert!(parse_content_line(":value", 1, Some(&mut repairs)).is_err());
        assert!(parse_content_line("", 1, Some(&mut repairs)).is_err());
    }

    #[test]
    fn strict_rejects_bare_param() {
        assert!(matches!(
            strict("TEL;HOME:+441234").unwrap_err().kind,
            ErrorKind::InvalidParamName(_)
        ));
    }

    #[test]
    fn lenient_accepts_bare_params() {
        let (p, repairs) = lenient("TEL;HOME;VOICE:+441234");
        assert_eq!(p.params.len(), 2);
        assert_eq!(p.params[0], Param::bare("HOME"));
        assert_eq!(p.params[1], Param::bare("VOICE"));
        assert_eq!(p.value, "+441234");
        assert_eq!(
            repairs
                .iter()
                .filter(|r| matches!(r.kind, RepairKind::BareParameter(_)))
                .count(),
            2
        );
    }

    #[test]
    fn strict_rejects_unterminated_quote() {
        assert_eq!(
            strict("X;A=\"oops:v").unwrap_err().kind,
            ErrorKind::UnterminatedQuote
        );
    }

    #[test]
    fn lenient_recovers_unterminated_quote() {
        let (p, repairs) = lenient("X;A=\"oops;B=2:v");
        assert_eq!(p.param_value("A"), Some("\"oops"));
        assert_eq!(p.param_value("B"), Some("2"));
        assert_eq!(p.value, "v");
        assert!(repairs
            .iter()
            .any(|r| r.kind == RepairKind::ClosedUnterminatedQuote));
    }

    #[test]
    fn strict_rejects_stray_quote_in_bare_value() {
        assert!(matches!(
            strict("X;A=fo\"o:v").unwrap_err().kind,
            ErrorKind::InvalidParamValue(_)
        ));
    }

    #[test]
    fn lenient_keeps_stray_quote() {
        let (p, repairs) = lenient("X;A=fo\"o:v");
        assert_eq!(p.param_value("A"), Some("fo\"o"));
        assert!(repairs.iter().any(|r| r.kind == RepairKind::KeptStrayQuote));
    }

    #[test]
    fn lenient_text_after_closing_quote() {
        let (p, _) = lenient("X;A=\"quoted\"extra:v");
        assert_eq!(p.param_value("A"), Some("quotedextra"));
        assert_eq!(p.value, "v");
    }

    #[test]
    fn strict_rejects_control_characters() {
        assert_eq!(
            strict("X:ab\u{0007}c").unwrap_err().kind,
            ErrorKind::ControlCharacter('\u{0007}')
        );
    }

    #[test]
    fn lenient_keeps_control_characters() {
        let (p, repairs) = lenient("X:ab\u{0007}c");
        assert_eq!(p.value, "ab\u{0007}c");
        assert!(repairs
            .iter()
            .any(|r| r.kind == RepairKind::KeptControlCharacter('\u{0007}')));
    }

    #[test]
    fn tab_is_allowed_everywhere() {
        assert!(strict("X:a\tb").is_ok());
        assert!(strict("X;A=a\tb:v").is_ok());
    }

    #[test]
    fn value_may_contain_equals_and_quotes() {
        let p = strict("RRULE:FREQ=WEEKLY;BYDAY=MO").unwrap();
        assert_eq!(p.value, "FREQ=WEEKLY;BYDAY=MO");
        let p = strict("X:say \"hi\"").unwrap();
        assert_eq!(p.value, "say \"hi\"");
    }

    #[test]
    fn unicode_in_all_positions() {
        let p = strict("X;A=çedilla:sn☃w").unwrap();
        assert_eq!(p.param_value("A"), Some("çedilla"));
        assert_eq!(p.value, "sn☃w");
    }

    #[test]
    fn lenient_drops_empty_param_between_semicolons() {
        let (p, _) = lenient("X;;A=1:v");
        assert_eq!(p.params.len(), 1);
        assert_eq!(p.param_value("A"), Some("1"));
    }

    #[test]
    fn begin_line_parses_as_property() {
        let p = strict("BEGIN:VCALENDAR").unwrap();
        assert_eq!(p.name, "BEGIN");
        assert_eq!(p.value, "VCALENDAR");
    }
}
