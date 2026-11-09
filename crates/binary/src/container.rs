use crate::codec::{Decode, Encode, Layout};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::{Error, Result};

use alloc::string::String;
use alloc::vec::Vec;
use core::mem;

// ---- String / &str ----

impl Layout for str {
    const LEN: usize = 0;
}

impl Encode for str {
    #[inline]
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write(self.as_bytes())
    }

    #[inline]
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        // `str` is UTF-8 by construction, so `len()` is the exact wire byte
        // count: header width is known before the payload moves, and one
        // reservation covers both.
        e.write_prefixed_bytes(self.as_bytes())
    }
}

impl Layout for String {
    const LEN: usize = 0;
}

impl Encode for String {
    #[inline]
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write(self.as_bytes())
    }

    #[inline]
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write_prefixed_bytes(self.as_bytes())
    }
}

impl<'de> Decode<'de> for String {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        // `auto` has already consumed the outer prefix and constrained the
        // decoder to the exact payload region, so read all remaining bytes.
        let bytes = d.take(d.remaining())?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Error::message_static("invalid UTF-8"))
    }
}

/// Zero-copy decode of `&str` from the wire.
/// Layout/Encode for `&str` mirror `str` so the reference type can be used
/// as both an encoding input and a decoding output.
impl Layout for &str {
    const LEN: usize = 0;
}

impl Encode for &str {
    #[inline]
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write(self.as_bytes())
    }

    #[inline]
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write_prefixed_bytes(self.as_bytes())
    }
}

impl<'de> Decode<'de> for &'de str {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        let bytes = d.take(d.remaining())?;
        core::str::from_utf8(bytes).map_err(|_| Error::message_static("invalid UTF-8"))
    }
}

// ---- &[u8] ----

impl Layout for [u8] {
    const LEN: usize = 0;
}

impl Encode for [u8] {
    #[inline]
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write(self)
    }

    #[inline]
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write_prefixed_bytes(self)
    }
}

/// Layout/Encode for `&[u8]` mirror `[u8]`.
impl Layout for &[u8] {
    const LEN: usize = 0;
}

impl Encode for &[u8] {
    #[inline]
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write(self)
    }

    #[inline]
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        e.write_prefixed_bytes(self)
    }
}

impl<'de> Decode<'de> for &'de [u8] {
    #[inline]
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        d.take(d.remaining())
    }
}

// ---- Vec<T> ----

impl<T> Layout for Vec<T> {
    const LEN: usize = 0;
}

impl<T: Encode> Encode for Vec<T> {
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        match T::LEN {
            // O(1): fixed-size elements need no per-element prefix.
            n if n > 0 => {
                e.write_prefixed_bounded(self.len().saturating_mul(n), |e| self.encode(e))
            }
            // No cheap estimate available — `write_prefixed` fallback, never recurse.
            _ => e.write_prefixed(|e| self.encode(e)),
        }
    }

    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        // Delegated to the element: fixed-width scalars override `encode_slice`
        // with one bulk write, everything else gets the loop (`codec.rs`).
        T::encode_slice(self, e)
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for Vec<T> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        // `auto` already consumed the outer prefix and constrained `d` to the
        // exact payload region, so decode until it is exhausted.
        match T::LEN {
            elem_len if elem_len > 0 => {
                let remaining = d.remaining();
                if !remaining.is_multiple_of(elem_len) {
                    return Err(Error::MisalignedElements);
                }
                let count = remaining / elem_len;
                if count == 0 {
                    return Ok(Vec::new());
                }
                // Same delegation on the way in; a type that does not override
                // `decode_slice` runs `T::decode` per value.
                let vec = T::decode_slice(d, count)?;
                debug_assert_eq!(vec.len(), count);
                debug_assert_eq!(d.remaining(), 0, "element hook left bytes behind");
                Ok(vec)
            }
            _ => {
                let mut vec = Vec::new();
                while d.remaining() > 0 {
                    vec.push(T::decode_auto(d)?);
                }
                Ok(vec)
            }
        }
    }
}

// ---- Arrays ----

impl<T: Layout, const N: usize> Layout for [T; N] {
    const LEN: usize = array_len::<T, N>();
}

/// `N * T::LEN`, with `0` (variable) propagating and `[T; 0]` staying `0`.
///
/// A zero-length array does have a fixed 0-byte size, but "fixed at zero bytes"
/// is indistinguishable from "variable" in this encoding, and treating it as
/// fixed would make it encode to nothing at all.
const fn array_len<T: Layout, const N: usize>() -> usize {
    let mut result = 0;
    let mut i = 0;
    while i < N {
        let elem = T::LEN;
        if elem == 0 {
            return 0;
        }
        result += elem;
        i += 1;
    }
    result
}

impl<T: Encode, const N: usize> Encode for [T; N] {
    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        match T::LEN {
            n if n > 0 => e.write_prefixed_bounded(N.saturating_mul(n), |e| self.encode(e)),
            _ => e.write_prefixed(|e| self.encode(e)),
        }
    }

    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        T::encode_slice(self, e)
    }
}

