//! Physical line handling: splitting input into logical (unfolded) lines.
//!
//! RFC 5545 §3.1: lines are delimited by CRLF; a CRLF followed by a single
//! space or horizontal tab is a fold and is removed together with that one
//! whitespace character. Lenient mode additionally accepts bare LF or CR
//! line endings (recording a repair), skips blank lines, and understands
//! vCard 2.1 quoted-printable soft line breaks (a line whose parameters
//! include QUOTED-PRINTABLE and which ends in `=` continues on the next
//! physical line).

use crate::error::{ErrorKind, Location, ParseError, Repair, RepairKind};

/// One logical (unfolded) content line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalLine {
    pub text: String,
    /// 1-based physical line number where this logical line started.
    pub line: usize,
}

/// A physical line together with how it was terminated.
struct PhysicalLine<'a> {
    text: &'a str,
    number: usize,
    /// True if terminated by a bare LF or CR rather than CRLF (or end of
    /// input, which is always acceptable).
    loose_ending: bool,
}

/// Split input into physical lines, accepting CRLF always and LF / CR alone
/// as loose endings. A final line without any terminator is fine.
fn physical_lines(input: &str) -> Vec<PhysicalLine<'_>> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut start = 0;
    let mut number = 1;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                let crlf = bytes.get(i + 1) == Some(&b'\n');
                out.push(PhysicalLine {
                    text: &input[start..i],
                    number,
                    loose_ending: !crlf,
                });
                i += if crlf { 2 } else { 1 };
                start = i;
                number += 1;
            }
            b'\n' => {
                out.push(PhysicalLine {
                    text: &input[start..i],
                    number,
                    loose_ending: true,
                });
                i += 1;
                start = i;
                number += 1;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        out.push(PhysicalLine {
            text: &input[start..],
            number,
            loose_ending: false,
        });
    }
    out
}

/// Does this logical-line prefix look like a vCard 2.1 quoted-printable
/// property? True if `QUOTED-PRINTABLE` appears (case-insensitively) in the
/// parameter section, i.e. before the first `:`.
fn looks_quoted_printable(line: &str) -> bool {
    let prefix = match line.split_once(':') {
        Some((before, _)) => before,
        None => return false,
    };
    let upper = prefix.to_ascii_uppercase();
    upper.contains("QUOTED-PRINTABLE")
}

/// Unfold input into logical lines.
///
/// In strict mode (`repairs == None`), loose line endings, blank lines,
/// leading continuations, and QP soft breaks are all errors. In lenient
/// mode they are recovered from and recorded.
pub fn unfold(
    input: &str,
    mut repairs: Option<&mut Vec<Repair>>,
) -> Result<Vec<LogicalLine>, ParseError> {
    let mut out: Vec<LogicalLine> = Vec::new();
    let physical = physical_lines(input);
    let mut idx = 0;
    // Tracks whether the previous physical line ended with a QP soft break
    // that we must honor by appending the next line verbatim.
    let mut qp_continuation = false;

    while idx < physical.len() {
        let phys = physical.get(idx).unwrap();
        idx += 1;
        let loc = Location { line: phys.number };

        if phys.loose_ending {
            match repairs.as_deref_mut() {
                Some(r) => r.push(Repair {
                    location: loc,
                    kind: RepairKind::LooseLineEnding,
                }),
                None => {
                    return Err(ParseError {
                        location: loc,
                        kind: ErrorKind::LooseLineEnding,
                    })
                }
            }
        }

        if qp_continuation {
            let current = out
                .last_mut()
                .expect("qp_continuation implies a previous line");
            // The previous line's trailing '=' was a soft break: drop it and
            // append this physical line verbatim.
            current.text.pop();
            current.text.push_str(phys.text);
            qp_continuation = current.text.ends_with('=') && looks_quoted_printable(&current.text);
            continue;
        }

        if phys.text.is_empty() {
            match repairs.as_deref_mut() {
                Some(r) => {
                    r.push(Repair {
                        location: loc,
                        kind: RepairKind::SkippedBlankLine,
                    });
                    continue;
                }
                None => {
                    return Err(ParseError {
                        location: loc,
                        kind: ErrorKind::BlankLine,
                    })
                }
            }
        }

        if let Some(rest) = strip_fold_prefix(phys.text) {
            match out.last_mut() {
                Some(current) => {
                    current.text.push_str(rest);
                }
                None => match repairs.as_deref_mut() {
                    Some(r) => {
                        r.push(Repair {
                            location: loc,
                            kind: RepairKind::LeadingContinuationTreatedAsLine,
                        });
                        out.push(LogicalLine {
                            text: rest.to_string(),
                            line: phys.number,
                        });
                    }
                    None => {
                        return Err(ParseError {
                            location: loc,
                            kind: ErrorKind::LeadingContinuation,
                        })
                    }
                },
            }
            continue;
        }

        out.push(LogicalLine {
            text: phys.text.to_string(),
            line: phys.number,
        });

        // Check for a vCard 2.1 quoted-printable soft line break. Only in
        // lenient mode: this is outside the RFC 5545 / 6350 grammar.
        if repairs.is_some() {
            let current = out.last().unwrap();
            if current.text.ends_with('=')
                && looks_quoted_printable(&current.text)
                && idx < physical.len()
            {
                if let Some(r) = repairs.as_deref_mut() {
                    r.push(Repair {
                        location: loc,
                        kind: RepairKind::JoinedQuotedPrintable,
                    });
                }
                qp_continuation = true;
            }
        }
    }

    Ok(out)
}

