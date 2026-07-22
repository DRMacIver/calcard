//! Minimal base64 (RFC 4648, standard alphabet) — enough for BINARY values
//! without an external dependency.

use super::ValueError;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        let chars = [
            ALPHABET[(n >> 18 & 63) as usize],
            ALPHABET[(n >> 12 & 63) as usize],
            ALPHABET[(n >> 6 & 63) as usize],
            ALPHABET[(n & 63) as usize],
        ];
        let keep = chunk.len() + 1;
        for (i, c) in chars.iter().enumerate() {
            out.push(if i < keep { *c as char } else { '=' });
        }
    }
    out
}

fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode base64, ignoring ASCII whitespace (folded BINARY values may
/// contain none after unfolding, but be liberal).
pub fn decode(s: &str) -> Result<Vec<u8>, ValueError> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u8;
    let mut padding = 0u8;
    for &c in s.as_bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return Err(ValueError::new("base64 data after padding"));
        }
        let v = decode_char(c)
            .ok_or_else(|| ValueError::new(format!("invalid base64 character {:?}", c as char)))?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if padding > 2 {
        return Err(ValueError::new("too much base64 padding"));
    }
    // Leftover bits must be zero (canonical encoding not enforced, but
    // partial groups of 1 sextet are impossible).
    if bits >= 6 {
        return Err(ValueError::new("truncated base64 data"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for data in [
            b"".as_slice(),
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0u8, 255, 128, 7, 9],
        ] {
            let encoded = encode(data);
            assert_eq!(decode(&encoded).unwrap(), data, "{encoded}");
        }
    }

    #[test]
    fn known_vectors() {
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(decode("Zm9vYg==").unwrap(), b"foob");
        assert_eq!(decode("Zm9v\r\n YmFy").unwrap(), b"foobar");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("a").is_err());
        assert!(decode("ab=c").is_err());
        assert!(decode("****").is_err());
    }
}
