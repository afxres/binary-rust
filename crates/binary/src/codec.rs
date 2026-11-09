use crate::decoder::Decoder;
use crate::encoder::Encoder;
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
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()>;

    /// Write an unconditional `prefix + payload`.
    ///
    /// The default picks the header width from `LEN` (O(1) for fixed-size
    /// types) and falls back to the `write_prefixed` path otherwise. Converters
    /// that know their length in O(1) — `String`, byte slices, `len() * LEN` —
    /// override it. `write_named_member` uses this for every member value.
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        match Self::LEN {
            0 => e.write_prefixed(|e| self.encode(e)),
            n => e.write_prefixed_bounded(n, |e| self.encode(e)),
        }
    }

    /// Write the value in `auto` position: fixed-size types are written raw,
    /// variable-size types get a length prefix so the enclosing structure can
    /// find their boundary.
    ///
    /// **Tuples override this** to flatten: every member goes through `auto`,
    /// including the last one, and the tuple contributes no header.
    fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
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
    fn encode_slice(items: &[Self], e: &mut Encoder<'_>) -> Result<()>
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
    fn decode(d: &mut Decoder<'de>) -> Result<Self>;

    /// Decode the value in `auto` position. Tuples override this to flatten
    /// (all members decoded via `auto`, including the last one).
    fn decode_auto(d: &mut Decoder<'de>) -> Result<Self> {
        match Self::LEN {
            0 => d.with_prefix(|d| Self::decode(d)),
            _ => Self::decode(d),
        }
    }

    /// Read `count` fixed-size values, one element at a time by default.
    ///
    /// **Internal extension point, not part of the documented surface** — the
    /// decode-side counterpart of [`Encode::encode_slice`] (`DESIGN.md` §19.16).
    /// The caller has already checked that the payload is exactly
    /// `count * Self::LEN` bytes, with `Self::LEN > 0`.
    ///
    /// An override that memcpys wire bytes straight in takes over the
    /// obligation: every bit pattern must be a valid value and the type must
    /// own nothing.
    #[doc(hidden)]
    fn decode_slice(d: &mut Decoder<'de>, count: usize) -> Result<Vec<Self>>
    where
        Self: Sized,
    {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(Self::decode(d)?);
        }
        Ok(values)
    }
}
