//! Named-object member lookup: is the derive's `match` worth replacing?
//!
//! The derive emits `|name| match name { b"id" => Some(0), … }` and DESIGN §5
//! leaves the question open on purpose: 先量再改. The C++ `named-lookup` suite
//! (`binary-benchmarks`) already ranks eight table schemes on a generated key
//! set, but the Rust baseline has never been on the same case set, so every
//! "Rust could use packed words" claim is currently unmeasured.
//!
//! Schemes measured here, all against the same `&[u8] -> Option<usize>` shape
//! the derive's `find` closure has:
//!
//! - `match`   — the derive's shape, hand-written for the three literal schemas;
//! - `linear`  — length + memcmp over an array: what `match` degrades into once
//!   the compiler stops inlining the comparisons (the C++ `findmember` row);
//! - `longdata`— C# `GetLongData`: keys ≤ 15 B packed into 2 × u64, ≤ 11 entries,
//!   `chain` fallback beyond that;
//! - `chain`   — `hash * 33` buckets with per-slot chaining;
//! - `open`    — FNV-1a, power-of-two table, linear probing;
//! - `perfect` — run-time seed search, one probe + verification;
//! - `sorted`  — sorted array + binary search.
//!
//! Not ported from the C++ device: `unordered` (a Rust `HashMap` needs owned
//! `String` keys, so the row would measure the allocation, not the lookup) and
//! `simd` / `swiss` (C++ showed neither beats `chain` / `open`).
//!
//! Measurement rules are this crate's (see `harness::measure`): 2000 warm-up
//! calls, 7 rounds, doubling to a 50 ms budget, minimum. Two deliberate
//! differences from the C++ device, both documented so the numbers are not
//! subtracted across devices: the query cursor wraps with a compare instead of
//! `i % len` (a run-time `%` is a `div` in the loop that the C++ device pays and
//! this one does not), and every table is `Vec`-backed exactly like C++'s
//! `std::vector`, i.e. **built at run time**. A `const`-built table would remove
//! `build_ns` and one heap indirection; that is a follow-up, not this round.
//!
//! Case set mirrors `named-lookup` (same xorshift, same seeds, same labels), so
//! `n11` / `len15` mean the same key bytes on both sides.
//!
//! ```text
//! cargo run --release -p binary-bench -- lookup         # summary, seq + hit
//! cargo run --release -p binary-bench -- lookup full    # whole matrix, pipe rows
//! ```

use std::hint::black_box;
use std::time::Instant;

use crate::harness::{header, measure};

// ---------------------------------------------------------------- key sets

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Same generator as the C++ device: `count` distinct keys of `min..=max` bytes
/// over `a..=z`, leaked so every scheme can hold `&'static [u8]`.
fn make_keys(count: usize, min_len: usize, max_len: usize, seed: u64) -> Vec<&'static [u8]> {
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let mut arena: Vec<u8> = Vec::new();
    let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
    let mut spans = Vec::with_capacity(count);
    while spans.len() < count {
        let length = if max_len > min_len {
            min_len + (next_random(&mut state) % (max_len - min_len + 1) as u64) as usize
        } else {
            min_len
        };
        let mut text = vec![0u8; length];
        loop {
            for byte in text.iter_mut() {
                *byte = b'a' + (next_random(&mut state) % 26) as u8;
            }
            if seen.insert(text.clone()) {
                break;
            }
        }
        let offset = arena.len();
        arena.extend_from_slice(&text);
        spans.push((offset, length));
    }
    let arena: &'static [u8] = Box::leak(arena.into_boxed_slice());
    spans.into_iter().map(|(o, l)| &arena[o..o + l]).collect()
}

