//! Free functions over raw slices and allocators — the compact integer family,
//! plus the two prefixed-payload writers.
//!
//! There is no decoder type. A cursor is `&mut &[u8]`: every function consumes
//! from the front, advances the slice **only on success**, and borrows the
//! pieces it returns from the underlying buffer (not from the cursor), so
//! zero-copy decoding keeps working. A short read is always
//! [`Error::NotEnoughBytes`].
//!
//! The compact integer has four entries, mirroring the C# and C++ overloads:
//! [`encode`] / [`decode`] take an allocator / a cursor, while
//! [`encode_slice`] / [`decode_slice`] take a plain slice and report the
//! byte count instead of advancing anything — C# `Converter.Encode(Span<byte>,
//! int, out int bytesWritten)` / `Decode(ReadOnlySpan<byte>, out int bytesRead)`,
//! C++ `binary::Encode(std::span<std::byte>, size_t, size_t&)` /
//! `Decode(std::span<const std::byte>, size_t&)`. Rust has no overloading, so the
//! slice forms carry a suffix instead of being overloads of `encode` / `decode`.
//! The trait's internal bulk hooks are named `encode_bulk` / `decode_bulk`
//! (`#[doc(hidden)]` on [`Encode`] / [`Decode`]), which leaves the `_slice`
//! suffix to the free functions alone.
//!
//! [`Encode`]: crate::Encode
//! [`Decode`]: crate::Decode

use crate::allocator::Allocator;
use crate::error::{Error, Result};
use crate::length::{decode_prefix, encode_prefix, encode_prefix_wide, prefix_header_len};

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

/// Write a *compact integer* into a caller-owned slice. Returns the number of
/// bytes written (1 or 4).
///
/// The slice form of [`encode`], and the direct counterpart of C#
/// `Converter.Encode(Span<byte>, int, out int bytesWritten)` and C++
/// `binary::Encode(std::span<std::byte>, size_t, size_t&)`. `buf` must hold the
/// header: an empty slice is [`Error::BufferFull`], and so is a 3-byte slice
/// when `v > 127`. Reserving 4 bytes always covers it — the returned count says
/// how many were actually used, so a caller sizes the payload after the header.
/// Nothing is written on failure.
#[inline]
pub fn encode_slice(buf: &mut [u8], v: u32) -> Result<usize> {
    encode_prefix(buf, v as usize)
}

/// Read a *compact integer* from the front of `buf` without advancing anything:
/// returns `(value, bytes_read)`.
///
/// The slice form of [`decode`], and the direct counterpart of C#
/// `Converter.Decode(ReadOnlySpan<byte>, out int bytesRead)` and C++
/// `binary::Decode(std::span<const std::byte>, size_t&)`. Callers that want a
/// cursor use [`decode`] instead.
#[inline]
pub fn decode_slice(buf: &[u8]) -> Result<(u32, usize)> {
    let mut pos = 0usize;
    let n = decode_prefix(buf, &mut pos)?;
    Ok((n as u32, pos))
}

/// Read a *compact integer* from the front of `buf`: 1 byte when the value is
/// `<= 127`, otherwise 4 bytes big-endian with bit 31 set. Advances `buf` past
/// it.
#[inline]
pub fn decode(buf: &mut &[u8]) -> Result<u32> {
    let src = *buf;
    let (v, read) = decode_slice(src)?;
    *buf = &src[read..];
    Ok(v)
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
    let (len, start) = decode_slice(src)?;
    let end = start + len as usize;
    if end > src.len() {
        return Err(Error::NotEnoughBytes);
    }
    *buf = &src[end..];
    Ok(&src[start..end])
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
