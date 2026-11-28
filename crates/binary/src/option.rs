//! `Option<T>` wire form.
//!
//! ```text
//! None    -> tag 0x00
//! Some(v) -> tag 0x01, then v
//! ```
//!
//! Neither form carries a length prefix in `auto` position: decoding is
//! self-delimiting, because the tag is one byte and `T` is either fixed-size or
//! itself prefixed. A prefix still appears where the framework demands one — a
//! named-object member value — through the default `encode_with_prefix`.

use crate::allocator::Allocator;
use crate::codec::{Decode, Encode, Layout};
use crate::converter::take_byte;
use crate::error::{Error, Result};

const TAG_NONE: u8 = 0;
const TAG_SOME: u8 = 1;

impl<T> Layout for Option<T> {
    /// 1 byte (`None`) or 1 + the value: never a single fixed size.
    const LEN: usize = 0;
}

impl<T: Encode> Encode for Option<T> {
    /// Raw form (no prefix): tag + raw value.
    fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
        match self {
            None => e.append_byte(TAG_NONE),
            Some(v) => {
                e.append_byte(TAG_SOME)?;
                v.encode(e)
            }
        }
    }

    /// Auto form (no prefix): tag + `auto` value.
    fn encode_auto(&self, e: &mut Allocator<'_>) -> Result<()> {
        match self {
            None => e.append_byte(TAG_NONE),
            Some(v) => {
                e.append_byte(TAG_SOME)?;
                v.encode_auto(e)
            }
        }
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for Option<T> {
    fn decode(payload: &'de [u8]) -> Result<Self> {
        let (&tag, rest) = payload.split_first().ok_or(Error::NotEnoughBytes)?;
        match tag {
            TAG_NONE => Ok(None),
            TAG_SOME => Ok(Some(T::decode(rest)?)),
            other => Err(Error::message(format_args!("invalid option tag: {other}"))),
        }
    }

    // Self-delimiting: the tag is the boundary, so `auto` does NOT read a
    // length prefix — the trait default would be wrong here.
    fn decode_auto(buf: &mut &'de [u8]) -> Result<Self> {
        match take_byte(buf)? {
            TAG_NONE => Ok(None),
            TAG_SOME => Ok(Some(T::decode_auto(buf)?)),
            other => Err(Error::message(format_args!("invalid option tag: {other}"))),
        }
    }
}
