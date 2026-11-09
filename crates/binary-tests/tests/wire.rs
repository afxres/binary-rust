//! Base wire tests: integers, strings, containers, and the prefix /
//! `write_prefixed` boundaries.

mod common;

use binary::{Decode, Decoder, Encode, Encoder, Error, Layout};
use common::*;

/// Test-local copy of the prefix compaction threshold. The library keeps it
/// private on purpose (it is an encoder canonicalization policy, not a
/// decoding rule), so the boundary is pinned here as a wire fact: C#
/// (`Limits = 16`) and C++ (`AllocatorAnchorShrinkLimits = 16`). If the
/// constant ever drifts, this test fails on the 16 / 17 boundary below.
const PREFIX_SHRINK_LIMIT: usize = 16;

#[test]
fn test_integers() {
    assert_eq!(round_trip(&42i32), 42);
    assert_eq!(round_trip(&-1i32), -1);
    assert_eq!(round_trip(&0u8), 0);
    assert_eq!(round_trip(&255u8), 255);
    assert_eq!(round_trip(&12345u16), 12345);
    assert_eq!(round_trip(&-789i64), -789);
    assert_eq!(round_trip(&u128::MAX), u128::MAX);
    assert_eq!(round_trip(&0.125f64), 0.125);
    assert!(round_trip(&true));
    assert!(!round_trip(&false));
}

#[test]
fn test_raw_integers() {
    assert_eq!(round_trip_raw(&42i32), 42);
    assert_eq!(round_trip_raw(&255u8), 255);
}

#[test]
fn test_string() {
    assert_eq!(round_trip(&"hello".to_string()), "hello");
    assert_eq!(round_trip(&"".to_string()), "");
    assert_eq!(
        round_trip(&"Unicode: 你好世界".to_string()),
        "Unicode: 你好世界"
    );
}

#[test]
fn test_str_borrowed() {
    let mut e = Encoder::new();
    "test".encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes);
    let s: &str = Decode::decode_auto(&mut d).unwrap();
    assert_eq!(s, "test");
}

#[test]
fn test_vec_int() {
    assert_eq!(round_trip(&vec![1i32, 2, 3, 4, 5]), vec![1, 2, 3, 4, 5]);
    assert_eq!(round_trip(&Vec::<u8>::new()), Vec::<u8>::new());
    assert_eq!(round_trip(&vec![0u8; 256]), vec![0u8; 256]);
}

#[test]
fn test_vec_string() {
    assert_eq!(
        round_trip(&vec!["a".to_string(), "bb".to_string(), "ccc".to_string()]),
        vec!["a", "bb", "ccc"]
    );
}

#[test]
fn test_bulk_and_loop_containers_are_byte_identical() {
    // `Vec<T>` and `[T; N]` hand their payload to `T::encode_slice`, which is
    // the per-element loop for every type that does not override it (all of
    // them outside this crate) and one bulk write for the fixed-width scalars
    // on a little-endian target. That is an optimization, not a wire change:
    // every case below must come out exactly like the loop (little endian, no
    // padding). See `test_wire_order_is_endian_independent`.
    let v = vec![1i32, -2, 0x0102_0304];
    assert_eq!(
        auto_bytes(&v),
        [
            0x0c, // payload = 3 * 4 bytes
            0x01, 0x00, 0x00, 0x00, //
            0xfe, 0xff, 0xff, 0xff, //
            0x04, 0x03, 0x02, 0x01,
        ]
    );
    assert_eq!(round_trip(&v), v);

    // `u8` takes the bulk arm on every target. `bool` deliberately never
    // does (see `test_bool_and_char_validate_on_the_way_in`): its decoder is
    // lenient, so its encoder must still produce exactly the same wire.
    assert_eq!(auto_bytes(&vec![1u8, 2, 3]), [0x03, 1, 2, 3]);
    assert_eq!(auto_bytes(&vec![true, false, true]), [0x03, 1, 0, 1]);
    assert_eq!(auto_bytes(&Vec::<i32>::new()), [0x00]);

    // `char` has no bulk arm either (unspecified in-memory layout +
    // restricted scalar range), so this exercises the loop and must agree
    // with the bulk one.
    assert_eq!(
        auto_bytes(&vec!['a', '中']),
        [0x08, 0x61, 0x00, 0x00, 0x00, 0x2d, 0x4e, 0x00, 0x00]
    );

    // Arrays share the helper; fixed-size elements carry no inner prefix.
    assert_eq!(
        auto_bytes(&[1u32, 2]),
        [0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00]
    );

    // Variable-size elements still go through the loop, prefixed per element.
    assert_eq!(auto_bytes(&vec!["a".to_string()]), [0x02, 0x01, b'a']);
}

#[test]
fn test_byte_slice_borrowed() {
    let mut e = Encoder::new();
    [1u8, 2, 3, 4][..].encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    let mut d = Decoder::new(&bytes);
    let s: &[u8] = Decode::decode_auto(&mut d).unwrap();
    assert_eq!(s, &[1, 2, 3, 4]);
}