/// A key of `length` bytes that is **not** in `keys` — the forward-compatibility
/// miss the decoder is required to skip.
fn make_unknown(keys: &[&'static [u8]], length: usize, seed: u64) -> &'static [u8] {
    let mut state = seed;
    loop {
        let mut text = vec![0u8; length];
        for byte in text.iter_mut() {
            *byte = b'a' + (next_random(&mut state) % 26) as u8;
        }
        if !keys.iter().any(|key| *key == &text[..]) {
            return Box::leak(text.into_boxed_slice());
        }
    }
}

struct Case {
    id: &'static str,
    keys: Vec<&'static [u8]>,
    /// Length used for the miss key; only matters in `miss` mode.
    query_len: usize,
}

/// The literal schemas. `match` can only exist for these, because the derive's
/// closure needs byte-string patterns, so they double as the check that the
/// table schemes agree with the code the derive actually emits.
///
/// `real4` keeps the C# spelling (`Id / Item / List / Name`) so the row lines up
/// with the C++ device; `real8` / `real16` / `real32` are snake_case, i.e. what a
/// Rust struct without `rename` would look like. `real32` is past the 11-entry
/// packed boundary and past anything a hand-written schema usually reaches —
/// it exists to find where `match` stops winning.
const REAL4: [&[u8]; 4] = [b"Id", b"Item", b"List", b"Name"];
const REAL8: [&[u8]; 8] = [
    b"id", b"name", b"list", b"item", b"tags", b"data", b"count", b"offset",
];
const REAL16: [&[u8]; 16] = [
    b"id", b"name", b"list", b"item", b"tags", b"data", b"count", b"offset", b"length", b"kind",
    b"value", b"next", b"prev", b"flags", b"owner", b"source",
];
const REAL32: [&[u8]; 32] = [
    b"id",
    b"name",
    b"list",
    b"item",
    b"tags",
    b"data",
    b"count",
    b"offset",
    b"length",
    b"kind",
    b"value",
    b"next",
    b"prev",
    b"flags",
    b"owner",
    b"source",
    b"created",
    b"updated",
    b"version",
    b"parent",
    b"children",
    b"label",
    b"title",
    b"status",
    b"weight",
    b"height",
    b"width",
    b"color",
    b"shape",
    b"origin",
    b"target",
    b"region",
];

/// The worst case for a `match`: every name has the same length, so the
/// length switch cannot narrow anything and the compiler has to dispatch on the
/// remaining bytes alone. All 16 are real field-name words of exactly 6 bytes.
const FLAT16: [&[u8]; 16] = [
    b"amount", b"number", b"target", b"origin", b"region", b"status", b"weight", b"height",
    b"length", b"source", b"parent", b"street", b"record", b"column", b"buffer", b"cursor",
];

fn make_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for count in [1, 2, 4, 8, 11, 12, 16, 32, 64, 256] {
        cases.push(Case {
            id: Box::leak(format!("n{count}").into_boxed_str()),
            keys: make_keys(count, 4, 4, 1000 + count as u64),
            query_len: 4,
        });
    }
    for length in [1, 4, 8, 12, 15, 16, 24, 64] {
        cases.push(Case {
            id: Box::leak(format!("len{length}").into_boxed_str()),
            keys: make_keys(4, length, length, 2000 + length as u64),
            query_len: length,
        });
    }
    for seed in 1..=5u64 {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let count = 2 + (next_random(&mut state) % 63) as usize;
        let length = 1 + (next_random(&mut state) % 32) as usize;
        cases.push(Case {
            id: Box::leak(format!("rand{seed}").into_boxed_str()),
            keys: make_keys(count, length, length, 3000 + seed),
            query_len: length,
        });
    }
    cases.push(Case {
        id: "real4",
        keys: REAL4.to_vec(),
        query_len: 4,
    });
    cases.push(Case {
        id: "real8",
        keys: REAL8.to_vec(),
        query_len: 3,
    });
    cases.push(Case {
        id: "real16",
        keys: REAL16.to_vec(),
        query_len: 15,
    });
    cases.push(Case {
        id: "real32",
        keys: REAL32.to_vec(),
        query_len: 9,
    });
    cases.push(Case {
        id: "flat16",
        keys: FLAT16.to_vec(),
        query_len: 5,
    });
    cases
}

// ---------------------------------------------------------------- schemes

trait Lookup {
    fn build(keys: &[&'static [u8]]) -> Self;
    fn lookup(&self, key: &[u8]) -> Option<usize>;
    /// Bytes the table occupies, for the size column. `0` = code only.
    fn storage(&self) -> usize;
    fn usable(&self) -> bool {
        true
    }
}

fn same_keys(left: &[&[u8]], right: &[&[u8]]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// The derive's closure, spelled out: `b"…"` patterns on `&[u8]` are exactly
/// what `derive(Decode)` generates, so these rows *are* the current
/// implementation. One type per schema rather than one enum — an enum
/// discriminant would add a branch inside the measured loop that the derive does
/// not have.
///
/// The index is written out instead of counted, because a `match` arm list
/// cannot be assembled incrementally by `macro_rules!`. Two checks make a typo
/// fatal rather than silent: `build` asserts the arms are in the const key
/// order, and `measure_scheme` asserts every key resolves to its own index.
macro_rules! match_scheme {
    ($name:ident, $keys:expr; $($key:literal => $index:literal),+ $(,)?) => {
        struct $name;

        impl Lookup for $name {
            fn build(keys: &[&'static [u8]]) -> Self {
                assert!(
                    same_keys(keys, &$keys),
                    concat!(stringify!($name), " bound to the wrong key set")
                );
                Self
            }

            fn lookup(&self, key: &[u8]) -> Option<usize> {
                match key {
                    $($key => Some($index),)+
                    _ => None,
                }
            }

            fn storage(&self) -> usize {
                0
            }
        }
    };
}

match_scheme!(MatchReal4, REAL4;
    b"Id" => 0, b"Item" => 1, b"List" => 2, b"Name" => 3,
);

match_scheme!(MatchReal8, REAL8;
    b"id" => 0,
    b"name" => 1,
    b"list" => 2,
    b"item" => 3,
    b"tags" => 4,
    b"data" => 5,
    b"count" => 6,
    b"offset" => 7,
);

match_scheme!(MatchReal16, REAL16;
    b"id" => 0,
    b"name" => 1,
    b"list" => 2,
    b"item" => 3,
    b"tags" => 4,
    b"data" => 5,
    b"count" => 6,
    b"offset" => 7,
    b"length" => 8,
    b"kind" => 9,
    b"value" => 10,
    b"next" => 11,
    b"prev" => 12,
    b"flags" => 13,
    b"owner" => 14,
    b"source" => 15,
);

match_scheme!(MatchReal32, REAL32;
    b"id" => 0,
    b"name" => 1,
    b"list" => 2,
    b"item" => 3,
    b"tags" => 4,
    b"data" => 5,
    b"count" => 6,
    b"offset" => 7,
    b"length" => 8,
    b"kind" => 9,
    b"value" => 10,
    b"next" => 11,
    b"prev" => 12,
    b"flags" => 13,
    b"owner" => 14,
    b"source" => 15,
    b"created" => 16,
    b"updated" => 17,
    b"version" => 18,
    b"parent" => 19,
    b"children" => 20,
    b"label" => 21,
    b"title" => 22,
    b"status" => 23,
    b"weight" => 24,
    b"height" => 25,
    b"width" => 26,
    b"color" => 27,
    b"shape" => 28,
    b"origin" => 29,
    b"target" => 30,
    b"region" => 31,
);

match_scheme!(MatchFlat16, FLAT16;
    b"amount" => 0,
    b"number" => 1,
    b"target" => 2,
    b"origin" => 3,
    b"region" => 4,
    b"status" => 5,
    b"weight" => 6,
    b"height" => 7,
    b"length" => 8,
    b"source" => 9,
    b"parent" => 10,
    b"street" => 11,
    b"record" => 12,
    b"column" => 13,
    b"buffer" => 14,
    b"cursor" => 15,
);

/// Linear scan of the names, `length` + `memcmp` per candidate — the C++
/// `findmember` row and the shape a large `match` decays into.
struct Linear {
    keys: Vec<&'static [u8]>,
}

impl Lookup for Linear {
    fn build(keys: &[&'static [u8]]) -> Self {
        Self {
            keys: keys.to_vec(),
        }
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        self.keys.iter().position(|known| *known == key)
    }

    fn storage(&self) -> usize {
        self.keys.len() * std::mem::size_of::<&[u8]>()
    }
}

// ---------------------------------------------------------------- packed keys

/// Two words holding up to 15 key bytes plus the length — the C# `GetLongData`
/// packing, ported from the C++ device so the bit layout is identical.
#[derive(Clone, Copy, Default)]
struct LongData {
    head: u64,
    tail: u64,
}

fn load_word(source: &[u8], offset: usize, size: usize) -> u64 {
    let mut word = [0u8; 8];
    word[..size].copy_from_slice(&source[offset..offset + size]);
    u64::from_le_bytes(word)
}

fn pack(key: &[u8]) -> LongData {
    let length = key.len();
    let tail = if length & 8 == 0 {
        0
    } else {
        load_word(key, length & 7, 8)
    };
    let mut head = if length & 4 == 0 {
        0
    } else {
        load_word(key, length & 3, 4)
    };
    if length & 2 != 0 {
        head = (head << 16) | load_word(key, length & 1, 2);
    }
    if length & 1 != 0 {
        head = (head << 8) | load_word(key, 0, 1);
    }
    LongData {
        head: (head << 8) | length as u64,
        tail,
    }
}

struct LongDataScheme {
    data: Vec<LongData>,
    fallback: Option<Chain>,
    packed: bool,
}

impl Lookup for LongDataScheme {
    fn build(keys: &[&'static [u8]]) -> Self {
        let packable = keys.len() <= 11 && keys.iter().all(|key| key.len() <= 15);
        if !packable {
            return Self {
                data: Vec::new(),
                fallback: Some(Chain::build(keys)),
                packed: false,
            };
        }
        Self {
            data: keys.iter().map(|key| pack(key)).collect(),
            fallback: None,
            packed: true,
        }
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        if !self.packed {
            return self.fallback.as_ref().and_then(|chain| chain.lookup(key));
        }
        if key.len() > 15 {
            return None;
        }
        let probe = pack(key);
        self.data
            .iter()
            .position(|slot| slot.head == probe.head && slot.tail == probe.tail)
    }

    fn storage(&self) -> usize {
        match &self.fallback {
            Some(chain) => chain.storage(),
            None => self.data.len() * std::mem::size_of::<LongData>(),
        }
    }
}

// ---------------------------------------------------------------- hashes

fn hash_fnv(key: &[u8]) -> u64 {
    let mut value = 14_695_981_039_346_656_037u64;
    for &byte in key {
        value ^= u64::from(byte);
        value = value.wrapping_mul(1_099_511_628_211);
    }
    value
}

fn hash33(key: &[u8]) -> u32 {
    let mut result = key.len() as u32;
    let mut index = 0;
    while index + 4 <= key.len() {
        let mut word = [0u8; 4];
        word.copy_from_slice(&key[index..index + 4]);
        result = result
            .wrapping_mul(33)
            .wrapping_add(u32::from_le_bytes(word));
        index += 4;
    }
    while index < key.len() {
        result = result.wrapping_mul(33).wrapping_add(u32::from(key[index]));
        index += 1;
    }
    result
}

fn mix(value: u64) -> u64 {
    let mut value = value;
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;
    value
}

// ---------------------------------------------------------------- chain

const PRIMES: [u32; 13] = [11, 17, 23, 37, 59, 89, 131, 197, 293, 431, 631, 919, 1327];

struct ChainSlot {
    source: &'static [u8],
    index: u32,
    next: u32,
    hash: u32,
}

struct Chain {
    slots: Vec<ChainSlot>,
    buckets: Vec<u32>,
}

impl Lookup for Chain {
    fn build(keys: &[&'static [u8]]) -> Self {
        let mut bucket_count = PRIMES[0];
        for prime in PRIMES {
            bucket_count = prime;
            if usize::try_from(prime).unwrap() >= keys.len() * 2 {
                break;
            }
        }
        let mut buckets = vec![u32::MAX; bucket_count as usize];
        let mut slots = Vec::with_capacity(keys.len());
        for (index, &key) in keys.iter().enumerate() {
            let hash = hash33(key);
            let bucket = (hash % bucket_count) as usize;
            slots.push(ChainSlot {
                source: key,
                index: index as u32,
                next: buckets[bucket],
                hash,
            });
            buckets[bucket] = index as u32;
        }
        Self { slots, buckets }
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        let hash = hash33(key);
        let mut cursor = self.buckets[(hash % self.buckets.len() as u32) as usize];
        while cursor != u32::MAX {
            let slot = &self.slots[cursor as usize];
            if slot.hash == hash && slot.source == key {
                return Some(slot.index as usize);
            }
            cursor = slot.next;
        }
        None
    }

    fn storage(&self) -> usize {
        self.slots.len() * std::mem::size_of::<ChainSlot>() + self.buckets.len() * 4
    }
}

// ---------------------------------------------------------------- open addressing

#[derive(Clone, Copy, Default)]
struct OpenSlot {
    hash: u64,
    source: &'static [u8],
    index: u32,
    used: bool,
}

struct Open {
    slots: Vec<OpenSlot>,
    mask: usize,
}

fn capacity_for(count: usize) -> usize {
    let mut capacity = 16usize;
    while capacity < count * 2 {
        capacity <<= 1;
    }
    capacity
}

impl Lookup for Open {
    fn build(keys: &[&'static [u8]]) -> Self {
        let capacity = capacity_for(keys.len());
        let mask = capacity - 1;
        let mut slots = vec![OpenSlot::default(); capacity];
        for (index, &key) in keys.iter().enumerate() {
            let hash = hash_fnv(key);
            let mut cursor = (hash as usize) & mask;
            while slots[cursor].used {
                cursor = (cursor + 1) & mask;
            }
            slots[cursor] = OpenSlot {
                hash,
                source: key,
                index: index as u32,
                used: true,
            };
        }
        Self { slots, mask }
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        let hash = hash_fnv(key);
        let mut cursor = (hash as usize) & self.mask;
        while self.slots[cursor].used {
            let slot = &self.slots[cursor];
            if slot.hash == hash && slot.source == key {
                return Some(slot.index as usize);
            }
            cursor = (cursor + 1) & self.mask;
        }
        None
    }

    fn storage(&self) -> usize {
        self.slots.len() * std::mem::size_of::<OpenSlot>()
    }
}

// ---------------------------------------------------------------- perfect

/// Run-time seed search, ported from the C++ device (including the second FNV
/// round over the key bytes, which is what the C# `PerfectHash` does).
struct Perfect {
    slots: Vec<OpenSlot>,
    mask: usize,
    seed: u64,
    usable: bool,
}

impl Lookup for Perfect {
    fn build(keys: &[&'static [u8]]) -> Self {
        let mut result = Self {
            slots: Vec::new(),
            mask: 0,
            seed: 0,
            usable: false,
        };
        if keys.is_empty() {
            return result;
        }
        let capacity = capacity_for(keys.len());
        let mask = capacity - 1;
        let mut candidate = vec![OpenSlot::default(); capacity];
        for attempt in 1..=100_000u64 {
            candidate
                .iter_mut()
                .for_each(|slot| *slot = OpenSlot::default());
            let mut collided = false;
            for (index, &key) in keys.iter().enumerate() {
                let mut hash = mix(hash_fnv(key) ^ attempt);
                for &byte in key {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(1_099_511_628_211);
                }
                let cursor = (hash as usize) & mask;
                if candidate[cursor].used {
                    collided = true;
                    break;
                }
                candidate[cursor] = OpenSlot {
                    hash,
                    source: key,
                    index: index as u32,
                    used: true,
                };
            }
            if !collided {
                result.slots = std::mem::take(&mut candidate);
                result.mask = mask;
                result.seed = attempt;
                result.usable = true;
                return result;
            }
        }
        result
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        if !self.usable {
            return None;
        }
        let mut hash = mix(hash_fnv(key) ^ self.seed);
        for &byte in key {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        let slot = &self.slots[(hash as usize) & self.mask];
        if slot.used && slot.hash == hash && slot.source == key {
            return Some(slot.index as usize);
        }
        None
    }

    fn storage(&self) -> usize {
        self.slots.len() * std::mem::size_of::<OpenSlot>()
    }

    fn usable(&self) -> bool {
        self.usable
    }
}

// ---------------------------------------------------------------- sorted

struct Sorted {
    items: Vec<(&'static [u8], u32)>,
}

impl Lookup for Sorted {
    fn build(keys: &[&'static [u8]]) -> Self {
        let mut items: Vec<(&'static [u8], u32)> = keys
            .iter()
            .enumerate()
            .map(|(index, &key)| (key, index as u32))
            .collect();
        items.sort_unstable_by(|a, b| a.0.cmp(b.0));
        Self { items }
    }

    fn lookup(&self, key: &[u8]) -> Option<usize> {
        match self.items.binary_search_by(|slot| slot.0.cmp(key)) {
            Ok(index) => Some(self.items[index].1 as usize),
            Err(_) => None,
        }
    }

    fn storage(&self) -> usize {
        self.items.len() * std::mem::size_of::<(&[u8], u32)>()
    }
}

// ---------------------------------------------------------------- measurement

struct Row {
    ns: f64,
    bytes: usize,
    build_ns: f64,
}

fn measure_scheme<S: Lookup>(case: &Case, sequence: &[&'static [u8]]) -> Option<Row> {
    let start = Instant::now();
    let scheme = S::build(&case.keys);
    let build_ns = start.elapsed().as_secs_f64() * 1e9;
    if !scheme.usable() {
        return None;
    }
    for (index, key) in case.keys.iter().enumerate() {
        assert_eq!(
            scheme.lookup(key),
            Some(index),
            "{} resolved a key to the wrong index",
            std::any::type_name::<S>()
        );
    }
    let bytes = scheme.storage();
    let len = sequence.len();
    let mut cursor = 0usize;
    let ns = measure(|| {
        let key = black_box(sequence[cursor]);
        cursor += 1;
        if cursor == len {
            cursor = 0;
        }
        scheme.lookup(key)
    });
    Some(Row {
        ns,
        bytes,
        build_ns,
    })
}

fn match_row(case: &Case, sequence: &[&'static [u8]]) -> Option<Row> {
    match case.id {
        "real4" => measure_scheme::<MatchReal4>(case, sequence),
        "real8" => measure_scheme::<MatchReal8>(case, sequence),
        "real16" => measure_scheme::<MatchReal16>(case, sequence),
        "real32" => measure_scheme::<MatchReal32>(case, sequence),
        "flat16" => measure_scheme::<MatchFlat16>(case, sequence),
        _ => None,
    }
}

const NAMES: [&str; 7] = [
    "match", "linear", "longdata", "chain", "open", "perfect", "sorted",
];

fn rows_for(case: &Case, sequence: &[&'static [u8]]) -> Vec<Option<Row>> {
    vec![
        match_row(case, sequence),
        measure_scheme::<Linear>(case, sequence),
        measure_scheme::<LongDataScheme>(case, sequence),
        measure_scheme::<Chain>(case, sequence),
        measure_scheme::<Open>(case, sequence),
        measure_scheme::<Perfect>(case, sequence),
        measure_scheme::<Sorted>(case, sequence),
    ]
}

fn make_sequence(case: &Case, shuffled: bool, with_miss: bool) -> Vec<&'static [u8]> {
    let mut items = case.keys.clone();
    if items.len() < 64 {
        let filled: Vec<&'static [u8]> = (0..64).map(|i| items[i % items.len()]).collect();
        items = filled;
    }
    if shuffled {
        let mut state = 0x0123_4567_89AB_CDEFu64;
        for i in (1..items.len()).rev() {
            let j = (next_random(&mut state) % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
    if with_miss {
        let unknown = make_unknown(&case.keys, case.query_len, 777 + case.id.len() as u64);
        let middle = items.len() / 2;
        items[middle] = unknown;
    }
    items
}

fn summary(cases: &[Case]) {
    let mut rows: Vec<Vec<Option<Row>>> = Vec::with_capacity(cases.len());
    for case in cases {
        let sequence = make_sequence(case, false, false);
        rows.push(rows_for(case, &sequence));
    }

    println!("-- seq, hit — ns/lookup (min of 7 x 50 ms) --");
    print!("{:<9}", "case");
    for name in NAMES {
        print!("{name:>9}");
    }
    println!("{:>11}", "match/best");
    for (case, row) in cases.iter().zip(&rows) {
        print!("{:<9}", case.id);
        for cell in row {
            match cell {
                Some(Row { ns, .. }) => print!("{ns:>9.2}"),
                None => print!("{:>9}", "-"),
            }
        }
        let best = row
            .iter()
            .skip(1)
            .filter_map(|cell| cell.as_ref().map(|row| row.ns))
            .fold(f64::INFINITY, f64::min);
        match (&row[0], best.is_finite()) {
            (Some(current), true) => println!("{:>11.2}", current.ns / best),
            _ => println!("{:>11}", "-"),
        }
    }

    println!();
    println!("-- table bytes (0 = code only; built at run time, not const) --");
    print!("{:<9}", "case");
    for name in NAMES {
        print!("{name:>9}");
    }
    println!();
    for (case, row) in cases.iter().zip(&rows) {
        print!("{:<9}", case.id);
        for cell in row {
            match cell {
                Some(Row { bytes, .. }) => print!("{bytes:>9}"),
                None => print!("{:>9}", "-"),
            }
        }
        println!();
    }
}

fn full(cases: &[Case]) {
    println!("impl|case|order|mode|ns|bytes|build_ns");
    for case in cases {
        for (order, shuffled) in [("seq", false), ("rand", true)] {
            for (mode, with_miss) in [("hit", false), ("miss", true)] {
                let sequence = make_sequence(case, shuffled, with_miss);
                for (name, cell) in NAMES.iter().zip(rows_for(case, &sequence)) {
                    match cell {
                        Some(row) => println!(
                            "{name}|{}|{order}|{mode}|{:.3}|{}|{:.0}",
                            case.id, row.ns, row.bytes, row.build_ns
                        ),
                        None => println!("{name}|{}|{order}|{mode}|na", case.id),
                    }
                }
            }
        }
    }
}

pub fn run(full_matrix: bool) {
    header("named lookup: match vs table schemes (Rust)");
    let mut cases = make_cases();
    if full_matrix {
        full(&cases);
        return;
    }
    // Summary keeps the cases that decide the question: the literal schemas
    // (where `match` exists), the packed/chain boundary (11 / 12), the length
    // boundary (15 / 16) and one large table where linear growth shows up.
    cases.retain(|case| {
        matches!(
            case.id,
            "n4" | "n11"
                | "n12"
                | "n16"
                | "n32"
                | "n64"
                | "len15"
                | "len16"
                | "real4"
                | "real8"
                | "real16"
                | "real32"
                | "flat16"
        )
    });
    summary(&cases);
}
