use crate::allocator::Allocator;
use crate::codec::{Decode, Encode, Layout};
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
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(self.as_bytes())
    }

    #[inline]
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        // `str` is UTF-8 by construction, so `len()` is the exact wire byte
        // count: header width is known before the payload moves, and one
        // reservation covers both.
        e.append_prefixed_bytes(self.as_bytes())
    }
}

impl Layout for String {
    const LEN: usize = 0;
}

impl Encode for String {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(self.as_bytes())
    }

    #[inline]
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append_prefixed_bytes(self.as_bytes())
    }
}

impl<'de> Decode<'de> for String {
    fn decode(payload: &'de [u8]) -> Result<Self> {
        // Raw position: the whole slice is the payload. In `auto` position the
        // framework hands over exactly the prefix-declared bytes.
        String::from_utf8(payload.to_vec()).map_err(|_| Error::message_static("invalid UTF-8"))
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
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(self.as_bytes())
    }

    #[inline]
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append_prefixed_bytes(self.as_bytes())
    }
}

impl<'de> Decode<'de> for &'de str {
    fn decode(payload: &'de [u8]) -> Result<Self> {
        core::str::from_utf8(payload).map_err(|_| Error::message_static("invalid UTF-8"))
    }
}

// ---- &[u8] ----

impl Layout for [u8] {
    const LEN: usize = 0;
}

impl Encode for [u8] {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(self)
    }

    #[inline]
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append_prefixed_bytes(self)
    }
}

/// Layout/Encode for `&[u8]` mirror `[u8]`.
impl Layout for &[u8] {
    const LEN: usize = 0;
}

impl Encode for &[u8] {
    #[inline]
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append(self)
    }

    #[inline]
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        e.append_prefixed_bytes(self)
    }
}

impl<'de> Decode<'de> for &'de [u8] {
    #[inline]
    fn decode(payload: &'de [u8]) -> Result<Self> {
        Ok(payload)
    }
}

// ---- Vec<T> ----

impl<T> Layout for Vec<T> {
    const LEN: usize = 0;
}

impl<T: Encode> Encode for Vec<T> {
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        match T::LEN {
            // O(1): fixed-size elements need no per-element prefix.
            n if n > 0 => e.append_with_length_prefix_bounded(self.len().saturating_mul(n), |e| {
                self.encode(e)
            }),
            // No cheap estimate available — `append_with_length_prefix` fallback, never recurse.
            _ => e.append_with_length_prefix(|e| self.encode(e)),
        }
    }

    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        // Delegated to the element: fixed-width scalars override `encode_bulk`
        // with one bulk write, everything else gets the loop (`codec.rs`).
        T::encode_bulk(self, e)
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for Vec<T> {
    fn decode(payload: &'de [u8]) -> Result<Self> {
        // Raw position: the whole slice is the payload. In `auto` position the
        // framework hands over exactly the prefix-declared bytes.
        match T::LEN {
            elem_len if elem_len > 0 => {
                if !payload.len().is_multiple_of(elem_len) {
                    return Err(Error::message_static("misaligned element payload"));
                }
                let count = payload.len() / elem_len;
                if count == 0 {
                    return Ok(Vec::new());
                }
                // Same delegation on the way in; a type that does not override
                // `decode_bulk` runs `T::decode` per value.
                let vec = T::decode_bulk(payload, count)?;
                debug_assert_eq!(vec.len(), count);
                Ok(vec)
            }
            _ => {
                let mut cur = payload;
                let mut vec = Vec::new();
                while !cur.is_empty() {
                    vec.push(T::decode_auto(&mut cur)?);
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
    fn encode_with_length_prefix(&self, e: &mut Allocator<'_>) -> Result<()> {
        match T::LEN {
            n if n > 0 => {
                e.append_with_length_prefix_bounded(N.saturating_mul(n), |e| self.encode(e))
            }
            _ => e.append_with_length_prefix(|e| self.encode(e)),
        }
    }

    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        T::encode_bulk(self, e)
    }
}

impl<'de, T: Decode<'de>, const N: usize> Decode<'de> for [T; N] {
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let mut cur = payload;
        let mut arr: mem::MaybeUninit<[T; N]> = mem::MaybeUninit::uninit();
        let ptr = arr.as_mut_ptr() as *mut T;
        let mut initialized = 0;
        let result = (|| {
            for i in 0..N {
                unsafe { ptr.add(i).write(T::decode_auto(&mut cur)?) };
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

// The unit type is deliberately not convertible, matching the reference C#
// implementation: there, `System.ValueTuple` throws `ArgumentException` in the
// reflection generator and reports `BINSRCGEN13` ("Type Not Recognized") in the
// source generator. The one-byte-per-conversion rule is no part of this
// decision — it governs the auto forms only (raw forms are unconstrained), and
// `()` could satisfy it with a bare `0x00` prefix. A zero-field union variant
// writes only its tag inside the derive and never reaches for `()`, mirroring
// C#'s empty union case (`IsEmptyUnionCaseType`); a zero-field struct is
// rejected by `binary-derive`. Zero-length arrays stay convertible: `[T; 0]`
// below keeps the `LEN == 0` treatment.

// ---- Tuples ----

// Tuples are flattened on the wire:
// - `encode` (raw): all but the last member use `auto`; the last is raw, its
//   length implied by the remaining bytes.
// - `encode_auto`: **every** member uses `auto`, and the tuple adds no header of
//   its own — so `((a,b),c)` and `(a,b,c)` produce identical bytes.
// - `encode_with_length_prefix` keeps the default (prefix + `encode`).
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
    ([$($val:expr),*] ; $cur:ident ; $idx:tt $name:ident) => {
        ($($val,)* $name::decode($cur)?,)
    };
    ([$($val:expr),*] ; $cur:ident ; $idx:tt $name:ident, $($rest_idx:tt $rest:ident),+) => {
        tuple_decode_raw!([$($val,)* $name::decode_auto(&mut $cur)?] ; $cur ; $($rest_idx $rest),+)
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
            fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
                tuple_encode_raw!(e, self; $($idx $name),+)
            }

            fn encode_auto(&self, e: &mut Allocator<'_>) -> Result<()> {
                $( self.$idx.encode_auto(e)?; )+
                Ok(())
            }
        }

        impl<'de, $($name: Decode<'de>),+> Decode<'de> for ($($name,)+) {
            fn decode(__payload: &'de [u8]) -> Result<Self> {
                // Intermediate members go through `auto`; the last one is raw,
                // its length implied by the bytes that are left.
                let mut __cur = __payload;
                Ok(tuple_decode_raw!([] ; __cur ; $($idx $name),+))
            }

            fn decode_auto(__buf: &mut &'de [u8]) -> Result<Self> {
                Ok(($( $name::decode_auto(__buf)?, )+))
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