#[test]
fn test_option() {
    assert_eq!(round_trip(&Some(42i32)), Some(42));
    assert_eq!(round_trip(&None::<i32>), None);
    assert_eq!(round_trip(&Some("hi".to_string())), Some("hi".to_string()));
}

#[test]
fn test_tuple() {
    assert_eq!(
        round_trip(&(1i32, "hello".to_string())),
        (1, "hello".to_string())
    );
    assert_eq!(round_trip(&(1i32, 2u64, 3.0f64)), (1, 2, 3.0));
}

#[test]
fn test_wide_tuples() {
    // The impls are generated for 1..=12: 6 was the first arity the old
    // hand-written set (1..=5) could not encode, 12 is where `std` itself stops
    // implementing `Debug` / `Clone` for tuples.
    let six = (1u8, 2u16, 3u32, 4u64, 5i8, 6i16);
    assert_eq!(round_trip(&six), six);
    assert_eq!(<(u8, u16, u32, u64, i8, i16) as Layout>::LEN, 18);

    #[allow(clippy::type_complexity)]
    let twelve = (
        1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 9u8, 10u8, 11u8, 12u8,
    );
    assert_eq!(round_trip(&twelve), twelve);
    // All members fixed → `LEN` is the sum, and `auto` adds no header of its own.
    assert_eq!(
        <(u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8) as Layout>::LEN,
        12
    );
    assert_eq!(auto_bytes(&twelve), (1u8..=12).collect::<Vec<_>>());
}

#[test]
fn test_wide_tuple_last_member_is_raw() {
    // The last-member rule must survive macro expansion at every arity: `raw`
    // drops the final member's prefix, `auto` keeps it and adds no tuple header.
    let v = (1u8, 2u8, 3u8, 4u8, 5u8, String::from("x"));
    assert_eq!(round_trip(&v), v);
    assert_eq!(round_trip_raw(&v), v);

    let mut e = Encoder::new();
    v.encode(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), [1, 2, 3, 4, 5, b'x']);

    assert_eq!(auto_bytes(&v), [1, 2, 3, 4, 5, 1, b'x']);
}

#[test]
fn test_ten_element_tuple_matches_csharp() {
    // C# already covers this arity in `Binary.SourceGeneration.TupleObjectTests/
    // SystemTupleTypes/LargeValueTupleTests.cs` (10 members, `converter.Length
    // == 62`, raw + auto round trip), but that is a self round trip and pins no
    // bytes. The vector below was printed by the real C# library
    // (`Generator.CreateDefault()`, `Encode` / `Converter.EncodeAuto`) for the
    // same values, and both sides agree byte for byte.
    //
    // It also pins the flattening rule: C# represents 10 members as
    // ValueTuple<..., ValueTuple<long, UInt128, Int128>>, and `Length == 62`
    // is exactly the flat sum (1+1+2+2+4+4+8+8+16+16) — the nested `TRest`
    // contributes no header of its own, same as the Rust side.
    #[allow(clippy::type_complexity)]
    let v = (
        1u8, -2i8, 3u16, -4i16, 5u32, -6i32, 7u64, -8i64, 9u128, -10i128,
    );
    type T = (u8, i8, u16, i16, u32, i32, u64, i64, u128, i128);
    assert_eq!(<T as Layout>::LEN, 62);

    let expected: [u8; 62] = [
        0x01, 0xfe, // 1u8, -2i8
        0x03, 0x00, // 3u16
        0xfc, 0xff, // -4i16
        0x05, 0x00, 0x00, 0x00, // 5u32
        0xfa, 0xff, 0xff, 0xff, // -6i32
        0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 7u64
        0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // -8i64
        0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 9u128
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0xf6, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // -10i128
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, //
    ];

    // All members are fixed size, so `auto` is indistinguishable from `raw`.
    assert_eq!(auto_bytes(&v), expected);
    assert_eq!(round_trip(&v), v);

    // ... and the bytes C# produced decode back to the same value.
    let mut d = Decoder::new(&expected);
    let back: T = Decode::decode_auto(&mut d).unwrap();
    assert_eq!(back, v);
    assert_eq!(d.remaining(), 0);
}

#[test]
fn test_empty_tuple() {
    assert_eq!(round_trip(&()), ());
}

#[test]
fn test_array() {
    assert_eq!(round_trip(&[1i32, 2, 3, 4]), [1, 2, 3, 4]);
    assert_eq!(round_trip(&[0u8; 16]), [0u8; 16]);
}

#[test]
fn test_nested() {
    let v = vec![
        (1i32, "a".to_string()),
        (2, "bb".to_string()),
        (3, "ccc".to_string()),
    ];
    assert_eq!(round_trip(&v), v);
}

