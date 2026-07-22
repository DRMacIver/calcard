//! Building component trees from content lines.

use crate::contentline::parse_content_line;
use crate::error::{ErrorKind, Location, ParseError, Repair, RepairKind};
use crate::lines::unfold;
use crate::model::{Component, Property};

/// How forgiving the parser should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strictness {
    /// Any deviation from the RFC grammars is an error.
    Strict,
    /// Recover from real-world breakage wherever a reasonable interpretation
    /// exists, recording a [`Repair`] for each recovery.
    #[default]
    Lenient,
}

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    pub strictness: Strictness,
}

impl ParseOptions {
    pub fn strict() -> ParseOptions {
        ParseOptions {
            strictness: Strictness::Strict,
        }
    }

    pub fn lenient() -> ParseOptions {
        ParseOptions {
            strictness: Strictness::Lenient,
        }
    }
}

/// The result of a successful parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Top-level components, in order. Usually one VCALENDAR, or a stream
    /// of VCARDs.
    pub components: Vec<Component>,
    /// Everything that was fixed up in lenient mode. Empty in strict mode
    /// (strict parses either fail or need no repairs), and empty in lenient
    /// mode exactly when the input was fully conformant.
    pub repairs: Vec<Repair>,
}

/// Parse a complete document from text.
pub fn parse(input: &str, options: &ParseOptions) -> Result<Parsed, ParseError> {
    let mut repairs = Vec::new();
    let lenient = options.strictness == Strictness::Lenient;

    let logical = unfold(input, if lenient { Some(&mut repairs) } else { None })?;

    let mut roots: Vec<Component> = Vec::new();
    // Stack of open components.
    let mut stack: Vec<Component> = Vec::new();

    for line in &logical {
        let loc = Location { line: line.line };
        let prop = match parse_content_line(
            &line.text,
            line.line,
            if lenient { Some(&mut repairs) } else { None },
        ) {
            Ok(p) => p,
            Err(e) => {
                if lenient {
                    repairs.push(Repair {
                        location: e.location,
                        kind: RepairKind::DroppedLine(e.kind),
                    });
                    continue;
                }
                return Err(e);
            }
        };

        if prop.name.eq_ignore_ascii_case("BEGIN") {
            match delimiter_name(&prop) {
                Some(name) => {
                    stack.push(Component::new(name));
                    continue;
                }
                None => {
                    if lenient {
                        repairs.push(Repair {
                            location: loc,
                            kind: RepairKind::DroppedLine(ErrorKind::MalformedDelimiter),
                        });
                        continue;
                    }
                    return Err(ParseError {
                        location: loc,
                        kind: ErrorKind::MalformedDelimiter,
                    });
                }
            }
        }

        if prop.name.eq_ignore_ascii_case("END") {
            let name = match delimiter_name(&prop) {
                Some(n) => n,
                None => {
                    if lenient {
                        repairs.push(Repair {
                            location: loc,
                            kind: RepairKind::DroppedLine(ErrorKind::MalformedDelimiter),
                        });
                        continue;
                    }
                    return Err(ParseError {
                        location: loc,
                        kind: ErrorKind::MalformedDelimiter,
                    });
                }
            };
            close_component(&mut stack, &mut roots, &name, loc, lenient, &mut repairs)?;
            continue;
        }

        match stack.last_mut() {
            Some(open) => open.push_property(prop),
            None => {
                if lenient {
                    repairs.push(Repair {
                        location: loc,
                        kind: RepairKind::DroppedContentOutsideComponent,
                    });
                    continue;
                }
                return Err(ParseError {
                    location: loc,
                    kind: ErrorKind::ContentOutsideComponent,
                });
            }
        }
    }

    // Unterminated components at end of input.
    while let Some(open) = stack.pop() {
        if !lenient {
            return Err(ParseError {
                location: Location {
                    line: logical.last().map(|l| l.line).unwrap_or(1),
                },
                kind: ErrorKind::UnterminatedComponent(open.name.clone()),
            });
        }
        repairs.push(Repair {
            location: Location {
                line: logical.last().map(|l| l.line).unwrap_or(1),
            },
            kind: RepairKind::ClosedUnterminatedComponent(open.name.clone()),
        });
        attach(&mut stack, &mut roots, open);
    }

    Ok(Parsed {
        components: roots,
        repairs,
    })
}

