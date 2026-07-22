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
//! the underlying text.
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
//!   zero repairs and the identical document a strict parse would.
//!
//! ```
//! let input = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:Tea\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
//! let parsed = vobject_core::parse(input, &vobject_core::ParseOptions::strict()).unwrap();
//! let event = parsed.components[0].comp("VEVENT").unwrap();
//! assert_eq!(event.prop("SUMMARY").unwrap().value, "Tea");
//! let out = vobject_core::write_document(&parsed.components, &Default::default());
//! assert_eq!(out, input);
//! ```

pub mod contentline;
pub mod error;
pub mod escape;
pub mod fold;
pub mod lines;
pub mod model;
pub mod parse;
pub mod write;

pub use error::{ErrorKind, Location, ParseError, Repair, RepairKind};
pub use model::{Child, Component, Param, Property};
pub use parse::{parse, ParseOptions, Parsed, Strictness};
pub use write::{write_component, write_document, WriteOptions};