#[test]
fn test_decode_encode_idempotent() {
    // decode → encode should produce the same bytes
    let original = "Hello, World!";
    let mut e = Encoder::new();
    original.encode_auto(&mut e).unwrap();
    let bytes1 = e.finish().unwrap();

    // decode
    let mut d = Decoder::new(&bytes1);
    let s: String = Decode::decode_auto(&mut d).unwrap();

    // re-encode
    let mut e2 = Encoder::new();
    s.encode_auto(&mut e2).unwrap();
    let bytes2 = e2.finish().unwrap();

    assert_eq!(bytes1, bytes2);
}

#[test]
fn test_borrowed_encoder_full() {
    let mut buf = [0u8; 4];
    let mut e = Encoder::on_slice(&mut buf);
    e.write(&[1, 2, 3, 4]).unwrap();
    // A distinct variant, not just "some error": the caller can tell "my buffer
    // is too small" from "the data is bad".
    assert_eq!(e.write(&[5]).unwrap_err(), Error::BufferFull);
}

#[test]
fn test_large_string() {
    let s = "x".repeat(1000);
    assert_eq!(round_trip(&s), s);
}

#[test]
fn test_long_prefix_boundary() {
    // 127 bytes → 1-byte prefix
    let s127 = "x".repeat(127);
    let mut e = Encoder::new();
    s127.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(bytes[0], 127);
    assert_eq!(bytes.len(), 128);

    // 128 bytes → 4-byte prefix
    let s128 = "x".repeat(128);
    let mut e = Encoder::new();
    s128.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(bytes[0] & 0x80, 0x80);
    assert_eq!(bytes.len(), 132);
}

#[test]
fn test_empty_decode_error() {
    // decode empty → should error
    let mut d = Decoder::new(&[]);
    let result = String::decode_auto(&mut d);
    assert!(result.is_err());
}

#[test]
fn test_write_prefixed_bounded_scope() {
    // Encoder side uses write_prefixed_bounded; decoder side still has with_prefix.
    let mut e = Encoder::new();
    e.write_prefixed_bounded(5, |e| e.write(b"hello")).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(bytes, &[5, b'h', b'e', b'l', b'l', b'o']);

    let mut d = Decoder::new(&bytes);
    let result: String = d
        .with_prefix(|d| {
            let s = d.take(5)?;
            String::from_utf8(s.to_vec()).map_err(|_| Error::message_static("bad utf-8"))
        })
        .unwrap();
    assert_eq!(result, "hello");
}

#[test]
fn test_write_prefixed_shrink_boundary() {
    // Up to PREFIX_SHRINK_LIMIT the `write_prefixed` path compacts to a 1-byte header;
    // above it the reserved 4-byte header is kept. This threshold is a wire
    // contract shared with C# (`Limits = 16`) and C++
    // (`AllocatorAnchorShrinkLimits = 16`), pinned by the two byte-level
    // assertions below rather than by naming a library constant.
    let at_limit = Unpredictable(vec![0xAB; PREFIX_SHRINK_LIMIT]);
    let mut e = Encoder::new();
    at_limit.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(bytes[0] as usize, PREFIX_SHRINK_LIMIT, "16 → 1-byte header");
    assert_eq!(bytes.len(), 1 + PREFIX_SHRINK_LIMIT);

    let over = Unpredictable(vec![0xAB; PREFIX_SHRINK_LIMIT + 1]);
    let mut e = Encoder::new();
    over.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(&bytes[..4], &[0x80, 0x00, 0x00, 0x11], "17 → 4-byte header");
    assert_eq!(bytes.len(), 4 + PREFIX_SHRINK_LIMIT + 1);

    let decoded: Unpredictable = {
        let mut d = Decoder::new(&bytes);
        Decode::decode_auto(&mut d).unwrap()
    };
    assert_eq!(decoded.0.len(), PREFIX_SHRINK_LIMIT + 1);
}

/// C#/C++ move the payload with one or two fixed-size `u64` byte moves. Every
/// length in 0..=16 must land byte-for-byte, especially the two-block cases
/// (9..=16) where the second block's destination overlaps the first block's
/// source. 17 keeps the wide header.
#[test]
fn test_write_prefixed_u64_moves_all_lengths() {
    for len in 0..=PREFIX_SHRINK_LIMIT + 1 {
        let payload: Vec<u8> = (0..len)
            .map(|i| (i as u8).wrapping_mul(37).wrapping_add(0x5a))
            .collect();
        let mut e = Encoder::new();
        Unpredictable(payload.clone()).encode_auto(&mut e).unwrap();
        let bytes = e.finish().unwrap();

        if len <= PREFIX_SHRINK_LIMIT {
            assert_eq!(bytes[0] as usize, len, "len {len}: 1-byte header");
            assert_eq!(&bytes[1..], &payload[..], "len {len}: moved payload");
            assert_eq!(
                bytes.len(),
                1 + len,
                "len {len}: tail margin is not in the result"
            );
        } else {
            assert_eq!(
                &bytes[..4],
                &[0x80, 0x00, 0x00, len as u8],
                "len {len}: wide header"
            );
            assert_eq!(&bytes[4..], &payload[..], "len {len}: payload");
        }

        let mut d = Decoder::new(&bytes);
        let decoded: Unpredictable = Decode::decode_auto(&mut d).unwrap();
        assert_eq!(decoded.0, payload, "len {len}: round trip");
    }
}