impl<'de, T: Decode<'de>, const N: usize> Decode<'de> for [T; N] {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        let mut arr: mem::MaybeUninit<[T; N]> = mem::MaybeUninit::uninit();
        let ptr = arr.as_mut_ptr() as *mut T;
        let mut initialized = 0;
        let result = (|| {
            for i in 0..N {
                unsafe { ptr.add(i).write(T::decode_auto(d)?) };
                initialized += 1;
            }
            Ok(())
        })();
        if result.is_ok() {
            unsafe { Ok(arr.assume_init()) }
        } else {
            for i in 0..initialized {
                unsafe { ptr.add(i).drop_in_place() };
            }
            result?;
            unreachable!()
        }
    }
}

// ---- () ----

// A zero-sized type is variable-length: `LEN == 0` is the marker for "no fixed
// size". Were it a fixed 0-byte size, `Vec<()>` would encode to nothing, the
// element count could not be recovered and the decode loop would not advance.
impl Layout for () {
    const LEN: usize = 0;
}

impl Encode for () {
    #[inline]
    fn encode(&self, _e: &mut Encoder<'_>) -> Result<()> {
        Ok(())
    }
    // Default `encode_with_prefix` takes the `write_prefixed` path and writes a
    // single `0x00`, so containers can still recover the element count.
}

impl<'de> Decode<'de> for () {
    #[inline]
    fn decode(_d: &mut Decoder<'de>) -> Result<Self> {
        Ok(())
    }
}

// ---- Tuples ----

// Tuples are flattened on the wire:
// - `encode` (raw): all but the last member use `auto`; the last is raw, its
//   length implied by the remaining bytes.
// - `encode_auto`: **every** member uses `auto`, and the tuple adds no header of
//   its own — so `((a,b),c)` and `(a,b,c)` produce identical bytes.
// - `encode_with_prefix` keeps the default (prefix + `encode`).
// Decode mirrors this with `decode` / `decode_auto`.
//
// Generated for 1..=12 members: twelve is where `std` stops implementing
// `Debug` / `Clone` / `PartialEq` for tuples. Wider shapes are written by hand
// as named structs with `#[binary(tuple)]`, which the derive expands to any
// arity.

/// `Σ` member lengths, with `0` (variable-length) propagating.
///
/// A loop over the lengths rather than a `match (A::LEN, B::LEN, ...)` pattern:
/// the hand-written version needed 2^N arms to express "any member is zero".
const fn tuple_len(lens: &[usize]) -> usize {
    let mut sum = 0usize;
    let mut i = 0;
    while i < lens.len() {
        match lens[i] {
            0 => return 0,
            n => sum += n,
        }
        i += 1;
    }
    sum
}

/// `encode` body: every member but the last through `auto`, the last raw.
///
/// The inner `{{ }}` is load-bearing: braces after `=>` only delimit the
/// transcriber, they are not emitted, so without them the recursive arm would
/// expand to `stmt; expr` and fail in expression position.
macro_rules! tuple_encode_raw {
    ($e:ident, $this:ident; $idx:tt $name:ident) => {
        $this.$idx.encode($e)
    };
    ($e:ident, $this:ident; $idx:tt $name:ident, $($rest_idx:tt $rest:ident),+) => {{
        $this.$idx.encode_auto($e)?;
        tuple_encode_raw!($e, $this; $($rest_idx $rest),+)
    }};
}

/// `decode` body: the same split, accumulated into one tuple expression.
///
/// `$val` is an accumulator because `macro_rules!` cannot prepend a term to a
/// recursive result; the prefix is collected left to right and emitted as a
/// literal, whose elements Rust evaluates in order. The trailing comma keeps the
/// 1-tuple a tuple instead of a parenthesised value.
macro_rules! tuple_decode_raw {
    ([$($val:expr),*] ; $d:ident ; $idx:tt $name:ident) => {
        ($($val,)* $name::decode($d)?,)
    };
    ([$($val:expr),*] ; $d:ident ; $idx:tt $name:ident, $($rest_idx:tt $rest:ident),+) => {
        tuple_decode_raw!([$($val,)* $name::decode_auto($d)?] ; $d ; $($rest_idx $rest),+)
    };
}

/// Generate `Layout` / `Encode` / `Decode` for one arity.
///
/// Each member arrives as an `index ident` pair (`0 A`, `1 B`, …) because
/// `macro_rules!` cannot compute `self.0` from a counter: the field index has to
/// be a token that already exists, so the invocations below are literal.
macro_rules! impl_tuple {
    ($($idx:tt $name:ident),+) => {
        impl<$($name: Layout),+> Layout for ($($name,)+) {
            const LEN: usize = tuple_len(&[$($name::LEN),+]);
        }

        impl<$($name: Encode),+> Encode for ($($name,)+) {
            fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
                tuple_encode_raw!(e, self; $($idx $name),+)
            }

            fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
                $( self.$idx.encode_auto(e)?; )+
                Ok(())
            }
        }

        impl<'de, $($name: Decode<'de>),+> Decode<'de> for ($($name,)+) {
            fn decode(d: &mut Decoder<'de>) -> Result<Self> {
                Ok(tuple_decode_raw!([] ; d ; $($idx $name),+))
            }

            fn decode_auto(d: &mut Decoder<'de>) -> Result<Self> {
                Ok(($( $name::decode_auto(d)?, )+))
            }
        }
    };
}

impl_tuple!(0 A);
impl_tuple!(0 A, 1 B);
impl_tuple!(0 A, 1 B, 2 C);
impl_tuple!(0 A, 1 B, 2 C, 3 D);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I, 9 J);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I, 9 J, 10 K);
impl_tuple!(0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H, 8 I, 9 J, 10 K, 11 L);
