//! Helpers shared by the test binaries under `tests/`.
//!
//! Each `tests/*.rs` file is compiled as its own binary, so this module is
//! compiled once per file and most binaries only use a subset of it — hence the
//! `dead_code` allowance, which keeps `clippy -D warnings` quiet.

#![allow(dead_code)]

use binary::{Allocator, Decode, Encode, Error, Layout};

pub mod cases;

pub fn round_trip<T>(v: &T) -> T
where
    T: for<'de> Decode<'de> + Encode,
{
    let mut e = Allocator::new();
    v.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    let mut buf: &[u8] = &bytes;
    T::decode_auto(&mut buf).unwrap()
}

/// Number of bytes a length header occupies, detected from the marker bit the
/// same way `decode_prefix` does. Needed because the encoder is free to keep a
/// 4-byte header on short payloads (`append_with_length_prefix` path, `payload > 16`).
pub fn header_width(first: u8) -> usize {
    if first & 0x80 == 0 {
        1
    } else {
        4
    }
}

/// Split a length-prefixed blob into `(header width, payload)`.
pub fn split_prefixed(bytes: &[u8]) -> (usize, &[u8]) {
    let h = header_width(bytes[0]);
    (h, &bytes[h..])
}

/// Build `prefix + payload` with the minimal header width. Used to hand-craft
/// wire input for decoder tests.
pub fn with_header(payload: &[u8]) -> Vec<u8> {
    let h = binary::prefix_header_len(payload.len());
    let mut out = vec![0u8; h + payload.len()];
    binary::encode_prefix(&mut out, payload.len()).unwrap();
    out[h..].copy_from_slice(payload);
    out
}

pub fn round_trip_raw<T>(v: &T) -> T
where
    T: for<'de> Decode<'de> + Encode,
{
    let mut e = Allocator::new();
    v.encode(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    let buf = &bytes;
    T::decode(buf).unwrap()
}

/// A variable-size type with no O(1) length estimate; uses the default
/// `encode_with_prefix` (`append_with_length_prefix` fallback).
pub struct Unpredictable(pub Vec<u8>);

impl Layout for Unpredictable {
    const LEN: usize = 0;
}

impl Encode for Unpredictable {
    fn encode(&self, e: &mut Allocator<'_>) -> Result<(), Error> {
        e.append(&self.0)
    }
}

impl<'de> Decode<'de> for Unpredictable {
    fn decode(payload: &'de [u8]) -> Result<Self, Error> {
        Ok(Unpredictable(payload.to_vec()))
    }
}

/// Decode a lowercase hex string into bytes; panics on odd length or bad digit.
pub fn from_hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex length must be even: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("bad hex digit"))
        .collect()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