/// A borrowed buffer decides the tail margin. C#/C++ keep the 4-byte header
/// when `bounds - offset < ((-length) & 7)`; both widths must decode. Sizing
/// the buffer up front keeps this on the compacting path.
#[test]
fn test_write_prefixed_borrowed_tail_margin() {
    for len in 0..=PREFIX_SHRINK_LIMIT {
        let payload: Vec<u8> = (0..len).map(|i| (i as u8) ^ 0xa5).collect();
        let pad = (8 - (len & 7)) & 7;

        // Roomy buffer: same fast path as the owned backend, always compacts.
        let mut roomy = vec![0xcc; 64];
        let mut e = Encoder::on_slice(&mut roomy);
        Unpredictable(payload.clone()).encode_auto(&mut e).unwrap();
        assert_eq!(e.as_bytes()[0] as usize, len, "len {len}: roomy compacts");
        assert_eq!(&e.as_bytes()[1..], &payload[..], "len {len}: roomy payload");

        // One byte short of the required margin: the 4-byte header is kept.
        if pad > 0 {
            let mut tight = vec![0xcc; 4 + len + pad - 1];
            let mut e = Encoder::on_slice(&mut tight);
            Unpredictable(payload.clone()).encode_auto(&mut e).unwrap();
            assert_eq!(
                &e.as_bytes()[..4],
                &[0x80, 0x00, 0x00, len as u8],
                "len {len}: insufficient margin keeps wide header"
            );
            assert_eq!(&e.as_bytes()[4..], &payload[..], "len {len}: wide payload");
            let mut d = Decoder::new(e.as_bytes());
            let decoded: Unpredictable = Decode::decode_auto(&mut d).unwrap();
            assert_eq!(decoded.0, payload, "len {len}: wide round trip");
        }

        // Exactly enough margin: compacts.
        let mut exact = vec![0xcc; 4 + len + pad];
        let mut e = Encoder::on_slice(&mut exact);
        Unpredictable(payload.clone()).encode_auto(&mut e).unwrap();
        assert_eq!(
            e.as_bytes()[0] as usize,
            len,
            "len {len}: exact margin compacts"
        );
        assert_eq!(&e.as_bytes()[1..], &payload[..], "len {len}: exact payload");
    }
}

#[test]
fn test_write_prefixed_wire_matches_csharp() {
    // Real C# output for List<string>{"aaaa","bbbb","cccc","dddd"} is
    // `80 00 00 14 | 04 "aaaa" | 04 "bbbb" | 04 "cccc" | 04 "dddd"`:
    // the element payload totals 20 bytes, so `write_prefixed` keeps the 4-byte form.
    let v: Vec<String> = ["aaaa", "bbbb", "cccc", "dddd"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(&bytes[..4], &[0x80, 0x00, 0x00, 0x14]);
    assert_eq!(bytes.len(), 4 + 20);
}

#[test]
fn test_write_prefixed_path() {
    // No O(1) length estimate → default encode_with_prefix uses the `write_prefixed` path.
    let v = Unpredictable(vec![1u8, 2, 3, 4, 5]);
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    let decoded: Unpredictable = Decode::decode_auto(&mut d).unwrap();
    assert_eq!(decoded.0, vec![1u8, 2, 3, 4, 5]);
}

#[test]
fn test_write_prefixed_bounded_writes_actual_length() {
    // The estimate is an upper bound; only bytes actually written count.
    let mut e = Encoder::new();
    e.write_prefixed_bounded(8, |e| e.write(&[1u8, 2, 3, 4]))
        .unwrap();
    // 1-byte header (estimate 8 <= 127) + 4 bytes actually written
    assert_eq!(e.finish().unwrap(), &[4, 1, 2, 3, 4]);
}

#[test]
fn test_write_prefixed_bounded_overshoot_is_an_error() {
    // `max_len` is a bound, not a reservation: `f` writes into the encoder
    // itself, so a payload bigger than the declared bound can only be rejected
    // afterwards. That is a caller bug, not bad data, and the error is terminal
    // (the header was never filled in) — the same class as a failed
    // `write_prefixed`. What matters here is that it is *reported*: the old
    // bounded sub-encoder returned `BufferFull`, which claims the caller's
    // buffer is too small, and an owned encoder has no such buffer.
    let mut e = Encoder::new();
    e.write(&[0xAA]).unwrap();
    let err = e
        .write_prefixed_bounded(2, |e| e.write(&[1u8, 2, 3]))
        .unwrap_err();
    assert!(matches!(err, Error::Message(_)), "{err:?}");
}

#[test]
fn test_write_prefixed_bounded_estimate_is_not_charged_to_borrowed_space() {
    // An over-estimate must not decide whether a borrowed encoder accepts the
    // payload: only the bytes `f` writes are checked against `buf.len()`. The
    // estimate stops at the 1-byte-header range (127) on purpose — a wider
    // estimate legitimately needs a 4-byte header, which this buffer cannot hold.
    // (Charging `max_len` to the buffer used to fail here with BufferFull.)
    let mut buf = [0u8; 6];
    let mut e = Encoder::on_slice(&mut buf);
    e.write_prefixed_bounded(127, |e| e.write(b"hello"))
        .unwrap();
    assert_eq!(e.as_bytes(), &[5, b'h', b'e', b'l', b'l', b'o']);
}

#[test]
fn test_write_prefixed_bounded_inner_failure_propagates() {
    // A genuine BufferFull from `f` — the buffer really is too small — comes
    // straight back, exactly as it would from a bare `write`.
    let mut buf = [0u8; 4];
    let mut e = Encoder::on_slice(&mut buf);
    e.write(&[0xAA]).unwrap();
    let err = e
        .write_prefixed_bounded(8, |e| e.write(&[1u8, 2, 3, 4]))
        .unwrap_err();
    assert_eq!(err, Error::BufferFull);
}

#[test]
fn test_unit_has_length_prefix() {
    // `()` is zero-sized but classified as variable-length: `auto` must write
    // a zero-length prefix so containers can count elements.
    let mut e = Encoder::new();
    ().encode_auto(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), vec![0x00]);
    assert_eq!(round_trip(&()), ());
}

