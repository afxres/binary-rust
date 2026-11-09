//! Standard-library containers beyond `Vec` / arrays / tuples.
//!
//! # Wire shape
//!
//! Every sequence here has **one** wire form, byte-for-byte identical to
//! `Vec<T>`'s raw payload: no element count, no tag, just each element written
//! through [`Encode::encode_auto`] back to back. The boundary comes from the
//! *outer* length prefix written by `encode_with_prefix` / `encode_auto`.
//!
//! Maps encode each entry as a flattened `(K, V)` tuple — `K` then `V`, with
//! **no pair header** — which is why `(K, V)` flattens in `encode_auto` /
//! `decode_auto`.
//!
//! Decoding delegates to `Vec<T>::decode`, which already derives the element
//! count for fixed-size elements, rejects misaligned payloads with
//! `Error::MisalignedElements`, and leaves the bulk-or-loop decision to the
//! element's `decode_slice` hook. Each container is then built with `collect()`.
//!
//! # Ordering is *not* a wire guarantee
//!
//! `HashMap` carries a per-process random seed and `BinaryHeap::iter()` yields
//! the heap's internal array order, so hash containers and heaps can only
//! promise "a KV sequence every implementation can decode", never byte equality
//! across languages. `BTreeMap` / `BTreeSet` are key-ordered and can be
//! byte-aligned with an ordered container on the other side.
//!
//! Two further semantics: decoding a duplicate key quietly overwrites here
//! (`FromIterator`), and `Cow` always decodes to the `Owned` variant — zero-copy
//! borrowing is already available as `&'de str` / `&'de [u8]`.
//!
//! # Dependency boundary
//!
//! Everything in `alloc` compiles unconditionally, so `--no-default-features`
//! exercises it. `HashMap` / `HashSet` sit behind `#[cfg(feature = "std")]`
//! because those types live in `std`. The hasher is generic
//! (`S: BuildHasher`), so `RandomState` works without `hashbrown`.

use crate::codec::{Decode, Encode, Layout};
use crate::decoder::Decoder;
use crate::encoder::Encoder;
use crate::error::Result;

use alloc::borrow::{Cow, ToOwned};
use alloc::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};
use alloc::vec::Vec;

#[cfg(feature = "std")]
use std::collections::{HashMap, HashSet};
#[cfg(feature = "std")]
use std::hash::{BuildHasher, Hash};

/// Write the raw payload of a sequence: no count, every element through `encode_auto`.
fn encode_items<'a, T: Encode + 'a>(
    items: impl IntoIterator<Item = &'a T>,
    e: &mut Encoder<'_>,
) -> Result<()> {
    for item in items {
        item.encode_auto(e)?;
    }
    Ok(())
}

/// Shared `encode_with_prefix`: fixed-size elements take the O(1) reserve path,
/// variable-size ones `Encoder::write_prefixed` — the same split as `Vec<T>`.
fn encode_items_with_prefix<'a, T: Encode + 'a>(
    len: usize,
    items: impl IntoIterator<Item = &'a T>,
    e: &mut Encoder<'_>,
) -> Result<()> {
    match T::LEN {
        n if n > 0 => e.write_prefixed_bounded(len.saturating_mul(n), |e| encode_items(items, e)),
        _ => e.write_prefixed(|e| encode_items(items, e)),
    }
}

/// A map entry is a flattened `(K, V)` tuple: both members through `auto`,
/// no header of its own.
fn encode_pairs<'a, K: Encode + 'a, V: Encode + 'a>(
    items: impl IntoIterator<Item = (&'a K, &'a V)>,
    e: &mut Encoder<'_>,
) -> Result<()> {
    for (k, v) in items {
        k.encode_auto(e)?;
        v.encode_auto(e)?;
    }
    Ok(())
}

fn encode_pairs_with_prefix<'a, K: Encode + 'a, V: Encode + 'a>(
    len: usize,
    items: impl IntoIterator<Item = (&'a K, &'a V)>,
    e: &mut Encoder<'_>,
) -> Result<()> {
    // `(K, V)::LEN` is the sum of the members' lengths, or 0 when either is
    // variable — exactly the estimate `Vec<(K, V)>` would use.
    match <(K, V) as Layout>::LEN {
        n if n > 0 => e.write_prefixed_bounded(len.saturating_mul(n), |e| encode_pairs(items, e)),
        _ => e.write_prefixed(|e| encode_pairs(items, e)),
    }
}

// ---- VecDeque ----

impl<T> Layout for VecDeque<T> {
    const LEN: usize = 0;
}

