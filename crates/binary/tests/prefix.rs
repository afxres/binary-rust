//! Length-prefix slice API: 1-byte vs 4-byte headers, round trip, and the
//! errors the public writer and reader report.
//!
//! `src/length.rs` is private; this exercises only what `binary` re-exports.
//! The forced-4-byte writer (`encode_prefix_wide`) is not public — the anchor
//! path that needs it is pinned by the `16` / `17` byte assertions in
//! `binary-tests`.

use binary::{decode_slice, encode_slice, Error};

/// The wire length is a signed 31-bit field. Spelled out rather than imported:
/// the literal is the spec.
const MAX_LENGTH: u32 = i32::MAX as u32;

#[test]
fn test_encode_single_byte() {
    let mut buf = [0u8; 4];
    for v in [0u32, 1, 64, 127] {
        buf.fill(0);
        let n = encode_slice(&mut buf, v).unwrap();
        assert_eq!(n, 1);
        assert_eq!(buf[0], v as u8);
        assert_eq!(buf[1], 0);
    }
}

#[test]
fn test_encode_multi_byte() {
    let mut buf = [0u8; 4];
    for v in [128u32, 256, 65536, MAX_LENGTH] {
        buf.fill(0);
        let n = encode_slice(&mut buf, v).unwrap();
        assert_eq!(n, 4);
        let wire = u32::from_be_bytes(buf);
        assert_eq!(wire & 0x7FFF_FFFF, v);
        assert_eq!(wire & 0x8000_0000, 0x8000_0000);
    }
}

#[test]
fn test_decode_single_byte() {
    let (v, read) = decode_slice(&[0x05u8, 0x00, 0x00, 0x00]).unwrap();
    assert_eq!((v, read), (5, 1));
}

#[test]
fn test_decode_multi_byte() {
    let mut buf = [0u8; 8];
    encode_slice(&mut buf, 300).unwrap();
    let (v, read) = decode_slice(&buf).unwrap();
    assert_eq!((v, read), (300, 4));
}

#[test]
fn test_round_trip() {
    for v in [
        0u32, 1, 50, 127, 128, 255, 256, 4096, 65536, 1048576, MAX_LENGTH,
    ] {
        let mut buf = [0u8; 4];
        let written = encode_slice(&mut buf, v).unwrap();
        let (decoded, read) = decode_slice(&buf).unwrap();
        assert_eq!(decoded, v);
        assert_eq!(read, written);
    }
}

#[test]
fn test_encode_truncated_buffer_is_an_error() {
    // Both widths report a too-small destination instead of panicking on the
    // slice.
    let mut empty: [u8; 0] = [];
    assert_eq!(
        encode_slice(&mut empty, 1),
        Err(Error::BufferFull),
        "1-byte header needs 1 byte"
    );

    let mut three = [0u8; 3];
    assert_eq!(
        encode_slice(&mut three, 128),
        Err(Error::BufferFull),
        "4-byte header needs 4 bytes"
    );

    // Nothing was written on the failure path.
    assert_eq!(three, [0, 0, 0]);
}

#[test]
fn test_encode_beyond_i32_max_is_an_error() {
    // `(v as u32) | 0x8000_0000` would keep bit 31 as *data* for values above
    // `i32::MAX`, and the decoder masks that bit away.
    let mut buf = [0u8; 4];
    assert_eq!(
        encode_slice(&mut buf, MAX_LENGTH + 1),
        Err(Error::LengthTooLarge)
    );
    assert_eq!(buf, [0, 0, 0, 0]);
}

#[test]
fn test_decode_empty() {
    let empty: [u8; 0] = [];
    assert_eq!(decode_slice(&empty), Err(Error::NotEnoughBytes));
}

#[test]
fn test_decode_truncated_multi() {
    // Marker bit set: needs 4 bytes, only 3 available.
    assert_eq!(
        decode_slice(&[0x80u8, 0x01, 0x00]),
        Err(Error::NotEnoughBytes)
    );
}
