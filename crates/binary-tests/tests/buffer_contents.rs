//! Converters own their bytes.
//!
//! C# `Allocator` keeps `bounds` (capacity) and `offset` (write position) apart
//! and never clears what it reserves; C++ `Allocator::Resize` is `malloc` plus a
//! copy of the written prefix. Neither hands a converter a byte another
//! converter reserved, and neither needs a zeroed buffer: the converter that
//! carves a range writes every byte of it.
//!
//! Rust cannot express that directly — `&mut [u8]` may not point at
//! uninitialised memory — so the owned backend commits a bounded watermark at
//! construction (DESIGN 20.15.13), and the append-only alternative was measured
//! and rejected (DESIGN 20.15.18). What keeps the watermark a bookkeeping detail
//! instead of a semantic one is this test: the same value must encode to the
//! same bytes whether the backend is owned, a zeroed slice, or a slice prefilled
//! with 0x5A / 0xFF.
//!
//! A converter that reserves twice the room it fills, or that leaves a
//! placeholder behind, is invisible on the owned path — the watermark is zeros,
//! so the leak reads back as a legitimate zero byte.

mod common;

use binary::Encoder;
use common::cases::{case_names, encode_case, encode_case_into};

/// Slack for the `write_prefixed` tail margin. The compaction reproduces the
/// owned 1-byte header only when there are `(-length) & 7` spare bytes past the
/// payload; without them the 4-byte header is kept (valid wire, and what C#/C++
/// do with a tight buffer — `test_write_prefixed_borrowed_tail_margin`). Worst
/// case for the innermost prefix is 3 extra header bytes plus 7 pad. Those bytes
/// are never written, so their value does not matter — which is the point.
const SLACK: usize = 16;

#[test]
fn test_wire_is_independent_of_buffer_contents() {
    for name in case_names() {
        let expected = encode_case(name);

        for fill in [0x00u8, 0x5a, 0xff] {
            let mut buf = vec![fill; expected.len() + SLACK];
            let mut e = Encoder::on_slice(&mut buf);
            encode_case_into(name, &mut e);
            assert_eq!(
                e.as_bytes(),
                &expected[..],
                "{name}: buffer prefilled with {fill:#04x} changed the wire"
            );
            assert_eq!(e.len(), expected.len(), "{name}: length with {fill:#04x}");
        }

        // Owned with no initial capacity: growth copies the written prefix and
        // leaves the rest alone, so a converter that relied on fresh zeroes past
        // the watermark would diverge on the first growth.
        let mut e = Encoder::with_capacity(0);
        encode_case_into(name, &mut e);
        assert_eq!(e.finish().unwrap(), expected, "{name}: growing from empty");
    }
}
