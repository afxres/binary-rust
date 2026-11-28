//! Free functions over raw slices — the decode side, plus the two free-form
//! writers (the compact integer and the prefixed payload).
//!
//! There is no decoder type. A cursor is `&mut &[u8]`: every function consumes
//! from the front, advances the slice **only on success**, and borrows the
//! pieces it returns from the underlying buffer (not from the cursor), so
//! zero-copy decoding keeps working. A short read is always
//! [`Error::NotEnoughBytes`].
//!
//! The compact-int pair ([`encode`] / [`decode`]) is named after the C# and C++
//! counterparts (`ConverterExtensions.Encode` / `Decode`,
//! `binary::Encode` / `binary::Decode`), which are free functions too.
//!
//! The span variants of those two — C# `Converter.Encode(Span<byte>, int,
//! out int bytesWritten)` and `Decode(ReadOnlySpan<byte>, out int bytesRead)` —
//! get no counterpart here, on purpose: both compose from what already exists.
//! Writing into a caller-owned slice is `length::encode_prefix(buf, len)`, or
//! `Allocator::on_slice(buf)` + [`encode`] + `Allocator::len`; reading without
//! advancing is a copy of the cursor passed to [`decode`], with
//! `bytes_read = buf.len() - probe.len()`.

use crate::allocator::Allocator;
use crate::error::{Error, Result};
use crate::length::{decode_prefix, encode_prefix_wide, prefix_header_len};

/// Write a *compact integer* into the allocator: 1 byte when `v <= 127`,
/// otherwise 4 bytes big-endian with bit 31 set.
///
/// Wire-identical to a length prefix by design — the same bytes a prefix of
/// that length would produce. Used for union / variant tags, so a tag stays
/// cheap for the first 128 variants and still covers `i32::MAX` variants.
///
/// Directly comparable with [`crate::Encode`] for `usize`: a `usize` writes
/// 8 fixed bytes, a compact integer writes 1 or 4.
#[inline]
pub fn encode(a: &mut Allocator<'_>, v: u32) -> Result<()> {
    // Release-safe guard: `v | 0x8000_0000` keeps bit 31 as *data* for values
    // above `i32::MAX`, and the decoder masks that bit away, so the value
    // would change silently. Nothing is written before the check.
    if v as usize > crate::length::MAX_LENGTH {
        return Err(Error::LengthTooLarge);
    }
    let h = prefix_header_len(v as usize);
    a.ensure_capacity(h)?;
    let pos = a.len();
    {
        let slice = a.as_mut_slice();
        if h == 1 {
            slice[pos] = v as u8;
        } else {
            encode_prefix_wide(&mut slice[pos..pos + 4], v as usize)?;
        }
    }
    a.advance(h);
    Ok(())
}

/// Read a *compact integer* from the front of `buf`: 1 byte when the value is
/// `<= 127`, otherwise 4 bytes big-endian with bit 31 set. Advances `buf` past
/// it.
#[inline]
pub fn decode(buf: &mut &[u8]) -> Result<u32> {
    let src = *buf;
    let mut pos = 0usize;
    let n = decode_prefix(src, &mut pos)?;
    *buf = &src[pos..];
    Ok(n as u32)
}

/// Write a length prefix followed by the payload bytes in one step: the write
/// half of [`decode_with_length_prefix`], and the counterpart of the C#
/// `Converter.EncodeWithLengthPrefix`. Reserves prefix and payload with a
/// single capacity check; a length beyond the wire's 31-bit field is
/// [`Error::LengthTooLarge`] before anything is written.
#[inline]
pub fn encode_with_length_prefix(a: &mut Allocator<'_>, payload: &[u8]) -> Result<()> {
    a.append_prefixed_bytes(payload)
}

/// Read a length prefix and return the payload slice, advancing `buf` past
/// both the prefix and the payload.
///
/// The read half of a value stored in `auto` position. Compare the C# and C++
/// `DecodeWithLengthPrefix`.
#[inline]
pub fn decode_with_length_prefix<'a>(buf: &mut &'a [u8]) -> Result<&'a [u8]> {
    let src = *buf;
    let mut pos = 0usize;
    let len = decode_prefix(src, &mut pos)?;
    let start = pos;
    if start + len > src.len() {
        return Err(Error::NotEnoughBytes);
    }
    let payload = &src[start..start + len];
    *buf = &src[start + len..];
    Ok(payload)
}

/// Take `n` bytes from the front of `buf`, returning a slice borrowed from the
/// underlying buffer. Internal primitive behind the fixed-size `Decode` paths.
#[inline]
pub(crate) fn take<'a>(buf: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if n > buf.len() {
        return Err(Error::NotEnoughBytes);
    }
    let (head, rest) = buf.split_at(n);
    *buf = rest;
    Ok(head)
}

/// Take a single byte from the front of `buf`. Internal primitive.
#[inline]
pub(crate) fn take_byte(buf: &mut &[u8]) -> Result<u8> {
    let (&b, rest) = buf.split_first().ok_or(Error::NotEnoughBytes)?;
    *buf = rest;
    Ok(b)
}
