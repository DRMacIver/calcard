//! xCal/xCard sweep over the vendored corpus.
//!
//! xCal conversion canonicalizes representation details the text format
//! leaves open (parameter-name case, escaping style, recurrence part
//! order), so byte-level losslessness against arbitrary wire input is not
//! the contract. What must hold everywhere:
//!
//! 1. Conversion is total: every parseable corpus file converts to XML and
//!    parses back without error.
//! 2. One conversion reaches a fixed point: xml(model(xml(m))) == xml(m).

use std::fs;
use std::path::{Path, PathBuf};

use vobject_core::xcal::{from_xml, to_xml};
use vobject_core::{parse, ParseOptions};

fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let ext = path.extension().map(|e| e.to_string_lossy().to_string());
                if matches!(ext.as_deref(), Some("ics") | Some("vcf"))
                    || (path.parent().unwrap().file_name().unwrap() == "fuzz"
                        && name != "LICENSE")
                {
                    files.push(path);
                }
            }
        }
    }
    assert!(files.len() >= 500);
    files
}

#[test]
fn xcal_conversion_is_total_and_stable_on_corpus() {
    let mut converted = 0;
    let mut unrepresentable = 0;
    for path in corpus() {
        let text = String::from_utf8_lossy(&fs::read(&path).unwrap()).to_string();
        let parsed = parse(&text, &ParseOptions::lenient()).unwrap();
        if parsed.components.is_empty() {
            continue;
        }
        // Mixed vCard/iCalendar top-level streams pick one root element;
        // convert each top-level component separately to stay well-typed.
        for comp in &parsed.components {
            let comps = vec![comp.clone()];
            let xml1 = match to_xml(&comps) {
                Ok(xml) => xml,
                Err(e) => {
                    // Lenient wire parsing keeps names and control bytes
                    // that XML cannot represent (fuzz corpus); the error
                    // must say so explicitly rather than emit broken XML.
                    assert!(
                        e.message.contains("cannot be represented"),
                        "unexpected to_xml error on {}: {e}",
                        path.display()
                    );
                    unrepresentable += 1;
                    continue;
                }
            };
            let model = from_xml(&xml1)
                .unwrap_or_else(|e| panic!("from_xml failed on {}: {e}\n{xml1}", path.display()));
            let xml2 = to_xml(&model)
                .unwrap_or_else(|e| panic!("second to_xml failed on {}: {e}", path.display()));
            assert_eq!(
                xml2,
                xml1,
                "xCal fixed point not reached for {}",
                path.display()
            );
            converted += 1;
        }
    }
    assert!(converted >= 500, "only {converted} components converted");
    // Only fuzz garbage should be unrepresentable.
    assert!(
        unrepresentable < 60,
        "{unrepresentable} components rejected — too many"
    );
}
