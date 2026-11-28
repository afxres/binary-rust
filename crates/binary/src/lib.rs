#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod allocator;
mod codec;
mod collections;
pub mod components;
mod container;
mod converter;
mod error;
mod hex;
mod length;
mod oneshot;
mod option;
mod pod;
mod primitives;
mod smart_ptr;

pub use allocator::Allocator;
pub use codec::{Decode, Encode, Layout};
pub use converter::{
    decode, decode_slice, decode_with_length_prefix, encode, encode_slice,
    encode_with_length_prefix,
};
pub use error::{Error, Result};
pub use oneshot::{decode_value, encode_value};

// Derive macros share the names of the traits (macro namespace vs type
// namespace), so `use binary::Encode` brings in both.
#[cfg(feature = "derive")]
pub use binary_derive::{Decode, Encode};
