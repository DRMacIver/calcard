//! # vobject-core
//!
//! A robust parser and serializer for the vobject family of formats:
//! iCalendar (RFC 5545), vCard 4.0 (RFC 6350), vCard 3.0 (RFC 2426), and
//! vCard 2.1, including RFC 6868 parameter encoding.
//!
//! The library is built around a *lossless* document model: parsing and
//! re-serializing preserves property order, unknown properties and
//! parameters, vCard groups, and the interleaving of properties with
//! subcomponents. Typed interpretation of values (dates, durations,
//! recurrence rules, …) is layered on top in [`value`] and never discards
//! the underlying text. Round-trips are *model*-exact rather than
//! byte-exact: serialization is canonical, so gratuitous parameter
//! quotes are dropped, RFC 6868 caret escapes are re-encoded in normal
//! form, and BEGIN/END keywords are written uppercase.
//!
//! Parsing has two modes:
//!
//! - **Strict** ([`ParseOptions::strict`]): any deviation from the RFC
//!   grammars is a [`ParseError`].
//! - **Lenient** ([`ParseOptions::lenient`], the default): real-world
//!   breakage — bare LF line endings, vCard 2.1 bare parameters and
//!   quoted-printable soft line breaks, unterminated components, stray
//!   quotes, control characters — is recovered from, and every recovery is
//!   recorded as a [`Repair`]. Lenient parsing of conformant input yields
//!   zero repairs and the identical document a strict parse would, with
//!   one inherent exception: a property that both declares
//!   `ENCODING=QUOTED-PRINTABLE` and has a value genuinely ending in `=`
//!   is indistinguishable from a vCard 2.1 QP soft line break, and
//!   lenient parsing resolves the ambiguity in favor of the (far more
//!   common) soft break, joining the following line with a [`Repair`]
//!   recorded.
//!
//! ```
//! let input = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Tea\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
//! let parsed = vobject_core::parse(input, &vobject_core::ParseOptions::strict()).unwrap();
//! let event = parsed.components[0].comp("VEVENT").unwrap();
//! assert_eq!(event.prop("SUMMARY").unwrap().value, "Tea");
//! let out = vobject_core::write_document(&parsed.components, &Default::default()).unwrap();
//! assert_eq!(out, input);
//! ```

pub mod contentline;
pub mod error;
pub mod escape;
pub mod fold;
pub mod jcal;
pub mod lines;
pub mod model;
pub mod parse;
pub mod rrule;
pub mod rscale;
pub mod value;
pub mod write;
pub mod xcal;

pub use error::{ErrorKind, Location, ParseError, Repair, RepairKind};
pub use jcal::{from_jcal, JcalError};
pub use model::{Child, Component, Param, Property};
pub use parse::{parse, parse_bytes, ParseOptions, Parsed, Strictness};
pub use write::{write_component, write_document, WriteError, WriteOptions};