#[test]
fn test_vec_of_unit_preserves_count() {
    let v = vec![(), (), ()];
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    // outer prefix (3) + one 0x00 prefix per element
    assert_eq!(bytes, vec![3, 0, 0, 0]);
    assert_eq!(round_trip(&v), v);
}

#[test]
fn test_array_of_unit_preserves_count() {
    let a = [(), (), ()];
    let mut e = Encoder::new();
    a.encode_auto(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), vec![3, 0, 0, 0]);
    assert_eq!(round_trip(&a), a);
}

#[test]
fn test_option_of_unit() {
    assert_eq!(round_trip(&Some(())), Some(()));
    assert_eq!(round_trip(&None::<()>), None);
}

#[test]
fn test_tuple_flatten_equals_nested() {
    // C# / C++ README 的规范例子：任意层级嵌套的 tuple 与直接平铺的 wire 必须完全相等。
    let nested = (
        (1712i64, 500i32),
        ("amy".to_string(), 7i32),
        "login".to_string(),
    );
    let flat = (
        1712i64,
        500i32,
        "amy".to_string(),
        7i32,
        "login".to_string(),
    );

    let mut e = Encoder::new();
    nested.encode(&mut e).unwrap();
    let nested_bytes = e.finish().unwrap();

    let mut e = Encoder::new();
    flat.encode(&mut e).unwrap();
    let flat_bytes = e.finish().unwrap();

    let expected = vec![
        0xb0, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 1712i64 LE
        0xf4, 0x01, 0x00, 0x00, // 500i32 LE
        0x03, b'a', b'm', b'y', // "amy" + prefix
        0x07, 0x00, 0x00, 0x00, // 7i32 LE
        b'l', b'o', b'g', b'i', b'n', // "login", last element, no prefix
    ];
    assert_eq!(nested_bytes, expected, "nested tuple wire");
    assert_eq!(flat_bytes, expected, "flat tuple wire");

    // Both types decode a payload written by the other.
    let mut d = Decoder::new(&nested_bytes);
    let decoded_flat = <(i64, i32, String, i32, String)>::decode(&mut d).unwrap();
    assert_eq!(decoded_flat, flat);

    let mut d = Decoder::new(&flat_bytes);
    let decoded_nested = <((i64, i32), (String, i32), String)>::decode(&mut d).unwrap();
    assert_eq!(decoded_nested, nested);
}

#[test]
fn test_no_recursive_length_prediction() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct Counting {
        calls: Rc<Cell<usize>>,
    }

    impl Layout for Counting {
        const LEN: usize = 0;
    }

    impl Encode for Counting {
        fn encode(&self, e: &mut Encoder<'_>) -> Result<(), Error> {
            self.calls.set(self.calls.get() + 1);
            e.write(&[1, 2])
        }
    }

    // Depth 2: each value is encoded exactly once, and the wire is unchanged.
    let calls = Rc::new(Cell::new(0));
    let v = vec![
        Counting {
            calls: Rc::clone(&calls),
        },
        Counting {
            calls: Rc::clone(&calls),
        },
    ];
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), vec![6, 2, 1, 2, 2, 1, 2]);
    assert_eq!(calls.get(), 2);

    // Depth 3: still one encode call per leaf — no repeated traversal.
    let calls3 = Rc::new(Cell::new(0));
    let inner = vec![
        Counting {
            calls: Rc::clone(&calls3),
        },
        Counting {
            calls: Rc::clone(&calls3),
        },
    ];
    let outer = vec![inner];
    let mut e = Encoder::new();
    outer.encode_auto(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), vec![7, 6, 2, 1, 2, 2, 1, 2]);
    assert_eq!(calls3.get(), 2);
}

