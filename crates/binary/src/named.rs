//! Named object support.
//!
//! Wire form of one member:
//!
//! ```text
//! [prefix(name)] [name bytes] [prefix(value)] [value payload]
//! ```
//!
//! Both the name and the value are **unconditionally** length-prefixed, unlike
//! `auto`: even a fixed-size `i32` value carries a prefix here. The object
//! itself has `Layout::LEN = 0`; whether an outer prefix exists is decided by
//! [`Encode::encode_auto`] / [`Decode::decode_auto`].
//!
//! Member order is **not** part of the format — decoding looks members up by
//! name — so this module writes declaration order and never sorts. A
//! cross-language comparison therefore asserts the decoded value, not the bytes
//! (`DESIGN.md` §19.4).
//!
//! The protocol is two free functions, [`write_named_member`] and
//! [`read_named_member`], not methods on [`Encoder`] / [`Decoder`]: a member is
//! not a byte-cursor operation, and hanging it on the cursor types read as a
//! privilege granted to one object shape. Each half exists in exactly one
//! function, so a name without a value has no expressible path.
//!
//! The object level is asymmetric on purpose. Writing streams, so one [`Encoder`]
//! used repeatedly *is* the object; reading has to buffer every member before a
//! field can be decoded, since member order is not part of the format, and that
//! buffer is [`NamedMembers`].

use crate::codec::{Decode, Encode};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::{Error, Result};

/// Write one named-object member: `prefix(name) name prefix(value) value`.
///
/// The name takes a single reservation, and the value prefix is unconditional
/// even for types with `LEN > 0`, so this calls [`Encode::encode_with_prefix`]
/// directly instead of `auto`.
///
/// A free function rather than a method on [`Encoder`]: a member is not a
/// byte-cursor operation, and hanging it on the cursor would read as a privilege
/// granted to one object shape.
#[inline]
pub fn write_named_member<T: Encode + ?Sized>(
    e: &mut Encoder<'_>,
    name: &[u8],
    value: &T,
) -> Result<()> {
    e.write_prefixed_bytes(name)?;
    value.encode_with_prefix(e)
}

/// Read one named-object member: `prefix(name) name prefix(value) value`.
///
/// Returns the name and the **raw value payload**: the value prefix is consumed
/// here, so the caller decodes the payload with `T::decode`, never `auto`. The
/// read half of [`write_named_member`].
#[inline]
pub fn read_named_member<'a>(d: &mut Decoder<'a>) -> Result<(&'a [u8], &'a [u8])> {
    let name = d.read_prefix()?;
    let value = d.read_prefix()?;
    Ok((name, value))
}

/// Parsed named-object members, indexed by declaration order.
///
/// `N` is the number of known members. Each slot holds the **raw value payload**;
/// the value prefix was already consumed by [`NamedMembers::parse`], so decoding a
/// slot must use `T::decode` (raw), never `auto::<T>()`.
pub struct NamedMembers<'a, const N: usize> {
    slots: [Option<&'a [u8]>; N],
}

impl<'a, const N: usize> NamedMembers<'a, N> {
    /// Parse all remaining bytes in `d` as named members.
    ///
    /// `find` maps a wire name to a declaration index (a generated `match` in the
    /// derive macro). Unknown names are skipped for forward compatibility;
    /// a known name appearing twice is an error.
    pub fn parse(d: &mut Decoder<'a>, find: impl Fn(&'a [u8]) -> Option<usize>) -> Result<Self> {
        let mut slots: [Option<&'a [u8]>; N] = [None; N];
        while d.remaining() > 0 {
            let (name, value) = read_named_member(d)?;
            let Some(index) = find(name) else {
                continue;
            };
            if index >= N {
                return Err(Error::InvalidMemberIndex);
            }
            if slots[index].is_some() {
                return Err(Error::DuplicateMember);
            }
            slots[index] = Some(value);
        }
        Ok(Self { slots })
    }

    /// Raw value payload of a member, or `None` if it did not appear.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&'a [u8]> {
        self.slots.get(index).copied().flatten()
    }

    /// Raw value payload of a required member.
    #[inline]
    pub fn require(&self, index: usize, name: &'static str) -> Result<&'a [u8]> {
        self.get(index).ok_or(Error::MissingMember(name))
    }

    /// Decode a required member. Raw `T::decode`, without reading another prefix.
    #[inline]
    pub fn value<T: Decode<'a>>(&self, index: usize, name: &'static str) -> Result<T> {
        let payload = self.require(index, name)?;
        T::decode(&mut Decoder::new(payload))
    }

    /// Decode an optional member; a missing member yields `T::default()`.
    #[inline]
    pub fn optional<T: Decode<'a> + Default>(&self, index: usize) -> Result<T> {
        match self.get(index) {
            Some(payload) => T::decode(&mut Decoder::new(payload)),
            None => Ok(T::default()),
        }
    }
}