impl<T: Encode> Encode for VecDeque<T> {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        // A deque is two contiguous runs. Delegating each run to the element
        // keeps whatever the element offers — the scalars' bulk write, or the
        // per-element loop for everything else (`codec.rs`).
        let (head, tail) = self.as_slices();
        T::encode_slice(head, e)?;
        T::encode_slice(tail, e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        match T::LEN {
            n if n > 0 => {
                e.write_prefixed_bounded(self.len().saturating_mul(n), |e| self.encode(e))
            }
            _ => e.write_prefixed(|e| self.encode(e)),
        }
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for VecDeque<T> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(<Vec<T> as Decode<'de>>::decode(d)?.into_iter().collect())
    }
}

// ---- LinkedList ----

impl<T> Layout for LinkedList<T> {
    const LEN: usize = 0;
}

impl<T: Encode> Encode for LinkedList<T> {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_items(self.iter(), e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_items_with_prefix(self.len(), self.iter(), e)
    }
}

impl<'de, T: Decode<'de>> Decode<'de> for LinkedList<T> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(<Vec<T> as Decode<'de>>::decode(d)?.into_iter().collect())
    }
}

// ---- Ordered sets ----

macro_rules! impl_ordered_set {
    ($ty:ident) => {
        impl<T> Layout for $ty<T> {
            const LEN: usize = 0;
        }

        impl<T: Encode + Ord> Encode for $ty<T> {
            fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
                encode_items(self.iter(), e)
            }

            fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
                encode_items_with_prefix(self.len(), self.iter(), e)
            }
        }

        impl<'de, T: Decode<'de> + Ord> Decode<'de> for $ty<T> {
            fn decode(d: &mut Decoder<'de>) -> Result<Self> {
                Ok(<Vec<T> as Decode<'de>>::decode(d)?.into_iter().collect())
            }
        }
    };
}

impl_ordered_set!(BTreeSet);
impl_ordered_set!(BinaryHeap);

// ---- BTreeMap ----

impl<K, V> Layout for BTreeMap<K, V> {
    const LEN: usize = 0;
}

impl<K: Encode + Ord, V: Encode> Encode for BTreeMap<K, V> {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_pairs(self.iter(), e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_pairs_with_prefix(self.len(), self.iter(), e)
    }
}

impl<'de, K: Decode<'de> + Ord, V: Decode<'de>> Decode<'de> for BTreeMap<K, V> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(<Vec<(K, V)> as Decode<'de>>::decode(d)?
            .into_iter()
            .collect())
    }
}

// ---- HashMap / HashSet (std only) ----

#[cfg(feature = "std")]
impl<T, S> Layout for HashSet<T, S> {
    const LEN: usize = 0;
}

#[cfg(feature = "std")]
impl<T: Encode + Eq + Hash, S: BuildHasher> Encode for HashSet<T, S> {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_items(self.iter(), e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_items_with_prefix(self.len(), self.iter(), e)
    }
}

#[cfg(feature = "std")]
impl<'de, T: Decode<'de> + Eq + Hash, S: BuildHasher + Default> Decode<'de> for HashSet<T, S> {
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(<Vec<T> as Decode<'de>>::decode(d)?.into_iter().collect())
    }
}

#[cfg(feature = "std")]
impl<K, V, S> Layout for HashMap<K, V, S> {
    const LEN: usize = 0;
}

#[cfg(feature = "std")]
impl<K: Encode + Eq + Hash, V: Encode, S: BuildHasher> Encode for HashMap<K, V, S> {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_pairs(self.iter(), e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        encode_pairs_with_prefix(self.len(), self.iter(), e)
    }
}

#[cfg(feature = "std")]
impl<'de, K: Decode<'de> + Eq + Hash, V: Decode<'de>, S: BuildHasher + Default> Decode<'de>
    for HashMap<K, V, S>
{
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(<Vec<(K, V)> as Decode<'de>>::decode(d)?
            .into_iter()
            .collect())
    }
}

// ---- Cow ----

/// `Cow` is wire-transparent: all three encode methods forward to `B` so a
/// borrowed and an owned value produce the same bytes (and so `str`'s O(1)
/// `encode_with_prefix` override is not lost).
impl<'a, B> Layout for Cow<'a, B>
where
    B: Layout + ToOwned + ?Sized,
{
    const LEN: usize = B::LEN;
}

impl<'a, B> Encode for Cow<'a, B>
where
    B: Encode + ToOwned + ?Sized,
{
    fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
        <B as Encode>::encode(&**self, e)
    }

    fn encode_with_prefix(&self, e: &mut Encoder<'_>) -> Result<()> {
        <B as Encode>::encode_with_prefix(&**self, e)
    }

    fn encode_auto(&self, e: &mut Encoder<'_>) -> Result<()> {
        <B as Encode>::encode_auto(&**self, e)
    }
}

/// Decoding always produces `Owned`. A generic `Cow` cannot decide whether
/// `&'de B` is decodable — only `str` and `[u8]` have that impl — so the
/// borrowed form stays the caller's choice via `&'de str` / `&'de [u8]`.
impl<'de, 'a, B> Decode<'de> for Cow<'a, B>
where
    B: Layout + ToOwned + ?Sized,
    B::Owned: Decode<'de>,
{
    fn decode(d: &mut Decoder<'de>) -> Result<Self> {
        Ok(Cow::Owned(<B::Owned as Decode<'de>>::decode(d)?))
    }
}
