#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod codec;
mod collections;
mod container;
mod decoder;
mod encoder;
mod error;
mod hex;
mod length;
mod named;
mod oneshot;
mod option;
mod pod;
mod primitives;
mod smart_ptr;

pub use codec::{Decode, Encode, Layout};
pub use decoder::Decoder;
pub use encoder::Encoder;
pub use error::{ErasedError, Error, Result};
pub use length::{decode_prefix, encode_prefix, encode_prefix_wide, prefix_header_len};
pub use named::{read_named_member, write_named_member, NamedMembers};
pub use oneshot::{from_slice, to_vec};

// Derive macros share the names of the traits (macro namespace vs type
// namespace), so `use binary::Encode` brings in both.
#[cfg(feature = "derive")]
pub use binary_derive::{Decode, Encode};
