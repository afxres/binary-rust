//! The bulk (memcpy) arms — the only place this crate reinterprets a slice of
//! values as payload bytes.
//!
//! Both are reached only from the `encode_slice` / `decode_slice` overrides in
//! `primitives.rs`, behind a condition that is a compile-time constant there
//! (`size_of::<T>() == 1`, which has no byte order to get wrong, or a
//! little-endian target). The overrides are `#[doc(hidden)]` trait methods, so
//! nothing outside this crate reaches them: a type that does not override them
//! stays on the per-element loop. See `DESIGN.md` §19.16.

use crate::codec::{Decode, Encode};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::{Error, Result};

use alloc::vec::Vec;
use core::mem;

/// Write `items` as one contiguous run of `items.len() * T::LEN` payload bytes.
///
/// # Safety
///
/// `T` must satisfy all of:
///
/// 1. `size_of::<T>() == T::LEN`;
/// 2. no padding — every byte of the slice is initialized and meaningful;
/// 3. the raw encoding of every value is exactly its bytes in the target's
///    native order (so a multi-byte type requires a little-endian target).
#[inline]
pub(crate) unsafe fn write_bulk<T: Encode>(items: &[T], e: &mut Encoder<'_>) -> Result<()> {
    debug_assert_eq!(
        mem::size_of::<T>(),
        T::LEN,
        "bulk write of a type whose size is not its wire length"
    );
    let byte_len = items
        .len()
        .checked_mul(T::LEN)
        .ok_or(Error::LengthTooLarge)?;
    // SAFETY: the caller promises no padding and a matching `size_of`, so the
    // slice is one run of initialized payload bytes; `byte_len` was checked
    // against overflow just above.
    let bytes = unsafe { core::slice::from_raw_parts(items.as_ptr().cast::<u8>(), byte_len) };
    e.write(bytes)
}

/// Adopt `count` values out of `count * T::LEN` wire bytes without running
/// [`Decode`].
///
/// # Safety
///
/// Everything [`write_bulk`] requires, plus:
///
/// 4. **every** bit pattern is a valid value of `T` — the bytes are untrusted
///    input;
/// 5. `T` owns nothing: no `Drop`, no references, no lifetime tied to `'de`.
#[inline]
pub(crate) unsafe fn read_bulk<'de, T: Decode<'de>>(
    d: &mut Decoder<'de>,
    count: usize,
) -> Result<Vec<T>> {
    debug_assert_eq!(
        mem::size_of::<T>(),
        T::LEN,
        "bulk read of a type whose size is not its wire length"
    );
    debug_assert!(!mem::needs_drop::<T>(), "bulk read of an owning type");
    let byte_len = count.checked_mul(T::LEN).ok_or(Error::LengthTooLarge)?;
    let src = d.take(byte_len)?;
    let mut values: Vec<T> = Vec::with_capacity(count);
    // SAFETY: the caller promises no padding, matching `size_of`, native byte
    // order equal to the wire order, every bit pattern valid, and no ownership.
    // `src` is exactly `count * T::LEN` bytes and the allocation holds `count`
    // values, so the copy fills it completely.
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), values.as_mut_ptr().cast::<u8>(), byte_len);
        values.set_len(count);
    }
    Ok(values)
}
