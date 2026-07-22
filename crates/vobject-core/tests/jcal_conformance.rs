//! Compare our jCal/jCard output against ical.js's expected-output pairs
//! (conformance/fixtures/icaljs/parser/*.{ics,vcf} + .json).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use vobject_core::jcal::to_jcal;
use vobject_core::{parse, ParseOptions};

fn parser_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures/icaljs/parser")
        .canonicalize()
        .expect("fixtures missing")
}

/// Structural equality with numeric normalization (1 == 1.0).
fn json_eq(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => {
            x.as_f64().unwrap_or(f64::NAN) == y.as_f64().unwrap_or(f64::NAN)
        }
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

/// Path to the first difference, for readable failure output.
fn first_diff(a: &Json, b: &Json, path: String, out: &mut Vec<String>) {
    if out.len() >= 5 {
        return;
    }
    match (a, b) {
        (Json::Array(x), Json::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!(
                    "{path}: array length {} vs expected {}",
                    x.len(),
                    y.len()
                ));
            }
            for (i, (av, bv)) in x.iter().zip(y).enumerate() {
                if !json_eq(av, bv) {
                    first_diff(av, bv, format!("{path}[{i}]"), out);
                }
            }
        }
        (Json::Object(x), Json::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => {
                        if !json_eq(v, w) {
                            first_diff(v, w, format!("{path}.{k}"), out);
                        }
                    }
                    None => out.push(format!("{path}.{k}: unexpected key (ours: {v})")),
                }
            }
            for (k, w) in y {
                if !x.contains_key(k) {
                    out.push(format!("{path}.{k}: missing key (expected: {w})"));
                }
            }
        }
        _ => out.push(format!("{path}: got {a} expected {b}")),
    }
}

#[test]
fn matches_icaljs_expected_output() {
    let dir = parser_dir();
    let mut pairs = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "ics" || ext == "vcf" {
            let expected = path.with_extension("json");
            if expected.exists() {
                pairs.push((path, expected));
            }
        }
    }
    pairs.sort();
    assert!(pairs.len() >= 25, "found only {} pairs", pairs.len());

    let mut failures = Vec::new();
    for (input_path, expected_path) in &pairs {
        let input = fs::read_to_string(input_path).unwrap();
        let expected: Json =
            serde_json::from_str(&fs::read_to_string(expected_path).unwrap()).unwrap();

        let parsed = match parse(&input, &ParseOptions::lenient()) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: parse failed: {e}", input_path.display()));
                continue;
            }
        };

        // Multi-root inputs are compared as arrays; single-root as one value.
        let ours: Json = if parsed.components.len() == 1 {
            to_jcal(&parsed.components[0])
        } else {
            Json::Array(parsed.components.iter().map(to_jcal).collect())
        };

        if !json_eq(&ours, &expected) {
            let mut diffs = Vec::new();
            first_diff(&ours, &expected, "$".to_string(), &mut diffs);
            failures.push(format!(
                "{}:\n    {}",
                input_path.file_name().unwrap().to_string_lossy(),
                diffs.join("\n    ")
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{}/{} jCal comparisons failed:\n{}",
            failures.len(),
            pairs.len(),
            failures.join("\n")
        );
    }
}