/// If the line begins with exactly one fold marker (space or tab), return
/// the remainder.
fn strip_fold_prefix(line: &str) -> Option<&str> {
    line.strip_prefix(' ').or_else(|| line.strip_prefix('\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict(input: &str) -> Result<Vec<String>, ParseError> {
        unfold(input, None).map(|v| v.into_iter().map(|l| l.text).collect())
    }

    fn lenient(input: &str) -> (Vec<String>, Vec<Repair>) {
        let mut repairs = Vec::new();
        let lines = unfold(input, Some(&mut repairs)).unwrap();
        (lines.into_iter().map(|l| l.text).collect(), repairs)
    }

    #[test]
    fn simple_crlf_lines() {
        assert_eq!(
            strict("A:1\r\nB:2\r\n").unwrap(),
            vec!["A:1".to_string(), "B:2".to_string()]
        );
    }

    #[test]
    fn missing_final_terminator_is_fine() {
        assert_eq!(strict("A:1\r\nB:2").unwrap(), vec!["A:1", "B:2"]);
    }

    #[test]
    fn folding_with_space_and_tab() {
        assert_eq!(
            strict("A:one\r\n two\r\n\tthree\r\n").unwrap(),
            vec!["A:onetwothree"]
        );
    }

    #[test]
    fn fold_strips_exactly_one_char() {
        assert_eq!(strict("A:x\r\n  y\r\n").unwrap(), vec!["A:x y"]);
    }

    #[test]
    fn strict_rejects_bare_lf() {
        assert!(strict("A:1\nB:2\r\n").is_err());
    }

    #[test]
    fn lenient_accepts_bare_lf_and_cr() {
        let (lines, repairs) = lenient("A:1\nB:2\rC:3\r\n");
        assert_eq!(lines, vec!["A:1", "B:2", "C:3"]);
        assert_eq!(repairs.len(), 2);
    }

    #[test]
    fn strict_rejects_blank_line() {
        assert!(strict("A:1\r\n\r\nB:2\r\n").is_err());
    }

    #[test]
    fn lenient_skips_blank_lines() {
        let (lines, repairs) = lenient("A:1\r\n\r\nB:2\r\n");
        assert_eq!(lines, vec!["A:1", "B:2"]);
        assert!(repairs
            .iter()
            .any(|r| r.kind == RepairKind::SkippedBlankLine));
    }

    #[test]
    fn blank_folded_continuation_is_not_blank_line() {
        // A line containing just " " is a continuation contributing nothing.
        assert_eq!(strict("A:1\r\n \r\nB:2\r\n").unwrap(), vec!["A:1", "B:2"]);
    }

    #[test]
    fn line_numbers_point_at_logical_start() {
        let lines = unfold("A:1\r\nB:two\r\n more\r\nC:3\r\n", None).unwrap();
        assert_eq!(lines[1].text, "B:twomore");
        assert_eq!(lines[1].line, 2);
        assert_eq!(lines[2].line, 4);
    }

    #[test]
    fn strict_rejects_leading_continuation() {
        assert!(strict(" A:1\r\n").is_err());
    }

    #[test]
    fn lenient_treats_leading_continuation_as_line() {
        let (lines, repairs) = lenient(" A:1\r\n");
        assert_eq!(lines, vec!["A:1"]);
        assert!(repairs
            .iter()
            .any(|r| r.kind == RepairKind::LeadingContinuationTreatedAsLine));
    }

    #[test]
    fn qp_soft_break_joined_in_lenient() {
        // The '=' soft-break marker is consumed by the join, so the logical
        // line holds clean quoted-printable content.
        let input = "NOTE;ENCODING=QUOTED-PRINTABLE:line one=\r\nline two\r\nFN:Bob\r\n";
        let (lines, repairs) = lenient(input);
        assert_eq!(
            lines,
            vec!["NOTE;ENCODING=QUOTED-PRINTABLE:line oneline two", "FN:Bob"]
        );
        assert!(repairs
            .iter()
            .any(|r| r.kind == RepairKind::JoinedQuotedPrintable));
    }

    #[test]
    fn qp_soft_break_can_chain() {
        let input = "NOTE;ENCODING=QUOTED-PRINTABLE:a=\r\nb=\r\nc\r\n";
        let (lines, _) = lenient(input);
        assert_eq!(lines, vec!["NOTE;ENCODING=QUOTED-PRINTABLE:abc"]);
    }

    #[test]
    fn qp_not_triggered_without_marker() {
        let (lines, _) = lenient("NOTE:ends with equals=\r\nFN:Bob\r\n");
        assert_eq!(lines, vec!["NOTE:ends with equals=", "FN:Bob"]);
    }

    #[test]
    fn qp_marker_in_value_does_not_trigger() {
        let (lines, _) = lenient("NOTE:mentions QUOTED-PRINTABLE=\r\nFN:Bob\r\n");
        assert_eq!(lines, vec!["NOTE:mentions QUOTED-PRINTABLE=", "FN:Bob"]);
    }

    #[test]
    fn unicode_survives_folding() {
        // Exactly one whitespace character is consumed by the fold; a space
        // that should survive needs to be doubled on the wire.
        assert_eq!(strict("A:sn☃w\r\n man\r\n").unwrap(), vec!["A:sn☃wman"]);
        assert_eq!(strict("A:sn☃w\r\n  man\r\n").unwrap(), vec!["A:sn☃w man"]);
    }
}
