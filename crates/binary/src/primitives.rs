use crate::allocator::Allocator;
use crate::codec::{Decode, Encode, Layout};
use crate::error::{Error, Result};

use alloc::vec::Vec;
use core::mem;

/// Macro to implement Encode/Decode for fixed-size numeric types.
/// Little-endian on wire.
macro_rules! impl_fixed {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Layout for $ty {
                const LEN: usize = mem::size_of::<$ty>();
            }

            impl Encode for $ty {
                #[inline]
                fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
                    e.append(&self.to_le_bytes())
                }

                // The only bulk arms in the crate, kept here so that no
                // "is this POD" flag has to be exposed (`DESIGN.md` §19.16). A
                // 1-byte scalar has no byte order and accepts every bit
                // pattern; a wider one is byte-identical to the wire only on a
                // little-endian target, so the arm compiles away elsewhere.
                #[inline]
                fn encode_bulk(items: &[Self], e: &mut Allocator<'_>) -> Result<()> {
                    if mem::size_of::<Self>() == 1 || cfg!(target_endian = "little") {
                        // SAFETY: fixed-width scalar — `LEN == size_of`, no
                        // padding, all bit patterns valid, nothing owned, and
                        // native order is the wire order (or there is only one
                        // byte to order).
                        unsafe { crate::pod::write_bulk(items, e) }
                    } else {
                        for item in items {
                            item.encode_auto(e)?;
                        }
                        Ok(())
                    }
                }
            }

            impl<'de> Decode<'de> for $ty {
                #[inline]
                fn decode(payload: &'de [u8]) -> Result<Self> {
                    // The framework slices exactly LEN bytes; a short payload
                    // only reaches here through a hand-rolled call.
                    let bytes = <$ty>::from_le_bytes(
                        payload.try_into().map_err(|_| Error::NotEnoughBytes)?,
                    );
                    Ok(bytes)
                }

                #[inline]
                fn decode_bulk(payload: &'de [u8], count: usize) -> Result<Vec<Self>> {
                    if mem::size_of::<Self>() == 1 || cfg!(target_endian = "little") {
                        // SAFETY: as in `encode_bulk` above; every bit pattern
                        // of a fixed-width scalar is a valid value, and the
                        // type owns nothing.
                        unsafe { crate::pod::read_bulk(payload, count) }
                    } else {
                        let mut values = Vec::with_capacity(count);
                        for p in payload.chunks_exact(mem::size_of::<Self>()) {
                            values.push(Self::decode(p)?);
                        }
                        Ok(values)
                    }
                }
            }
        )*
    };
}

impl_fixed!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64);

/// usize/isize encode as their native-size unsigned counterparts.
/// On 64-bit platforms this is 8 bytes.
impl Layout for usize {
    const LEN: usize = mem::size_of::<usize>();
}

impl Encode for usize {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(&self.to_le_bytes())
    }

    #[inline]
    fn encode_bulk(items: &[Self], e: &mut Allocator<'_>) -> Result<()> {
        // Never 1 byte, so the bulk arm only exists on a little-endian target.
        if cfg!(target_endian = "little") {
            // SAFETY: `LEN == size_of`, no padding, every bit pattern valid,
            // nothing owned, native order is the wire order.
            unsafe { crate::pod::write_bulk(items, e) }
        } else {
            for item in items {
                item.encode_auto(e)?;
            }
            Ok(())
        }
    }
}

impl<'de> Decode<'de> for usize {
    #[inline]
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let bytes = usize::from_le_bytes(payload.try_into().map_err(|_| Error::NotEnoughBytes)?);
        Ok(bytes)
    }

    #[inline]
    fn decode_bulk(payload: &'de [u8], count: usize) -> Result<Vec<Self>> {
        if cfg!(target_endian = "little") {
            // SAFETY: as in `encode_bulk` above.
            unsafe { crate::pod::read_bulk(payload, count) }
        } else {
            let mut values = Vec::with_capacity(count);
            for p in payload.chunks_exact(mem::size_of::<Self>()) {
                values.push(Self::decode(p)?);
            }
            Ok(values)
        }
    }
}

impl Layout for isize {
    const LEN: usize = mem::size_of::<isize>();
}

impl Encode for isize {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(&self.to_le_bytes())
    }

    #[inline]
    fn encode_bulk(items: &[Self], e: &mut Allocator<'_>) -> Result<()> {
        // Never 1 byte, so the bulk arm only exists on a little-endian target.
        if cfg!(target_endian = "little") {
            // SAFETY: `LEN == size_of`, no padding, every bit pattern valid,
            // nothing owned, native order is the wire order.
            unsafe { crate::pod::write_bulk(items, e) }
        } else {
            for item in items {
                item.encode_auto(e)?;
            }
            Ok(())
        }
    }
}

impl<'de> Decode<'de> for isize {
    #[inline]
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let bytes = isize::from_le_bytes(payload.try_into().map_err(|_| Error::NotEnoughBytes)?);
        Ok(bytes)
    }

    #[inline]
    fn decode_bulk(payload: &'de [u8], count: usize) -> Result<Vec<Self>> {
        if cfg!(target_endian = "little") {
            // SAFETY: as in `encode_bulk` above.
            unsafe { crate::pod::read_bulk(payload, count) }
        } else {
            let mut values = Vec::with_capacity(count);
            for p in payload.chunks_exact(mem::size_of::<Self>()) {
                values.push(Self::decode(p)?);
            }
            Ok(values)
        }
    }
}

/// bool encodes as a single byte: 0 = false, 1 = true.
///
/// Decoding is deliberately lenient: **any nonzero byte is `true`**, matching
/// the other two implementations. The asymmetry (`02` → `true` → re-encodes as
/// `01`) is intended: reading is lenient, writing is canonical.
///
/// No bulk override, on purpose. A slice of `bool` would be valid wire, but the
/// two directions share one hook: a bulk read would adopt arbitrary wire bytes
/// as values *and* drop the lenient rule. A thousand flags therefore cost a
/// thousand writes — dense boolean data belongs in a bitset (`DESIGN.md`
/// §19.10).
impl Layout for bool {
    const LEN: usize = 1;
}

impl Encode for bool {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append_byte(if *self { 1 } else { 0 })
    }
}

impl<'de> Decode<'de> for bool {
    #[inline]
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let &b = payload.first().ok_or(Error::NotEnoughBytes)?;
        Ok(b != 0)
    }
}

/// char encodes as a 4-byte little-endian u32 (the Unicode scalar value).
///
/// No bulk override for the same reason as `bool`: only a subset of the `u32`
/// range is a valid scalar, and a bulk read would skip the `char::from_u32`
/// check. Chars are rare enough that the per-element loop costs nothing.
impl Layout for char {
    const LEN: usize = 4;
}

impl Encode for char {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(&(*self as u32).to_le_bytes())
    }
}

impl<'de> Decode<'de> for char {
    #[inline]
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let bytes = u32::from_le_bytes(payload.try_into().map_err(|_| Error::NotEnoughBytes)?);
        char::from_u32(bytes).ok_or(Error::message_static("invalid char value"))
    }
}
