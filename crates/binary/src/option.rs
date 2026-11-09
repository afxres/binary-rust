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

use crate::codec::{Decode, Encode, Layout};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::{Error, Result};

const TAG_NONE: u8 = 0;
const TAG_SOME: u8 = 1;

impl<T> Layout for Option<T> {
    /// 1 byte (`None`) or 1 + the value: never a single fixed size.
    const LEN: usize = 0;
}

impl<T: Encode> Encode for Option<T> {
    /// Raw form (no prefix): tag + raw value.
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        match self {
            None => e.write_byte(TAG_NONE),
            Some(v) => {
                e.write_byte(TAG_SOME)?;
                v.encode(e)
            }
        }
    }

    /// Auto form (no prefix): tag + `auto` value.
    fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
        match self {
            None => e.write_byte(TAG_NONE),
            Some(v) => {
                e.write_byte(TAG_SOME)?;
                v.encode_auto(e)
            }
        }
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for Option<T> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        match d.take_byte()? {
            TAG_NONE => Ok(None),
            TAG_SOME => Ok(Some(T::decode(d)?)),
            other => Err(Error::InvalidTag(other as u32)),
        }
    }

    fn decode_auto(d: &mut Decoder<'de>) -> Result<Self> {
        match d.take_byte()? {
            TAG_NONE => Ok(None),
            TAG_SOME => Ok(Some(T::decode_auto(d)?)),
            other => Err(Error::InvalidTag(other as u32)),
        }
    }
}
