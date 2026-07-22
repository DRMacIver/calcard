//! Escaping and unescaping.
//!
//! Three separate schemes live here:
//!
//! 1. **TEXT value escaping** (RFC 5545 §3.3.11 / RFC 6350 §3.4): `\\`,
//!    `\;`, `\,`, and `\n`/`\N` for newline. Used for TEXT-typed values.
//! 2. **RFC 6868 caret encoding** for parameter values: `^^` for `^`,
//!    `^n` for newline, `^'` for `"`.
//! 3. **Parameter value quoting**: values containing `:`, `;` or `,` must be
//!    surrounded by double quotes on the wire.

use crate::error::{ErrorKind, Repair, RepairKind};

/// Escape a string as a single TEXT value.
///
/// `escape_commas` should be false only when the comma is structural (the
/// caller is joining a multi-valued TEXT list and escapes each element).
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            // A lone CR cannot survive serialization (it would corrupt line
            // structure); normalize CRLF/CR to \n at a higher level before
            // calling this. We map it to \n as a last resort.
            '\r' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Unescape a single TEXT value.
///
/// In strict mode (`repairs == None`) an invalid escape (backslash followed
/// by anything other than `\ ; , n N`) is an error. In lenient mode the
/// backslash and following character are kept verbatim and a repair is
/// recorded. A trailing lone backslash is likewise kept in lenient mode.
pub fn unescape_text(
    s: &str,
    mut repairs: Option<&mut Vec<Repair>>,
    line: usize,
) -> Result<String, ErrorKind> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(',') => out.push(','),
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => match repairs.as_deref_mut() {
                Some(r) => {
                    r.push(Repair {
                        location: crate::error::Location { line },
                        kind: RepairKind::KeptInvalidEscape(other),
                    });
                    out.push('\\');
                    out.push(other);
                }
                None => return Err(ErrorKind::InvalidEscape(other)),
            },
            None => match repairs.as_deref_mut() {
                Some(r) => {
                    r.push(Repair {
                        location: crate::error::Location { line },
                        kind: RepairKind::KeptInvalidEscape('\0'),
                    });
                    out.push('\\');
                }
                None => return Err(ErrorKind::InvalidEscape('\0')),
            },
        }
    }
    Ok(out)
}

/// Split a raw multi-valued TEXT field on unescaped commas (or another
/// separator such as `;` for structured values like vCard N/ADR), without
/// unescaping the pieces.
pub fn split_unescaped(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&s[start..]);
    parts
}

/// RFC 6868: encode a parameter value's special characters with carets.
pub fn caret_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '^' => out.push_str("^^"),
            '\n' => out.push_str("^n"),
            // A lone CR in a param value can't be written any other way.
            '\r' => out.push_str("^n"),
            '"' => out.push_str("^'"),
            _ => out.push(c),
        }
    }
    out
}

/// RFC 6868: decode caret escapes in a parameter value. Sequences that are
/// not defined (`^` followed by anything but `^`, `n`, `'`) are kept
/// verbatim, as the RFC specifies.
pub fn caret_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '^' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('^') => {
                out.push('^');
                chars.next();
            }
            Some('n') => {
                out.push('\n');
                chars.next();
            }
            Some('\'') => {
                out.push('"');
                chars.next();
            }
            _ => out.push('^'),
        }
    }
    out
}

/// How a parameter value must be written on the wire.
pub enum ParamQuoting {
    /// Safe to write bare.
    Bare,
    /// Must be surrounded with double quotes (contains `:`, `;`, or `,`).
    Quoted,
}

/// Decide quoting for an (already caret-encoded) parameter value.
pub fn param_quoting(encoded: &str) -> ParamQuoting {
    if encoded.contains([':', ';', ',']) {
        ParamQuoting::Quoted
    } else {
        ParamQuoting::Bare
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let cases = [
            "plain",
            "semi;colon",
            "com,ma",
            "back\\slash",
            "new\nline",
            "all of it: \\ ; , \n done",
            "",
            "unicode ünïcödé ☃",
        ];
        for case in cases {
            let escaped = escape_text(case);
            let back = unescape_text(&escaped, None, 1).unwrap();
            assert_eq!(back, case, "escaped form was {escaped:?}");
        }
    }

    #[test]
    fn unescape_accepts_upper_n() {
        assert_eq!(unescape_text("a\\Nb", None, 1).unwrap(), "a\nb");
    }

    #[test]
    fn strict_rejects_invalid_escape() {
        assert_eq!(
            unescape_text("a\\tb", None, 1).unwrap_err(),
            ErrorKind::InvalidEscape('t')
        );
        assert_eq!(
            unescape_text("trailing\\", None, 1).unwrap_err(),
            ErrorKind::InvalidEscape('\0')
        );
    }

    #[test]
    fn lenient_keeps_invalid_escape() {
        let mut repairs = Vec::new();
        let out = unescape_text("a\\tb", Some(&mut repairs), 7).unwrap();
        assert_eq!(out, "a\\tb");
        assert_eq!(repairs.len(), 1);
        assert_eq!(repairs[0].location.line, 7);
        let out = unescape_text("trailing\\", Some(&mut repairs), 8).unwrap();
        assert_eq!(out, "trailing\\");
    }

    #[test]
    fn split_respects_escapes() {
        assert_eq!(split_unescaped("a,b,c", ','), vec!["a", "b", "c"]);
        assert_eq!(split_unescaped("a\\,b,c", ','), vec!["a\\,b", "c"]);
        assert_eq!(split_unescaped("", ','), vec![""]);
        assert_eq!(split_unescaped("a\\\\,b", ','), vec!["a\\\\", "b"]);
        assert_eq!(split_unescaped("x;y\\;z", ';'), vec!["x", "y\\;z"]);
        // Trailing escape does not eat the separator check.
        assert_eq!(split_unescaped("a\\", ','), vec!["a\\"]);
    }

    #[test]
    fn caret_round_trip() {
        let cases = ["plain", "care^t", "quo\"te", "new\nline", "^n literal ^' mix ^^"];
        for case in cases {
            assert_eq!(caret_decode(&caret_encode(case)), case);
        }
    }

    #[test]
    fn caret_decode_keeps_undefined_sequences() {
        assert_eq!(caret_decode("a^xb"), "a^xb");
        assert_eq!(caret_decode("end^"), "end^");
    }

    #[test]
    fn quoting_decision() {
        assert!(matches!(param_quoting("simple"), ParamQuoting::Bare));
        assert!(matches!(param_quoting("with space"), ParamQuoting::Bare));
        for needs in ["a:b", "a;b", "a,b"] {
            assert!(matches!(param_quoting(needs), ParamQuoting::Quoted));
        }
    }
}
