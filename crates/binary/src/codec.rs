use crate::allocator::Allocator;
use crate::converter::{decode_with_length_prefix, take};
use crate::error::Result;

use alloc::vec::Vec;

/// Type-level layout information, known at compile time.
pub trait Layout {
    /// Bytes a raw encoding of **any** value of this type occupies, or `0` when
    /// the type is variable-length.
    ///
    /// `0` **is** the variable-length marker, so there is no `Some(0)` state to
    /// guard against and `[T; 0]` degrades to `0` on its own. Zero-sized types
    /// such as `()` are variable-length for the same reason: a container cannot
    /// recover an element count from zero bytes. See `DESIGN.md` §5.
    const LEN: usize = 0;
}

/// Encode a value to raw bytes (no length prefix).
pub trait Encode: Layout {
    /// Write raw payload bytes. Implementations must never write a length
    /// prefix themselves — prefixes belong to [`Encode::encode_auto`] and
    /// [`Encode::encode_with_prefix`].
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()>;

    /// Write an unconditional `prefix + payload`.
    ///
    /// The default picks the header width from `LEN` (O(1) for fixed-size
    /// types) and falls back to the `append_with_length_prefix` path otherwise. Converters
    /// that know their length in O(1) — `String`, byte slices, `len() * LEN` —
    /// override it. `encode_named_member` uses this for every member value.
    fn encode_with_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        match Self::LEN {
            0 => e.append_with_length_prefix(|e| self.encode(e)),
            n => e.append_with_length_prefix_bounded(n, |e| self.encode(e)),
        }
    }

    /// Write the value in `auto` position: fixed-size types are written raw,
    /// variable-size types get a length prefix so the enclosing structure can
    /// find their boundary.
    ///
    /// **Tuples override this** to flatten: every member goes through `auto`,
    /// including the last one, and the tuple contributes no header.
    fn encode_auto(&self, e: &mut Allocator<'_>) -> Result<()> {
        match Self::LEN {
            0 => self.encode_with_prefix(e),
            _ => self.encode(e),
        }
    }

    /// Write `items` as one raw payload, one element at a time by default.
    ///
    /// **Internal extension point, not part of the documented surface.**
    /// `Vec<T>` and `[T; N]` call it so the fixed-width scalars can replace the
    /// loop with a single bulk write while `Vec<T>` stays one generic `impl`.
    ///
    /// The default is the loop, so a type that does not override this — every
    /// type outside this crate — is never on a bulk path; overriding is what
    /// moves the `unsafe` obligation to the impl that wants it (`DESIGN.md`
    /// §19.16). An override must produce exactly the bytes the loop would, and
    /// must not write a length prefix.
    #[doc(hidden)]
    fn encode_bulk(items: &[Self], e: &mut Allocator<'_>) -> Result<()>
    where
        Self: Sized,
    {
        for item in items {
            item.encode_auto(e)?;
        }
        Ok(())
    }
}

/// Decode a value from raw bytes (no length prefix).
pub trait Decode<'de>: Layout + Sized {
    /// Decode from the bytes that belong to this value — the slice model, no
    /// cursor in the signature.
    ///
    /// For `LEN > 0` types the framework slices exactly `LEN` bytes off the
    /// cursor before calling, mirroring C#'s `Decode(in span)` on a converter
    /// with a fixed `Length`; an impl may assume the length and must not read
    /// past it. For `LEN == 0` types the slice is everything the value owns in
    /// this position: all remaining bytes in raw position ([`decode_value`],
    /// tuple tails), the prefix-declared payload in `auto` position.
    ///
    /// A fixed-size impl validates cheaply and reports a short payload as
    /// `NotEnoughBytes`; nothing on a framework path panics.
    ///
    /// [`decode_value`]: crate::decode_value
    fn decode(payload: &'de [u8]) -> Result<Self>;

    /// Decode the value in `auto` position: fixed-size types are sliced by
    /// `LEN`, variable-size types read a length prefix first. Tuples override
    /// this to flatten (all members decoded via `auto`, including the last
    /// one), as do self-delimiting types (`Option<T>`).
    fn decode_auto(buf: &mut &'de [u8]) -> Result<Self> {
        match Self::LEN {
            0 => {
                let payload = decode_with_length_prefix(buf)?;
                Self::decode(payload)
            }
            n => {
                let payload = take(buf, n)?;
                Self::decode(payload)
            }
        }
    }

    /// Read `count` fixed-size values out of a payload of exactly
    /// `count * Self::LEN` bytes.
    ///
    /// **Internal extension point, not part of the documented surface** — the
    /// decode-side counterpart of [`Encode::encode_bulk`] (`DESIGN.md` §19.16).
    /// The caller has already sliced the payload to exactly
    /// `count * Self::LEN` bytes, with `Self::LEN > 0`.
    ///
    /// An override that memcpys wire bytes straight in takes over the
    /// obligation: every bit pattern must be a valid value and the type must
    /// own nothing.
    #[doc(hidden)]
    fn decode_bulk(payload: &'de [u8], count: usize) -> Result<Vec<Self>>
    where
        Self: Sized,
    {
        let mut values = Vec::with_capacity(count);
        let mut rest = payload;
        for _ in 0..count {
            let p = take(&mut rest, Self::LEN)?;
            values.push(Self::decode(p)?);
        }
        Ok(values)
    }
}
