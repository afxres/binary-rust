//! Length-prefix codec: 1-byte vs 4-byte headers and their round trip.
//!
//! Kept out of `src/length.rs` so `src/` holds implementation only; the module
//! is private, and everything exercised here is re-exported at the crate root.

use binary::{decode_prefix, encode_prefix, encode_prefix_wide, prefix_header_len, Error};

/// Same value as `binary::length::MAX_LENGTH`, which is not re-exported. The
/// literal is the spec (signed 31-bit), so spelling it out is the point.
const MAX_LENGTH: usize = i32::MAX as usize;

#[test]
fn test_header_len() {
    assert_eq!(prefix_header_len(0), 1);
    assert_eq!(prefix_header_len(1), 1);
    assert_eq!(prefix_header_len(127), 1);
    assert_eq!(prefix_header_len(128), 4);
    assert_eq!(prefix_header_len(1024), 4);
    assert_eq!(prefix_header_len(MAX_LENGTH), 4);
}

#[test]
fn test_encode_single_byte() {
    let mut buf = [0u8; 4];
    for len in [0usize, 1, 64, 127] {
        buf.fill(0);
        let n = encode_prefix(&mut buf, len).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], len as u8);
        assert_eq!(buf[1], 0);
    }
}

#[test]
fn test_encode_multi_byte() {
    let mut buf = [0u8; 4];
    for len in [128usize, 256, 65536, MAX_LENGTH] {
        buf.fill(0);
        let n = encode_prefix(&mut buf, len).unwrap();
        assert_eq!(n, 4);
        let v = u32::from_be_bytes(buf);
        assert_eq!(v & 0x7FFF_FFFF, len as u32);
        assert_eq!(v & 0x8000_0000, 0x8000_0000);
    }
}

/// `encode_prefix_wide` keeps the 4-byte width even for short lengths — the
/// `write_prefixed` path relies on it, otherwise three stale zero bytes stay in front of
/// the payload.
#[test]
fn test_encode_wide_for_short_length() {
    let mut buf = [0u8; 4];
    let n = encode_prefix_wide(&mut buf, 5).unwrap();
    assert_eq!(n, 4);
    assert_eq!(buf, [0x80, 0x00, 0x00, 0x05]);
    let mut pos = 0;
    assert_eq!(decode_prefix(&buf, &mut pos).unwrap(), 5);
    assert_eq!(pos, 4);
}

#[test]
fn test_decode_single_byte() {
    let buf = [0x05u8, 0x00, 0x00, 0x00];
    let mut pos = 0;
    assert_eq!(decode_prefix(&buf, &mut pos).unwrap(), 5);
    assert_eq!(pos, 1);
}

#[test]
fn test_decode_multi_byte() {
    let mut buf = [0u8; 8];
    encode_prefix(&mut buf, 300).unwrap();
    let mut pos = 0;
    assert_eq!(decode_prefix(&buf, &mut pos).unwrap(), 300);
    assert_eq!(pos, 4);
}

#[test]
fn test_decode_round_trip() {
    for len in [
        0usize, 1, 50, 127, 128, 255, 256, 4096, 65536, 1048576, MAX_LENGTH,
    ] {
        let mut buf = [0u8; 4];
        let written = encode_prefix(&mut buf, len).unwrap();
        let mut pos = 0;
        let decoded = decode_prefix(&buf, &mut pos).unwrap();
        assert_eq!(decoded, len);
        assert_eq!(pos, written);
    }
}

#[test]
fn test_encode_truncated_buffer_is_an_error() {
    // Both writers report a too-small destination instead of panicking on the
    // slice (the old code only had `debug_assert!`, which is absent in release).
    let mut empty: [u8; 0] = [];
    assert_eq!(
        encode_prefix(&mut empty, 1),
        Err(Error::BufferFull),
        "1-byte header needs 1 byte"
    );
    assert_eq!(
        encode_prefix_wide(&mut empty, 1),
        Err(Error::BufferFull),
        "wide header needs 4 bytes"
    );

    let mut three = [0u8; 3];
    assert_eq!(encode_prefix_wide(&mut three, 1), Err(Error::BufferFull));

    // Nothing was written on the failure path.
    assert_eq!(three, [0, 0, 0]);
}

#[test]
fn test_encode_beyond_i32_max_is_an_error() {
    // Same guard the encoder-level callers rely on: the wire length is a signed
    // 31-bit field, and `(len as u32) | 0x8000_0000` would keep bit 31 as data.
    let mut buf = [0u8; 4];
    assert_eq!(
        encode_prefix(&mut buf, MAX_LENGTH + 1),
        Err(Error::LengthTooLarge)
    );
    assert_eq!(
        encode_prefix_wide(&mut buf, MAX_LENGTH + 1),
        Err(Error::LengthTooLarge)
    );
    assert_eq!(buf, [0, 0, 0, 0]);
}

#[test]
fn test_decode_empty() {
    let buf: [u8; 0] = [];
    let mut pos = 0;
    assert_eq!(decode_prefix(&buf, &mut pos), Err(Error::NotEnoughBytes));
}

#[test]
fn test_decode_truncated_multi() {
    let buf = [0x80u8, 0x01, 0x00]; // needs 4 bytes, only 3 available
    let mut pos = 0;
    assert_eq!(decode_prefix(&buf, &mut pos), Err(Error::NotEnoughBytes));
}
