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
//! The protocol is two free functions, [`encode_named_member`] and
//! [`decode_named_member`], not methods on the allocator: a member is
//! not a byte-cursor operation, and hanging it on the cursor types read as a
//! privilege granted to one object shape. Each half exists in exactly one
//! function, so a name without a value has no expressible path.
//!
//! The object level is asymmetric on purpose. Writing streams, so one [`Allocator`]
//! used repeatedly *is* the object; reading has to buffer every member before a
//! field can be decoded, since member order is not part of the format, and that
//! buffer is [`NamedMembers`].

use crate::allocator::Allocator;
use crate::codec::{Decode, Encode};
use crate::error::{Error, Result};

/// Encode one named-object member: `prefix(name) name prefix(value) value`.
///
/// The name takes a single reservation, and the value prefix is unconditional
/// even for types with `LEN > 0`, so this calls [`Encode::encode_with_prefix`]
/// directly instead of `auto`.
///
/// A free function rather than a method on [`Allocator`]: a member is not a
/// byte-cursor operation, and hanging it on the cursor would read as a privilege
/// granted to one object shape.
#[inline]
pub fn encode_named_member<T: Encode + ?Sized>(
    e: &mut Allocator<'_>,
    name: &[u8],
    value: &T,
) -> Result<()> {
    e.append_prefixed_bytes(name)?;
    value.encode_with_prefix(e)
}

/// Decode one named-object member: `prefix(name) name prefix(value) value`.
///
/// Returns the name and the **raw value payload**: the value prefix is consumed
/// here, so the caller decodes the payload with `T::decode`, never `auto`. The
/// decode half of [`encode_named_member`].
///
/// This reads exactly one member and stops. It does **not** skip unknown names,
/// reject duplicates, or buffer anything — a full decoder needs
/// [`NamedMembers::parse`], which is the only complete entry point.
#[inline]
pub fn decode_named_member<'a>(buf: &mut &'a [u8]) -> Result<(&'a [u8], &'a [u8])> {
    let name = crate::converter::decode_with_length_prefix(buf)?;
    let value = crate::converter::decode_with_length_prefix(buf)?;
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
    pub fn parse(payload: &'a [u8], find: impl Fn(&'a [u8]) -> Option<usize>) -> Result<Self> {
        let mut slots: [Option<&'a [u8]>; N] = [None; N];
        let mut cur = payload;
        while !cur.is_empty() {
            let (name, value) = decode_named_member(&mut cur)?;
            let Some(index) = find(name) else {
                continue;
            };
            if index >= N {
                // A `find` table answering with an out-of-range index is an
                // invariant violation of the generated code, not bad input.
                return Err(Error::message_static("invalid member index"));
            }
            if slots[index].is_some() {
                return Err(Error::message_static("duplicate named member"));
            }
            slots[index] = Some(value);
        }
        Ok(Self { slots })
    }

    /// Raw value payload of a member, or `None` if it did not appear.
    ///
    /// The only accessor that hands back bytes instead of a decoded value; a
    /// hand-written decoder uses it when it has to inspect a payload before
    /// choosing a type. `value` / `optional` cover the normal case.
    #[inline]
    pub fn get(&self, index: usize) -> Option<&'a [u8]> {
        self.slots.get(index).copied().flatten()
    }

    /// Raw value payload of a required member.
    #[inline]
    pub(crate) fn require(&self, index: usize, name: &'static str) -> Result<&'a [u8]> {
        // `ok_or_else`, not `ok_or`: the message is formatted lazily, so the
        // success path stays allocation-free.
        self.get(index)
            .ok_or_else(|| Error::message(format_args!("missing required member: {name}")))
    }

    /// Decode a required member. Raw `T::decode`, without reading another prefix.
    #[inline]
    pub fn value<T: Decode<'a>>(&self, index: usize, name: &'static str) -> Result<T> {
        let payload = self.require(index, name)?;
        T::decode(payload)
    }

    /// Decode a member, falling back to `T::default()` when it did not appear.
    ///
    /// The `#[binary(optional)]` counterpart of [`Self::value`]. Named for what it
    /// returns, not for the attribute: `optional` read as a predicate ("is this
    /// member optional?"), while every call here decodes a value.
    #[inline]
    pub fn value_or_default<T: Decode<'a> + Default>(&self, index: usize) -> Result<T> {
        match self.get(index) {
            Some(payload) => T::decode(payload),
            None => Ok(T::default()),
        }
    }
}