#[test]
fn test_compact_int_width() {
    // The union tag is a "compact integer": wire-identical to a length prefix,
    // because C# encodes both through the same `NumberModule.Encode`.
    let mut e = Encoder::new();
    e.write_compact_int(0).unwrap();
    e.write_compact_int(127).unwrap();
    assert_eq!(e.as_bytes(), &[0x00, 0x7F]);

    let mut e = Encoder::new();
    e.write_compact_int(128).unwrap();
    assert_eq!(e.as_bytes(), &[0x80, 0x00, 0x00, 0x80]);

    for v in [0u32, 1, 127, 128, 255, 65_535, i32::MAX as u32] {
        let mut e = Encoder::new();
        e.write_compact_int(v).unwrap();
        let bytes = e.finish().unwrap();
        assert_eq!(bytes.len(), binary::prefix_header_len(v as usize));
        let mut d = Decoder::new(&bytes);
        assert_eq!(d.read_compact_int().unwrap(), v);
        assert_eq!(d.remaining(), 0);
    }
}

#[test]
fn test_compact_int_truncated() {
    let mut d = Decoder::new(&[0x80, 0x00, 0x00]);
    assert_eq!(d.read_compact_int(), Err(Error::NotEnoughBytes));
}

// ---- `Layout::LEN` ----

fn auto_bytes<T: Encode>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    e.finish().unwrap()
}

