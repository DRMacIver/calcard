//! Conformance sweep over the vendored reference-implementation corpus
//! (`conformance/fixtures/`): libical's regression samples and OSS-Fuzz
//! reproducers, ical.js's parser cases and samples, and sabre/vobject's
//! standalone fixtures.
//!
//! Two guarantees are checked for every file:
//!
//! 1. Lenient parsing is total: no panic, no error, on any input — including
//!    the fuzzer crash corpus.
//! 2. Serialization is faithful: writing the parsed model and leniently
//!    reparsing it reproduces the model (excluding files that trip the
//!    inherently ambiguous vCard 2.1 quoted-printable soft-break heuristic).
//!
//! Expected-output comparisons (ical.js jCal pairs, libical recurrence
//! expansions) are wired up separately as the corresponding features land.

use std::fs;
use std::path::{Path, PathBuf};

use vobject_core::model::{Child, Component, Property};
use vobject_core::write::property_line;
use vobject_core::{parse, write_document, ParseOptions, WriteOptions};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .canonicalize()
        .expect("conformance/fixtures missing — run conformance/tools/vendor.py")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every vendored file that is itself vobject data (not expected-output
/// JSON, not RRULE expectation tables, not license text).
fn corpus() -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(&fixtures_root(), &mut files);
    files.retain(|p| {
        let name = p.file_name().unwrap().to_string_lossy();
        let ext = p.extension().map(|e| e.to_string_lossy().to_string());
        match ext.as_deref() {
            Some("ics") | Some("vcf") => true,
            // The fuzz corpus files mostly have no extension.
            _ => p.parent().unwrap().file_name().unwrap() == "fuzz" && name != "LICENSE",
        }
    });
    // 146 data files as of the initial vendoring; guard against silent loss.
    assert!(
        files.len() >= 140,
        "suspiciously small corpus ({} files) — vendoring incomplete?",
        files.len()
    );
    files
}

fn all_properties(comp: &Component) -> Vec<&Property> {
    let mut out = Vec::new();
    let mut stack = vec![comp];
    while let Some(c) = stack.pop() {
        for child in &c.children {
            match child {
                Child::Property(p) => out.push(p),
                Child::Component(k) => stack.push(k),
            }
        }
    }
    out
}

/// Does re-serializing this model risk re-triggering the vCard 2.1
/// quoted-printable soft-break join? (See the property tests for details.)
fn qp_hazard(components: &[Component]) -> bool {
    components.iter().any(|c| {
        all_properties(c).into_iter().any(|p| {
            p.value.contains('=')
                && property_line(p)
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .contains("QUOTED-PRINTABLE")
        })
    })
}

#[test]
fn lenient_parse_is_total_on_corpus() {
    let mut parsed_files = 0;
    let mut components = 0;
    for path in corpus() {
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse(&text, &ParseOptions::lenient())
            .unwrap_or_else(|e| panic!("lenient parse failed on {}: {e}", path.display()));
        parsed_files += 1;
        components += parsed.components.len();
    }
    // Sanity: the corpus is real data, so most files contain components.
    assert!(parsed_files >= 140, "only {parsed_files} files parsed");
    assert!(
        components >= 140,
        "only {components} components across the whole corpus"
    );
}

#[test]
fn serialization_is_faithful_on_corpus() {
    let mut checked = 0;
    let mut skipped_qp = 0;
    for path in corpus() {
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let first = parse(&text, &ParseOptions::lenient()).unwrap();
        if qp_hazard(&first.components) {
            skipped_qp += 1;
            continue;
        }
        let wire = write_document(&first.components, &WriteOptions::default());
        let second = parse(&wire, &ParseOptions::lenient())
            .unwrap_or_else(|e| panic!("reparse failed on {}: {e}", path.display()));
        assert_eq!(
            second.components,
            first.components,
            "round-trip mismatch for {}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 130, "only {checked} files round-tripped");
    // The QP escape hatch must stay an exception, not the rule.
    assert!(
        skipped_qp < 15,
        "{skipped_qp} files skipped as QP hazards — too many"
    );
}

#[test]
fn writer_output_lines_respect_fold_width_on_corpus() {
    for path in corpus() {
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse(&text, &ParseOptions::lenient()).unwrap();
        let wire = write_document(&parsed.components, &WriteOptions::default());
        for line in wire.split("\r\n") {
            assert!(
                line.len() <= 75,
                "overlong line ({} octets) writing {}: {line:?}",
                line.len(),
                path.display()
            );
        }
    }
}
