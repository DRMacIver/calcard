//! Building component trees from content lines.

use crate::contentline;
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

#[derive(Debug, Clone)]
pub struct ParseOptions {
    pub strictness: Strictness,
    /// Maximum component nesting depth. Real documents nest a handful of
    /// levels; the cap exists so that adversarial inputs (fuzzer files nest
    /// BEGIN 60k+ deep) cannot build trees whose recursive traversal or
    /// destruction overflows the stack. In strict mode exceeding it is an
    /// error; in lenient mode the offending BEGIN is dropped with a repair.
    pub max_depth: usize,
}

pub const DEFAULT_MAX_DEPTH: usize = 512;

impl Default for ParseOptions {
    fn default() -> ParseOptions {
        ParseOptions::lenient()
    }
}

impl ParseOptions {
    pub fn strict() -> ParseOptions {
        ParseOptions {
            strictness: Strictness::Strict,
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    pub fn lenient() -> ParseOptions {
        ParseOptions {
            strictness: Strictness::Lenient,
            max_depth: DEFAULT_MAX_DEPTH,
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
            match delimiter_name(&prop, loc, if lenient { Some(&mut repairs) } else { None }) {
                Some(name) => {
                    if stack.len() >= options.max_depth {
                        if lenient {
                            repairs.push(Repair {
                                location: loc,
                                kind: RepairKind::DroppedLine(ErrorKind::TooDeeplyNested),
                            });
                            continue;
                        }
                        return Err(ParseError {
                            location: loc,
                            kind: ErrorKind::TooDeeplyNested,
                        });
                    }
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
            let name =
                match delimiter_name(&prop, loc, if lenient { Some(&mut repairs) } else { None }) {
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

/// Parse a complete document from raw bytes.
///
/// The RFC default (and only strict) charset is UTF-8; a UTF-8 BOM is
/// stripped transparently. In lenient mode non-UTF-8 input is decoded as
/// Latin-1 — a total, byte-preserving decoding for legacy data — with a
/// [`RepairKind::DecodedNonUtf8AsLatin1`] repair recorded at the first
/// offending line; in strict mode it is an [`ErrorKind::InvalidUtf8`] error.
pub fn parse_bytes(input: &[u8], options: &ParseOptions) -> Result<Parsed, ParseError> {
    let input = input.strip_prefix(b"\xef\xbb\xbf").unwrap_or(input);
    match std::str::from_utf8(input) {
        Ok(s) => parse(s, options),
        Err(e) => {
            let line = 1 + input[..e.valid_up_to()]
                .iter()
                .filter(|&&b| b == b'\n')
                .count();
            if options.strictness == Strictness::Strict {
                return Err(ParseError {
                    location: Location { line },
                    kind: ErrorKind::InvalidUtf8,
                });
            }
            let decoded: String = input.iter().map(|&b| b as char).collect();
            let mut parsed = parse(&decoded, options)?;
            parsed.repairs.insert(
                0,
                Repair {
                    location: Location { line },
                    kind: RepairKind::DecodedNonUtf8AsLatin1,
                },
            );
            Ok(parsed)
        }
    }
}

/// The component name carried by a BEGIN/END line.
///
/// Strictly well formed means: no parameters, no group, and a non-empty
/// iana-token/x-name with no surrounding whitespace. In lenient mode a
/// group prefix (e.g. sabre tests' `home.begin:vcard`) or stray whitespace
/// is normalized away with a recorded repair, and a name outside the
/// strict grammar (but free of structural/control characters) is kept
/// with a repair, mirroring property-name handling; in strict mode all of
/// these are errors, keeping the zero-repairs ⟺ strictly-valid invariant
/// honest.
fn delimiter_name(
    prop: &Property,
    loc: Location,
    mut repairs: Option<&mut Vec<Repair>>,
) -> Option<String> {
    if !prop.params.is_empty() {
        return None;
    }
    let name = prop.value.trim();
    if name.is_empty() {
        return None;
    }
    if prop.group.is_some() || name != prop.value {
        match repairs.as_deref_mut() {
            Some(repairs) => repairs.push(Repair {
                location: loc,
                kind: RepairKind::NormalizedDelimiter(name.to_string()),
            }),
            None => return None,
        }
    }
    if !contentline::is_strict_name(name) {
        match repairs {
            Some(repairs) if contentline::is_lenient_name(name) => repairs.push(Repair {
                location: loc,
                kind: RepairKind::NonstandardName(name.to_string()),
            }),
            _ => return None,
        }
    }
    Some(name.to_string())
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
        assert_eq!(
            cal.comp("VEVENT").unwrap().prop("SUMMARY").unwrap().value,
            "Hi"
        );
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
            strict("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n")
                .unwrap_err()
                .kind,
            ErrorKind::UnterminatedComponent(_)
        ));
    }

    #[test]
    fn lenient_closes_unterminated() {
        let parsed = lenient("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:x\r\n");
        assert_eq!(parsed.components.len(), 1);
        let cal = &parsed.components[0];
        assert_eq!(
            cal.comp("VEVENT").unwrap().prop("SUMMARY").unwrap().value,
            "x"
        );
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
        let parsed =
            lenient("junk before\r\nBEGIN:VCARD\r\nFN:x\r\nEND:VCARD\r\ntrailing junk\r\n");
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
    fn strict_rejects_invalid_component_names() {
        // Component names must satisfy the same iana-token/x-name grammar
        // as property names; these used to slip through with 0 repairs.
        for input in [
            "BEGIN:foo bar\r\nEND:foo bar\r\n",
            "BEGIN:V=CARD\r\nEND:V=CARD\r\n",
            "BEGIN:X_A\r\nEND:X_A\r\n",
        ] {
            assert_eq!(
                strict(input).unwrap_err().kind,
                ErrorKind::MalformedDelimiter,
                "{input:?}"
            );
        }
    }

    #[test]
    fn lenient_repairs_nonstandard_component_name() {
        // Names outside the strict grammar but harmless (no structural or
        // control characters) are kept with a repair, mirroring property
        // name handling.
        for input in [
            "BEGIN:foo bar\r\nEND:foo bar\r\n",
            "BEGIN:X_A\r\nEND:X_A\r\n",
        ] {
            let parsed = lenient(input);
            assert_eq!(parsed.components.len(), 1, "{input:?}");
            assert!(
                parsed
                    .repairs
                    .iter()
                    .any(|r| matches!(r.kind, RepairKind::NonstandardName(_))),
                "{input:?}"
            );
        }
    }

    #[test]
    fn lenient_drops_structural_component_name() {
        // A delimiter whose name could not be re-serialized unambiguously
        // is dropped like any other malformed delimiter.
        let parsed = lenient("BEGIN:V=CARD\r\nFN:x\r\nEND:V=CARD\r\n");
        assert_eq!(parsed.components.len(), 0);
        assert!(parsed.repairs.iter().any(|r| matches!(
            r.kind,
            RepairKind::DroppedLine(ErrorKind::MalformedDelimiter)
        )));
    }

    #[test]
    fn interleaving_preserved() {
        let input =
            "BEGIN:VCALENDAR\r\nA:1\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nB:2\r\nEND:VCALENDAR\r\n";
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
    fn pathological_nesting_does_not_overflow() {
        // Fuzz corpus files nest BEGIN 60k+ deep; unbounded trees would
        // overflow the stack in the recursive derived impls (Drop,
        // PartialEq, Debug). Depth is capped instead.
        let depth = 100_000;
        let mut input = String::with_capacity(depth * 10);
        for _ in 0..depth {
            input.push_str("BEGIN:X\r\n");
        }
        input.push_str("SUMMARY:deep\r\n");
        for _ in 0..depth {
            input.push_str("END:X\r\n");
        }

        assert!(matches!(
            strict(&input).unwrap_err().kind,
            ErrorKind::TooDeeplyNested
        ));

        let parsed = lenient(&input);
        assert_eq!(parsed.components.len(), 1);
        assert!(parsed
            .repairs
            .iter()
            .any(|r| matches!(r.kind, RepairKind::DroppedLine(ErrorKind::TooDeeplyNested))));
        // The over-deep content still lands somewhere: the innermost kept
        // component holds the SUMMARY.
        let mut c = &parsed.components[0];
        while let Some(inner) = c.comp("X") {
            c = inner;
        }
        assert_eq!(c.prop("SUMMARY").unwrap().value, "deep");
    }

    #[test]
    fn depth_limit_is_configurable() {
        let input = "BEGIN:A\r\nBEGIN:B\r\nEND:B\r\nEND:A\r\n";
        let options = ParseOptions {
            max_depth: 1,
            ..ParseOptions::strict()
        };
        assert!(matches!(
            parse(input, &options).unwrap_err().kind,
            ErrorKind::TooDeeplyNested
        ));
        let options = ParseOptions {
            max_depth: 2,
            ..ParseOptions::strict()
        };
        assert!(parse(input, &options).is_ok());
    }

    #[test]
    fn folded_property_line() {
        let input = "BEGIN:VCARD\r\nNOTE:hello\r\n  world\r\nEND:VCARD\r\n";
        let parsed = strict(input).unwrap();
        assert_eq!(
            parsed.components[0].prop("NOTE").unwrap().value,
            "hello world"
        );
    }

    #[test]
    fn begin_with_whitespace_only_name_is_malformed() {
        // The name is trimmed, so a whitespace-only BEGIN value is as
        // malformed as an empty one; it must not create a component with an
        // empty name (which could not be re-serialized).
        let input = "BEGIN:  \r\nEND:  \r\n";
        assert!(strict(input).is_err());
        let parsed = lenient(input);
        assert!(parsed.components.is_empty());
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
