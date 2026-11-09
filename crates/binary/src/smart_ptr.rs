//! Wire-transparent owning pointers: `Box<T>`, `Rc<T>`, `Arc<T>`.
//!
//! The indirection and the ownership policy are Rust details, not part of the
//! format: all three forward to `T` verbatim in raw, `auto`, and prefixed
//! positions, so `Box<T>`, `Rc<T>`, `Arc<T>` and `T` produce identical bytes.
//!
//! * `Layout::LEN` is inherited so containers keep the element size
//!   (`Vec<Box<i32>>` == `Vec<i32>`), but the bulk hooks are **not** overridden.
//!   `size_of::<Self>()` is a pointer: a bulk write would put pointer bytes on
//!   the wire, and a bulk read would adopt wire bytes as owning pointers.
//! * Decoding always builds a fresh allocation. Object identity is not part of
//!   the format, so two clones of one `Rc` decode as two independent values.
//!
//! `Encode` accepts `T: ?Sized` because it only borrows `&**self`; `Decode`
//! requires `T: Sized`. DST pointers therefore encode like their borrowed
//! values but cannot be decoded.
//!
//! This is also what makes `struct Node { next: Option<Box<Node>> }` derivable:
//! `Option<T>::LEN == 0` stops the derive's const layout evaluation. There is no
//! depth guard, so a deep chain can exhaust the stack (`DESIGN.md` §9).

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::sync::Arc;

use crate::codec::{Decode, Encode, Layout};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::Result;

macro_rules! impl_transparent {
    ($ty:ident) => {
        impl<T: Layout + ?Sized> Layout for $ty<T> {
            // Non-zero for `Box<i32>`, so containers do enter their fixed-size
            // branch — but the pointer owns memory, so the bulk hooks stay at
            // their default (`DESIGN` §19.16).
            const LEN: usize = T::LEN;
        }

        impl<T: Encode + ?Sized> Encode for $ty<T> {
            // All three are forwarded explicitly. The default `encode_auto` /
            // `encode_with_prefix` dispatch on `Self::LEN` and would silently
            // lose an override on `T` (tuple flattening, `Option<T>`, the O(1)
            // prefix of `String`).
            #[inline]
            fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
                (**self).encode(e)
            }

            #[inline]
            fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
                (**self).encode_with_prefix(e)
            }

            #[inline]
            fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
                (**self).encode_auto(e)
            }
        }

        impl<'de, T: Decode<'de>> Decode<'de> for $ty<T> {
            #[inline]
            fn decode(d: &mut Decoder<'de>) -> Result<Self> {
                Ok($ty::new(T::decode(d)?))
            }

            #[inline]
            fn decode_auto(d: &mut Decoder<'de>) -> Result<Self> {
                Ok($ty::new(T::decode_auto(d)?))
            }
        }
    };
}

impl_transparent!(Box);
impl_transparent!(Rc);
impl_transparent!(Arc);
