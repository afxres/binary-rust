//! One-shot encode / decode: allocate, write the **raw** payload, stop.
//!
//! Raw rather than `auto`: a top-level value owns the whole buffer, so a length
//! prefix would be redundant *and* would not match the other two
//! implementations, whose top-level entry points are raw as well. For the `auto`
//! form (prefix when `LEN == 0`, tuple flattening) use [`Encode::encode_auto`].
//!
//! What they deliberately do **not** hide:
//!
//! - **No reuse.** Every call allocates. When that matters, drive [`Allocator`]
//!   yourself (or `Allocator::on_slice`) — see DESIGN §12.
//! - **No trailing data for fixed-size types only.** [`decode`] slices exactly
//!   `LEN` bytes and rejects a leftover tail. A variable-length type is handed
//!   the whole slice and decides where its own value ends, so a tail after it
//!   goes unreported; a caller reading one value out of a larger buffer should
//!   slice the remainder themselves.

use alloc::vec::Vec;

use crate::allocator::Allocator;
use crate::codec::{Decode, Encode};
use crate::error::Result;

/// Encode `value` into a new `Vec<u8>` in **raw** position (no length prefix).
///
/// The whole buffer is the payload. [`decode`] reads this raw form back.
#[inline]
pub fn encode<T: Encode + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut a = Allocator::new();
    value.encode(&mut a)?;
    a.finish()
}

/// Decode one **raw** value from `bytes`.
///
/// A `LEN > 0` type requires `bytes` to be exactly `LEN` long: a short slice is
/// [`Error::NotEnoughBytes`], a long one is a trailing-bytes error. A
/// variable-length type is given the whole slice and decides its own end, so a
/// tail after it is not reported.
///
/// For a value stored in `auto` position (with a length prefix) use
/// [`Decode::decode_auto`] instead.
///
/// [`Error::NotEnoughBytes`]: crate::Error::NotEnoughBytes
#[inline]
pub fn decode<'a, T: Decode<'a>>(bytes: &'a [u8]) -> Result<T> {
    // Fixed-length types get exactly LEN bytes, so a tail after them is
    // observable and rejected. A LEN == 0 raw decoder receives the whole slice
    // and owns the question of what its value spans — most consume everything
    // ("String", containers); a self-delimiting one like "Option" stops where
    // its wire form ends, and a tail after it is not this function's business.
    match T::LEN {
        0 => T::decode(bytes),
        n => {
            let mut rest = bytes;
            let payload = crate::converter::take(&mut rest, n)?;
            if !rest.is_empty() {
                return Err(crate::error::Error::message_static(
                    "trailing bytes after value",
                ));
            }
            T::decode(payload)
        }
    }
}
