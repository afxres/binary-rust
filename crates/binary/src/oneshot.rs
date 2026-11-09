//! One-shot encode / decode: allocate, write the **raw** payload, stop.
//!
//! Raw rather than `auto`: a top-level value owns the whole buffer, so a length
//! prefix would be redundant *and* would not match the other two
//! implementations, whose top-level entry points are raw as well. For the `auto`
//! form (prefix when `LEN == 0`, tuple flattening) use [`Encode::encode_auto`].
//!
//! What they deliberately do **not** hide:
//!
//! - **No reuse.** Every call allocates. When that matters, drive [`Encoder`]
//!   yourself (or `Encoder::on_slice`) — see DESIGN §12.
//! - **No trailing data.** [`from_slice`] decodes exactly one value and rejects
//!   leftover bytes; a caller reading one value out of a larger buffer should
//!   use [`Decoder`] directly, where `remaining()` is visible. Variable-length
//!   raw decoders consume the whole slice by definition, so a tail is only
//!   observable for fixed-length types.

use alloc::vec::Vec;

use crate::codec::{Decode, Encode};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::Result;

/// Encode `value` into a new `Vec<u8>` in **raw** position (no length prefix).
///
/// The whole buffer is the payload. [`from_slice`] reads this raw form back.
#[inline]
pub fn to_vec<T: Encode + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut e = Encoder::new();
    value.encode(&mut e)?;
    e.finish()
}

/// Decode one **raw** value from `bytes`, which must be fully consumed.
///
/// For a value stored in `auto` position (with a length prefix) use
/// [`Decode::decode_auto`] instead.
#[inline]
pub fn from_slice<'a, T: Decode<'a>>(bytes: &'a [u8]) -> Result<T> {
    let mut d = Decoder::new(bytes);
    let value = T::decode(&mut d)?;
    if d.remaining() != 0 {
        return Err(crate::error::Error::message_static(
            "trailing bytes after value",
        ));
    }
    Ok(value)
}