fn from_auto<T: for<'de> Decode<'de>>(bytes: &[u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode_auto(&mut d).unwrap()
}

/// `LEN` is a plain `usize` in which `0` means variable-length — the same
/// sentinel C# `Converter<T>.Length` and C++ `Converter<T>::Length()` branch on
/// (`if (length != 0) raw else prefix + raw`). The point of the type is that
/// "fixed at 0 bytes" is no longer representable, so there is no illegal state
/// to document or assert against.
#[test]
fn test_layout_len_values() {
    assert_eq!(<i32 as Layout>::LEN, 4);
    assert_eq!(<u8 as Layout>::LEN, 1);
    assert_eq!(<u128 as Layout>::LEN, 16);
    assert_eq!(<bool as Layout>::LEN, 1);
    assert_eq!(<char as Layout>::LEN, 4);
    assert_eq!(<usize as Layout>::LEN, core::mem::size_of::<usize>());

    // Variable-length types.
    assert_eq!(<str as Layout>::LEN, 0);
    assert_eq!(<String as Layout>::LEN, 0);
    assert_eq!(<[u8] as Layout>::LEN, 0);
    assert_eq!(<Vec<i32> as Layout>::LEN, 0);
    assert_eq!(<Option<i32> as Layout>::LEN, 0);

    // Zero-sized types are variable, not "fixed at 0 bytes": a container has to
    // recover the element count from somewhere.
    assert_eq!(<() as Layout>::LEN, 0);
    assert_eq!(<[i32; 0] as Layout>::LEN, 0);
    assert_eq!(<(i32, ()) as Layout>::LEN, 0);

    // Composition: sums only when every member is fixed.
    assert_eq!(<(i32, i32) as Layout>::LEN, 8);
    assert_eq!(<(i32, i32, i32) as Layout>::LEN, 12);
    assert_eq!(<(u8, u16, u32, u64) as Layout>::LEN, 15);
    assert_eq!(<(u8, u16, u32, u64, u128) as Layout>::LEN, 31);
    assert_eq!(<[i32; 3] as Layout>::LEN, 12);
    assert_eq!(<[String; 3] as Layout>::LEN, 0);
}

/// Regression for the reason `Option<usize>` was dropped: `[T; 0]` used to
/// compute `Some(0)`, the one state the old trait documented as invalid.
/// `encode_auto` then hit `debug_assert!(n > 0)` and panicked in debug builds;
/// in release it wrote no prefix at all, so `Vec<[i32; 0]>` lost its count.
/// Variable-length is both truthful and safe: one empty prefix per element,
/// which is also the shape C# gives an empty array (`00`, cf. `string_empty`).
#[test]
fn test_zero_length_array_is_variable() {
    let empty: [i32; 0] = [];
    assert_eq!(auto_bytes(&empty), [0x00]);
    assert_eq!(from_auto::<[i32; 0]>(&[0x00]), empty);

    let v = vec![[0i32; 0]; 3];
    let bytes = auto_bytes(&v);
    assert_eq!(bytes, [0x03, 0x00, 0x00, 0x00]);
    assert_eq!(from_auto::<Vec<[i32; 0]>>(&bytes).len(), 3);
}

/// Same reasoning one level up: a zero-sized member makes the tuple
/// variable-length, so an enclosing `Vec` still counts elements. Were `()` ever
/// classified as "fixed 0", `(i32, ())` would collapse to 4 and the elements
/// below would become indistinguishable.
#[test]
fn test_tuple_with_zst_member_stays_variable() {
    let v = vec![(1i32, ()), (2, ())];
    let bytes = auto_bytes(&v);
    // 0x0A payload: [01 00 00 00][00] [02 00 00 00][00]
    assert_eq!(bytes, [0x0A, 1, 0, 0, 0, 0x00, 2, 0, 0, 0, 0x00]);
    assert_eq!(from_auto::<Vec<(i32, ())>>(&bytes), v);
}

#[test]
fn test_oneshot_helpers_round_trip() {
    // One-shot helpers are the raw form: no length prefix, the whole buffer is
    // the payload (C# `Converter<T>::Encode` / `Decode`, C++ same). Contrast
    // with `encode_auto`, which prefixes variable-length values.
    let v = (1024i32, String::from("csharp"), vec![7i32, 11]);
    let bytes = binary::to_vec(&v).unwrap();
    let mut e = Encoder::new();
    v.encode(&mut e).unwrap();
    assert_eq!(bytes, e.finish().unwrap());

    let back: (i32, String, Vec<i32>) = binary::from_slice(&bytes).unwrap();
    assert_eq!(back, v);

    // The raw form is observable: a lone string has no header byte, and a
    // fixed-length container is just its elements back to back.
    assert_eq!(
        binary::to_vec(&"hello".to_string()).unwrap(),
        b"hello".to_vec()
    );
    assert_eq!(
        binary::from_slice::<String>(&b"hello"[..]).unwrap(),
        "hello"
    );
    assert_eq!(
        binary::to_vec(&vec![1i32, 2]).unwrap(),
        vec![1, 0, 0, 0, 2, 0, 0, 0]
    );

    // Leftover bytes are rejected: a fixed-size value leaves a visible tail.
    // (Variable-length raw decoders consume the whole slice by definition.)
    let mut extra = binary::to_vec(&42i32).unwrap();
    extra.push(0);
    let err = binary::from_slice::<i32>(&extra).unwrap_err();
    assert!(matches!(err, Error::Message(_)), "{err:?}");
}

#[test]
fn test_debug_prints_state_and_bytes() {
    let mut e = Encoder::new();
    e.write(&[0xde, 0xad]).unwrap();
    let text = format!("{e:?}");
    assert!(text.contains("backend: \"owned\""), "{text}");
    assert!(text.contains("len: 2"), "{text}");
    assert!(text.contains("dead"), "{text}");

    let d = Decoder::new(&[0x01, 0x02, 0x03]);
    let text = format!("{d:?}");
    assert!(text.contains("remaining: 3"), "{text}");
    assert!(text.contains("010203"), "{text}");
}

/// Four bytes on the wire, no bulk override — i.e. exactly what a type from
/// another crate looks like — so its container stays on the per-element arm.
/// Lets the tests compare both arms on byte-identical payloads.
#[derive(Debug, PartialEq)]
struct ScopedI32(i32);

impl Layout for ScopedI32 {
    const LEN: usize = 4;
}

impl Encode for ScopedI32 {
    fn encode(&self, e: &mut Encoder<'_>) -> binary::Result<()> {
        e.write(&self.0.to_le_bytes())
    }
}

impl<'de> Decode<'de> for ScopedI32 {
    fn decode(d: &mut Decoder<'de>) -> binary::Result<Self> {
        Ok(ScopedI32(i32::decode(d)?))
    }
}

#[test]
fn test_vec_decode_bulk_arm_matches_per_element_arm() {
    let pod = vec![1i32, -2, 0x0102_0304, i32::MIN, i32::MAX];
    let scoped: Vec<ScopedI32> = pod.iter().copied().map(ScopedI32).collect();
    let bytes = auto_bytes(&pod);

    // `i32` only has a bulk arm where native order == wire order; on s390x
    // (big endian) both types below take the loop. Either way the two must
    // produce identical bytes and identical values.
    assert_eq!(auto_bytes(&scoped), bytes, "both arms share one wire");

    assert_eq!(from_auto::<Vec<i32>>(&bytes), pod);
    assert_eq!(from_auto::<Vec<ScopedI32>>(&bytes), scoped);
}

#[test]
fn test_bool_and_char_validate_on_the_way_in() {
    // Neither type overrides the bulk hooks (`primitives.rs`), so a container
    // of them always decodes value by value.

    // `02` / `ff` are not valid `bool` bit patterns. A bulk copy would adopt
    // them as values (UB); the loop keeps C# / C++'s lenient `b != 0` rule.
    let wire = [0x03u8, 0x02, 0x00, 0xff];
    let got = from_auto::<Vec<bool>>(&wire);
    assert_eq!(got, vec![true, false, true]);
    assert_eq!(auto_bytes(&got), [0x03, 1, 0, 1]); // re-encodes canonically

    // `char` validates its scalar range on the way in; a memcpy would not.
    let bad = with_header(&[0xff, 0xff, 0xff, 0xff]);
    let mut d = Decoder::new(&bad);
    assert!(<Vec<char>>::decode_auto(&mut d).is_err());
}

#[test]
fn test_vec_of_fixed_arrays_round_trips() {
    // `[i32; N]` forwards its whole slice to `i32::encode_slice`, so the array
    // layer adds no per-element overhead of its own; decoding an array is per
    // element and must agree on any target.
    let v = vec![[1i32, -2], [3, 4], [i32::MIN, i32::MAX]];
    assert_eq!(round_trip(&v), v);
}

/// Big-endian contract, checked for real under `s390x` + qemu-user
/// (`scripts/test-multiarch.sh s390x`). A 1-byte scalar has no byte order, so both bulk
/// arms stay; a wider scalar only keeps them where native order equals the
/// little-endian wire order, and falls back to the loop otherwise. Mirrors C++
/// `ContainerConverter::Copyable` (`sizeof(E) == 1 || native == little`).
///
/// The fallback is now a `cfg!` inside the scalar impls rather than a public
/// constant, so it is asserted through wire bytes and round-trips.
#[test]
fn test_wire_order_is_endian_independent() {
    // Wire order is fixed regardless of host byte order.
    assert_eq!(auto_bytes(&0x0102_0304i32), [0x04, 0x03, 0x02, 0x01]);
    assert_eq!(auto_bytes(&0x0102u16), [0x02, 0x01]);

    // 1-byte scalars keep the bulk arms on every target: every bit pattern is
    // a valid `u8`, unlike `bool` (checked in the test above).
    let bytes = [0x03u8, 0x02, 0x00, 0xff];
    assert_eq!(from_auto::<Vec<u8>>(&bytes), vec![2, 0, 255]);
    assert_eq!(auto_bytes(&vec![2u8, 0, 255]), bytes);
}

/// On the owned backend `Vec::len` is a committed watermark, not the write
/// position (DESIGN §19.15.13): the first bytes are zero-filled at
/// construction so that one encoded value never grows the buffer. Nothing
/// observable may depend on it — `as_bytes` and `finish` both slice to `pos`,
/// a payload larger than the watermark still works, and the watermark never
/// exceeds the capacity the caller asked for.
#[test]
fn test_owned_encoder_commits_ahead_without_changing_output() {
    let value = (1i32, "csharp".to_string(), vec![1i32, 2, 3]);
    let expected = auto_bytes(&value);

    let mut e = Encoder::new();
    value.encode_auto(&mut e).expect("infallible");
    assert_eq!(e.len(), expected.len());
    assert_eq!(e.as_bytes(), &expected[..]);
    assert_eq!(e.finish().expect("owned"), expected);

    // Past the watermark the backend falls back to exact growth.
    let long = "x".repeat(300);
    let mut e = Encoder::new();
    long.encode_auto(&mut e).expect("infallible");
    assert_eq!(e.as_bytes(), auto_bytes(&long).as_slice());

    // A capacity below the watermark commits only what was asked for, and
    // growing past it stays the plain `Vec` behavior (an owned encoder never
    // reports `BufferFull`).
    let mut e = Encoder::with_capacity(3);
    e.write(&[1, 2, 3]).expect("fits");
    e.write(&[4]).expect("grows");
    assert_eq!(e.as_bytes(), [1, 2, 3, 4]);
    assert_eq!(e.finish().expect("owned"), [1, 2, 3, 4]);
}

#[test]
fn test_bulk_write_backend_parity() {
    // Payloads at or above `BULK_WRITE_MIN` take a different path on the owned
    // backend (`extend_from_slice` into spare capacity) than the `Vec::resize`
    // + copy shape every smaller write still uses. The wire must not know
    // which backend wrote it, at any length around the threshold, at a
    // non-zero start offset (part of the payload lands in the already-committed
    // region, the rest is appended), and through the wide length prefix.
    for len in [255usize, 256, 257, 4096] {
        let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();

        let mut e = Encoder::new();
        payload.encode_auto(&mut e).expect("infallible");
        let owned = e.finish().expect("owned");

        let mut buf = vec![0u8; owned.len()];
        let mut e = Encoder::on_slice(&mut buf);
        payload.encode_auto(&mut e).expect("fits");
        assert_eq!(e.as_bytes(), &owned[..], "len {len}: backend parity");

        let mut e = Encoder::new();
        e.write(&[0xAA; 3]).expect("fits");
        payload.encode_auto(&mut e).expect("infallible");
        let bytes = e.finish().expect("owned");
        assert_eq!(&bytes[..3], &[0xAA; 3], "len {len}: prefix kept");
        assert_eq!(&bytes[3..], &owned[..], "len {len}: offset parity");

        let mut d = Decoder::new(&owned);
        let round: Vec<u8> = Decode::decode_auto(&mut d).expect("prefix + payload");
        assert_eq!(round, payload, "len {len}: round trip");
    }
}