/// The component name carried by a BEGIN/END line, if it is well formed
/// (non-empty value, no parameters, no group).
fn delimiter_name(prop: &Property) -> Option<String> {
    if prop.value.is_empty() || !prop.params.is_empty() {
        return None;
    }
    // A group on BEGIN (e.g. sabre tests `home.begin:vcard`) is unusual but
    // harmless; we ignore the group rather than reject.
    Some(prop.value.trim().to_string())
}

fn attach(stack: &mut [Component], roots: &mut Vec<Component>, comp: Component) {
    match stack.last_mut() {
        Some(parent) => parent.push_component(comp),
        None => roots.push(comp),
    }
}

/// Handle an END:name line against the open-component stack.
fn close_component(
    stack: &mut Vec<Component>,
    roots: &mut Vec<Component>,
    name: &str,
    loc: Location,
    lenient: bool,
    repairs: &mut Vec<Repair>,
) -> Result<(), ParseError> {
    // Does anything on the stack match?
    let matching = stack
        .iter()
        .rposition(|c| c.name.eq_ignore_ascii_case(name));

    match matching {
        Some(idx) if idx == stack.len() - 1 => {
            let done = stack.pop().unwrap();
            attach(stack, roots, done);
            Ok(())
        }
        Some(idx) => {
            // END matches an outer component: inner ones were never closed.
            if !lenient {
                return Err(ParseError {
                    location: loc,
                    kind: ErrorKind::MismatchedEnd {
                        expected: stack.last().unwrap().name.clone(),
                        found: name.to_string(),
                    },
                });
            }
            while stack.len() > idx + 1 {
                let unterminated = stack.pop().unwrap();
                repairs.push(Repair {
                    location: loc,
                    kind: RepairKind::ClosedUnterminatedComponent(unterminated.name.clone()),
                });
                attach(stack, roots, unterminated);
            }
            let done = stack.pop().unwrap();
            attach(stack, roots, done);
            Ok(())
        }
        None => {
            if !lenient {
                return Err(match stack.last() {
                    Some(open) => ParseError {
                        location: loc,
                        kind: ErrorKind::MismatchedEnd {
                            expected: open.name.clone(),
                            found: name.to_string(),
                        },
                    },
                    None => ParseError {
                        location: loc,
                        kind: ErrorKind::UnmatchedEnd(name.to_string()),
                    },
                });
            }
            repairs.push(Repair {
                location: loc,
                kind: RepairKind::IgnoredUnmatchedEnd(name.to_string()),
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Child;

    const SIMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Hi\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn strict(input: &str) -> Result<Parsed, ParseError> {
        parse(input, &ParseOptions::strict())
    }

    fn lenient(input: &str) -> Parsed {
        parse(input, &ParseOptions::lenient()).unwrap()
    }

    #[test]
    fn simple_document() {
        let parsed = strict(SIMPLE).unwrap();
        assert!(parsed.repairs.is_empty());
        assert_eq!(parsed.components.len(), 1);
        let cal = &parsed.components[0];
        assert_eq!(cal.name, "VCALENDAR");
        assert_eq!(cal.prop("VERSION").unwrap().value, "2.0");
        assert_eq!(cal.comp("VEVENT").unwrap().prop("SUMMARY").unwrap().value, "Hi");
    }

    #[test]
    fn lenient_on_clean_input_records_no_repairs() {
        let parsed = lenient(SIMPLE);
        assert!(parsed.repairs.is_empty());
        assert_eq!(parsed.components, strict(SIMPLE).unwrap().components);
    }

    #[test]
    fn multiple_top_level_components() {
        let input = "BEGIN:VCARD\r\nFN:A\r\nEND:VCARD\r\nBEGIN:VCARD\r\nFN:B\r\nEND:VCARD\r\n";
        let parsed = strict(input).unwrap();
        assert_eq!(parsed.components.len(), 2);
        assert_eq!(parsed.components[1].prop("FN").unwrap().value, "B");
    }

    #[test]
    fn empty_input_is_empty_document() {
        assert_eq!(strict("").unwrap().components.len(), 0);
    }

    #[test]
    fn end_matching_is_case_insensitive() {
        let input = "begin:vcalendar\r\nEND:VCALENDAR\r\n";
        let parsed = strict(input).unwrap();
        assert_eq!(parsed.components[0].name, "vcalendar");
    }

    #[test]
    fn strict_rejects_unterminated() {
        assert!(matches!(
            strict("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n").unwrap_err().kind,
            ErrorKind::UnterminatedComponent(_)
        ));
    }

    #[test]
    fn lenient_closes_unterminated() {
        let parsed = lenient("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\n");
        assert_eq!(parsed.components.len(), 1);
        let cal = &parsed.components[0];
        assert_eq!(cal.comp("VEVENT").unwrap().prop("SUMMARY").unwrap().value, "x");
        assert_eq!(
            parsed
                .repairs
                .iter()
                .filter(|r| matches!(r.kind, RepairKind::ClosedUnterminatedComponent(_)))
                .count(),
            2
        );
    }

    #[test]
    fn strict_rejects_mismatched_end() {
        let input = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nEND:VCALENDAR\r\nEND:VEVENT\r\n";
        assert!(matches!(
            strict(input).unwrap_err().kind,
            ErrorKind::MismatchedEnd { .. }
        ));
    }

    #[test]
    fn lenient_closes_inner_on_outer_end() {
        let input = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\nEND:VCALENDAR\r\n";
        let parsed = lenient(input);
        assert_eq!(parsed.components.len(), 1);
        assert!(parsed.components[0].comp("VEVENT").is_some());
    }

    #[test]
    fn strict_rejects_unmatched_end() {
        assert!(matches!(
            strict("END:VCALENDAR\r\n").unwrap_err().kind,
            ErrorKind::UnmatchedEnd(_)
        ));
    }

    #[test]
    fn lenient_ignores_unmatched_end() {
        let parsed = lenient("BEGIN:VCARD\r\nFN:x\r\nEND:VTODO\r\nEND:VCARD\r\n");
        assert_eq!(parsed.components.len(), 1);
        assert_eq!(parsed.components[0].name, "VCARD");
        assert!(parsed
            .repairs
            .iter()
            .any(|r| r.kind == RepairKind::IgnoredUnmatchedEnd("VTODO".into())));
    }

    #[test]
    fn strict_rejects_content_outside_component() {
        assert!(matches!(
            strict("VERSION:2.0\r\n").unwrap_err().kind,
            ErrorKind::ContentOutsideComponent
        ));
    }

    #[test]
    fn lenient_drops_content_outside_component() {
        let parsed = lenient("junk before\r\nBEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\ntrailing junk\r\n");
        assert_eq!(parsed.components.len(), 1);
        assert!(!parsed.repairs.is_empty());
    }

    #[test]
    fn lenient_drops_unparseable_lines() {
        let parsed = lenient("BEGIN:VCARD\r\nTHIS LINE HAS NO COLON\r\nFN:x\r\nEND:VCARD\r\n");
        assert_eq!(parsed.components[0].properties().count(), 1);
        assert!(parsed
            .repairs
            .iter()
            .any(|r| matches!(r.kind, RepairKind::DroppedLine(_))));
    }

    #[test]
    fn interleaving_preserved() {
        let input = "BEGIN:VCALENDAR\r\nA:1\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nB:2\r\nEND:VCALENDAR\r\n";
        let parsed = strict(input).unwrap();
        let cal = &parsed.components[0];
        assert!(matches!(cal.children[0], Child::Property(_)));
        assert!(matches!(cal.children[1], Child::Component(_)));
        assert!(matches!(cal.children[2], Child::Property(_)));
    }

    #[test]
    fn deep_nesting() {
        let mut input = String::new();
        let depth = 200;
        for _ in 0..depth {
            input.push_str("BEGIN:X\r\n");
        }
        for _ in 0..depth {
            input.push_str("END:X\r\n");
        }
        let parsed = strict(&input).unwrap();
        let mut c = &parsed.components[0];
        let mut count = 1;
        while let Some(inner) = c.comp("X") {
            c = inner;
            count += 1;
        }
        assert_eq!(count, depth);
    }

    #[test]
    fn folded_property_line() {
        let input = "BEGIN:VCARD\r\nNOTE:hello\r\n  world\r\nEND:VCARD\r\n";
        let parsed = strict(input).unwrap();
        assert_eq!(parsed.components[0].prop("NOTE").unwrap().value, "hello world");
    }

    #[test]
    fn begin_with_params_rejected_strict_dropped_lenient() {
        let input = "BEGIN;X=1:VCALENDAR\r\nEND:VCALENDAR\r\n";
        assert!(strict(input).is_err());
        let parsed = lenient(input);
        // The BEGIN was dropped, so END:VCALENDAR is unmatched and ignored.
        assert!(parsed.components.is_empty());
    }
}
