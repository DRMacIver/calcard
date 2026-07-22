//! Output line folding (RFC 5545 §3.1).
//!
//! Lines longer than 75 octets (excluding the line break) SHOULD be folded.
//! Folds are inserted only at UTF-8 character boundaries: the RFC permits
//! splitting anywhere between octets, but splitting inside a multi-byte
//! sequence produces physically invalid UTF-8 lines that many consumers
//! (and Rust strings) cannot represent.

/// Standard maximum line length in octets, excluding the terminator.
pub const FOLD_WIDTH: usize = 75;

/// Append `line` to `out`, folding at `width` octets, using `line_ending`
/// as the terminator. Continuation lines start with a single space, which
/// counts toward the width.
///
/// `width` values smaller than 2 are treated as 2 (one octet of payload
/// after the fold marker) so that progress is always made; multi-byte
/// characters may force a line to exceed a tiny width since characters are
/// never split.
pub fn fold_into(out: &mut String, line: &str, width: usize, line_ending: &str) {
    let width = width.max(2);
    let mut budget = width;
    let mut current = String::new();
    for c in line.chars() {
        let len = c.len_utf8();
        if current.len() + len > budget && !current.is_empty() {
            out.push_str(&current);
            out.push_str(line_ending);
            out.push(' ');
            current.clear();
            budget = width - 1; // the fold marker space consumed one octet
        }
        current.push(c);
    }
    out.push_str(&current);
    out.push_str(line_ending);
}

/// Fold a single line to a standalone string with CRLF terminators.
pub fn fold(line: &str) -> String {
    let mut out = String::new();
    fold_into(&mut out, line, FOLD_WIDTH, "\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lines_untouched() {
        assert_eq!(fold("SUMMARY:hello"), "SUMMARY:hello\r\n");
    }

    #[test]
    fn exactly_75_octets_untouched() {
        let line = format!("X:{}", "a".repeat(73));
        assert_eq!(line.len(), 75);
        assert_eq!(fold(&line), format!("{line}\r\n"));
    }

    #[test]
    fn long_line_folds_at_75() {
        let line = format!("X:{}", "a".repeat(100));
        let folded = fold(&line);
        for physical in folded.split("\r\n").filter(|l| !l.is_empty()) {
            assert!(physical.len() <= 75, "line too long: {}", physical.len());
        }
        // Unfolding restores the original.
        let unfolded = crate::lines::unfold(&folded, None).unwrap();
        assert_eq!(unfolded.len(), 1);
        assert_eq!(unfolded[0].text, line);
    }

    #[test]
    fn continuation_lines_account_for_marker() {
        let line = "X".repeat(300);
        let folded = fold(&line);
        let physicals: Vec<&str> = folded.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(physicals[0].len(), 75);
        for cont in &physicals[1..] {
            assert!(cont.starts_with(' '));
            assert!(cont.len() <= 75);
        }
    }

    #[test]
    fn multibyte_never_split() {
        // Snowmen are 3 octets each; 75 is divisible by 3, so alignment
        // varies as we shift with an ASCII prefix.
        for prefix_len in 0..4 {
            let line = format!("{}{}", "a".repeat(prefix_len), "☃".repeat(50));
            let folded = fold(&line);
            for physical in folded.split("\r\n") {
                // Must be valid UTF-8 by construction (it's a &str), but also
                // must not exceed the width.
                assert!(physical.len() <= 75);
            }
            let unfolded = crate::lines::unfold(&folded, None).unwrap();
            assert_eq!(unfolded[0].text, line);
        }
    }

    #[test]
    fn round_trip_many_lengths() {
        for n in 0..200 {
            let line = format!("N:{}", "ab©".repeat(n));
            let folded = fold(&line);
            let unfolded = crate::lines::unfold(&folded, None).unwrap();
            assert_eq!(unfolded.len(), 1, "n={n}");
            assert_eq!(unfolded[0].text, line, "n={n}");
        }
    }

    #[test]
    fn custom_line_ending_and_width() {
        let mut out = String::new();
        fold_into(&mut out, "ABCDEFGHIJ", 5, "\n");
        assert_eq!(out, "ABCDE\n FGHI\n J\n");
    }
}
