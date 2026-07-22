//! Error and repair types.
//!
//! In strict mode any syntactic problem is a [`ParseError`]. In lenient mode
//! most problems are recovered from, and each recovery is recorded as a
//! [`Repair`] so callers can report exactly what was fixed up.

use std::fmt;

/// Location of a problem in the input, as a 1-based logical line number.
///
/// Line numbers refer to physical lines in the original input (before
/// unfolding), pointing at the first physical line of the logical line
/// involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}", self.line)
    }
}

/// A fatal parse error (strict mode, or unrecoverable even in lenient mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub location: Location,
    pub kind: ErrorKind,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.kind)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A content line with no ':' separating name from value.
    MissingColon,
    /// A content line whose property name is empty or contains invalid characters.
    InvalidName(String),
    /// A parameter with an empty or invalid name.
    InvalidParamName(String),
    /// A quoted parameter value that never closes its quote.
    UnterminatedQuote,
    /// A parameter value contains a character that is never allowed (e.g. a raw
    /// double quote inside an unquoted value).
    InvalidParamValue(String),
    /// Raw control character in a value or parameter.
    ControlCharacter(char),
    /// Blank line inside a document.
    BlankLine,
    /// A bare LF or CR line ending (strict mode requires CRLF).
    LooseLineEnding,
    /// The first line of a component was not BEGIN:...
    ContentOutsideComponent,
    /// END:X seen where X does not match the open component.
    MismatchedEnd { expected: String, found: String },
    /// END:X seen with no open component.
    UnmatchedEnd(String),
    /// Input finished while components were still open.
    UnterminatedComponent(String),
    /// BEGIN or END line carried parameters or an empty name.
    MalformedDelimiter,
    /// Invalid escape sequence in a TEXT value.
    InvalidEscape(char),
    /// Input is not valid UTF-8.
    InvalidUtf8,
    /// A folded continuation line at the very start of input.
    LeadingContinuation,
    /// Component nesting exceeded [`crate::parse::ParseOptions::max_depth`].
    TooDeeplyNested,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::MissingColon => write!(f, "content line has no ':' separator"),
            ErrorKind::InvalidName(n) => write!(f, "invalid property name {n:?}"),
            ErrorKind::InvalidParamName(n) => write!(f, "invalid parameter name {n:?}"),
            ErrorKind::UnterminatedQuote => write!(f, "unterminated quoted parameter value"),
            ErrorKind::InvalidParamValue(v) => write!(f, "invalid parameter value {v:?}"),
            ErrorKind::ControlCharacter(c) => write!(f, "raw control character {:?}", c),
            ErrorKind::BlankLine => write!(f, "blank line inside document"),
            ErrorKind::LooseLineEnding => write!(f, "bare LF or CR line ending"),
            ErrorKind::ContentOutsideComponent => {
                write!(f, "content line outside any component")
            }
            ErrorKind::MismatchedEnd { expected, found } => {
                write!(f, "END:{found} does not match open component {expected}")
            }
            ErrorKind::UnmatchedEnd(n) => write!(f, "END:{n} with no matching BEGIN"),
            ErrorKind::UnterminatedComponent(n) => {
                write!(f, "component {n} was never closed")
            }
            ErrorKind::MalformedDelimiter => write!(f, "malformed BEGIN/END line"),
            ErrorKind::InvalidEscape(c) => write!(f, "invalid escape sequence \\{c}"),
            ErrorKind::InvalidUtf8 => write!(f, "input is not valid UTF-8"),
            ErrorKind::LeadingContinuation => {
                write!(f, "input starts with a folded continuation line")
            }
            ErrorKind::TooDeeplyNested => {
                write!(f, "component nesting exceeds the configured depth limit")
            }
        }
    }
}

/// A recovery performed in lenient mode.
///
/// The parser guarantees that the set of repairs is empty if and only if the
/// input would also have parsed cleanly in strict mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub location: Location,
    pub kind: RepairKind,
}

impl fmt::Display for Repair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairKind {
    /// A line that could not be parsed at all was dropped.
    DroppedLine(ErrorKind),
    /// A blank line was ignored.
    SkippedBlankLine,
    /// A bare newline (LF or CR without partner) was accepted as a line break.
    LooseLineEnding,
    /// An unclosed component was implicitly closed at end of input or at a
    /// mismatched END.
    ClosedUnterminatedComponent(String),
    /// An END with no matching BEGIN was ignored.
    IgnoredUnmatchedEnd(String),
    /// Content lines before any BEGIN, or after all components closed, were
    /// dropped.
    DroppedContentOutsideComponent,
    /// An invalid escape sequence was kept verbatim.
    KeptInvalidEscape(char),
    /// A raw control character was kept in a value.
    KeptControlCharacter(char),
    /// Invalid UTF-8 bytes were replaced with U+FFFD.
    ReplacedInvalidUtf8,
    /// A quoted-printable soft line break continuation was joined.
    JoinedQuotedPrintable,
    /// The input started with a continuation line; the leading whitespace was
    /// treated as the start of a normal line.
    LeadingContinuationTreatedAsLine,
    /// A parameter without '=' (vCard 2.1 style, e.g. TEL;HOME:...) was kept
    /// as a value-less parameter.
    BareParameter(String),
    /// A property, group, or parameter name outside the strict grammar
    /// (e.g. containing '_' or non-ASCII) was kept as-is.
    NonstandardName(String),
    /// An unterminated quoted parameter value was closed at end of line.
    ClosedUnterminatedQuote,
    /// A double quote appeared inside an unquoted parameter value and was kept.
    KeptStrayQuote,
}

impl fmt::Display for RepairKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepairKind::DroppedLine(k) => write!(f, "dropped unparseable line ({k})"),
            RepairKind::SkippedBlankLine => write!(f, "skipped blank line"),
            RepairKind::LooseLineEnding => write!(f, "accepted bare CR or LF line ending"),
            RepairKind::ClosedUnterminatedComponent(n) => {
                write!(f, "implicitly closed unterminated component {n}")
            }
            RepairKind::IgnoredUnmatchedEnd(n) => write!(f, "ignored unmatched END:{n}"),
            RepairKind::DroppedContentOutsideComponent => {
                write!(f, "dropped content line outside any component")
            }
            RepairKind::KeptInvalidEscape(c) => {
                write!(f, "kept invalid escape sequence \\{c}")
            }
            RepairKind::KeptControlCharacter(c) => {
                write!(f, "kept raw control character {c:?}")
            }
            RepairKind::ReplacedInvalidUtf8 => {
                write!(f, "replaced invalid UTF-8 with U+FFFD")
            }
            RepairKind::JoinedQuotedPrintable => {
                write!(f, "joined quoted-printable soft line break")
            }
            RepairKind::LeadingContinuationTreatedAsLine => {
                write!(f, "treated leading continuation line as a new line")
            }
            RepairKind::BareParameter(n) => {
                write!(f, "kept bare parameter {n} (vCard 2.1 style)")
            }
            RepairKind::NonstandardName(n) => {
                write!(f, "kept nonstandard name {n:?}")
            }
            RepairKind::ClosedUnterminatedQuote => {
                write!(f, "closed unterminated quoted parameter value at end of line")
            }
            RepairKind::KeptStrayQuote => {
                write!(f, "kept stray double quote in parameter value")
            }
        }
    }
}
