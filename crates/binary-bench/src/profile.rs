//! Targeted micro-benchmarks for attributing the C++ vs Rust gap (DESIGN §19.15).
//!
//! Two rules keep these rows honest:
//!
//! - every input is fed through `black_box` and every closure ends with
//!   `black_box(encoder.as_bytes())`. Without the second one LLVM deletes all
//!   stores to a buffer nobody reads, and without the first one it folds the
//!   whole loop into a constant `len()` (the first version of this file reported
//!   a suspicious 0.42 / 1.05 ns for every row — that was constant folding);
//! - each row does the operation 16 times so the value sits well above the
//!   ~1 ns measurement floor.
//!
//! The `flat` rows are a lower bound: one bounds check + one `copy_from_slice`
//! into `&mut [u8]`, no enum, no `Result`, no sub-encoder.
//!
//! All payloads are native `u8` strings on both sides (Rust `String` / C++
//! `std::string`), so no UTF-16 transcoding enters the comparison.

use std::hint::black_box;

use binary::{write_named_member, Decode, Decoder, Encode, Encoder, NamedMembers};

use crate::harness::{allocs, bench, bench_ret, header};
use crate::wire::{named_value, tuple_value, Type01, TypeX1};

const REPEAT: usize = 16;

/// One bounds check + one `copy_from_slice`, no enum, no `Result`, no sub-encoder.
#[inline]
fn flat_many(buf: &mut [u8], pos: &mut usize, src: &[u8], n: usize) {
    for _ in 0..n {
        let end = *pos + src.len();
        if end > buf.len() {
            return;
        }
        buf[*pos..end].copy_from_slice(src);
        *pos = end;
    }
}

#[inline]
fn write_many(e: &mut Encoder<'_>, src: &[u8], n: usize) {
    for _ in 0..n {
        e.write(src).expect("1024 bytes is enough");
    }
}

/// Records per row in the `FinishAnchor` A/B below.
const RECORDS: usize = 16;

/// One `FinishAnchor`-shaped record on a flat cursor, matching the three
/// variants of the C++ microbenchmark (DESIGN §19.15.7):
///
/// - `VARIANT = 0`: 4-byte wide header, no payload move (the "old" form);
/// - `VARIANT = 1`: 1-byte header + two unaligned 8-byte chunk moves (C#/C++);
/// - `VARIANT = 2`: 1-byte header + exact-length `copy_within` (memmove shape).
///
/// `STRIDE` separates records so an 8-byte chunk write cannot land in the next
/// record's header; `WRITTEN` stores the payload immediately before the move,
/// which is the real converter order and is what triggers store-to-load
/// forwarding. `FENCE` drains part of the store buffer between the two.
/// Everything is a const generic so each row compiles to one straight-line
/// shape with no runtime dispatch.
#[inline]
fn anchor_record<
    const N: usize,
    const STRIDE: usize,
    const VARIANT: u8,
    const WRITTEN: bool,
    const FENCE: bool,
>(
    buf: &mut [u8],
    i: usize,
    src: &[u8; N],
) {
    let base = i * STRIDE;
    if WRITTEN {
        buf[base + 4..base + 4 + N].copy_from_slice(src);
    }
    if FENCE {
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    }
    if N > 16 || VARIANT == 0 {
        // >16 keeps the 4-byte header in every variant (the 50 B control row).
        buf[base..base + 4].copy_from_slice(&[0x80, 0, 0, N as u8]);
        return;
    }
    buf[base] = N as u8;
    if VARIANT == 1 {
        // Load the whole word before storing: the two 8-byte blocks overlap
        // their own destination, and the bytes past the payload are scratch
        // inside this record (never part of the reported result).
        let mut chunk = [0u8; 8];
        chunk.copy_from_slice(&buf[base + 4..base + 12]);
        buf[base + 1..base + 9].copy_from_slice(&chunk);
        if N > 8 {
            chunk.copy_from_slice(&buf[base + 12..base + 20]);
            buf[base + 9..base + 17].copy_from_slice(&chunk);
        }
    } else {
        buf.copy_within(base + 4..base + 4 + N, base + 1);
    }
}

pub fn run() {
    header("profile: attributing the C++ vs Rust gap (native u8 strings)");

    let mut buf = vec![0u8; 1024];
    // Opaque inputs: the compiler must not know the bytes or the lengths.
    let one = black_box([7u8]);
    let four = black_box([1u8, 2, 3, 4]);
    let eight = black_box([0u8; 8]);
    let payload = black_box(*b"csharp");
    let name_id = black_box(*b"Id");
    let strings = String::from("csharp");
    let vec_payload = black_box(b"csharp".to_vec());

    println!("-- per write: library vs flat cursor ({REPEAT} writes) --");
    bench("flat write 1B", || {
        let src = black_box(one);
        let mut pos = 0;
        flat_many(&mut buf, &mut pos, &src, REPEAT);
        black_box(&buf[..pos]);
    });
    bench("flat write 4B", || {
        let src = black_box(four);
        let mut pos = 0;
        flat_many(&mut buf, &mut pos, &src, REPEAT);
        black_box(&buf[..pos]);
    });
    // `black_box` is hoisted out of the loop like every other row. Inside it,
    // the box is a store plus a reload per iteration, and the reload waits on
    // store-to-load forwarding: the row read 22 ns against 2.2 ns for
    // `write 1B`, which is the harness, not `write_byte` (DESIGN 20.15.18).
    bench("Encoder::write_byte (borrowed)", || {
        let byte = black_box(7u8);
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            e.write_byte(byte).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("Encoder::write 1B (borrowed)", || {
        let src = black_box(one);
        let mut e = Encoder::on_slice(&mut buf);
        write_many(&mut e, &src, REPEAT);
        black_box(e.as_bytes());
    });
    bench("Encoder::write 4B (borrowed)", || {
        let src = black_box(four);
        let mut e = Encoder::on_slice(&mut buf);
        write_many(&mut e, &src, REPEAT);
        black_box(e.as_bytes());
    });
    bench("Encoder::write 8B (borrowed)", || {
        let src = black_box(eight);
        let mut e = Encoder::on_slice(&mut buf);
        write_many(&mut e, &src, REPEAT);
        black_box(e.as_bytes());
    });
    bench("Encoder::write 4B (owned)", || {
        let src = black_box(four);
        let mut e = Encoder::with_capacity(1024);
        write_many(&mut e, &src, REPEAT);
        black_box(e.as_bytes());
    });
    // Backend chosen through an opaque value: LLVM can no longer fold the
    // `Inner::Owned | Inner::Borrowed` discriminant away. The delta to the
    // borrowed row above is the *actual* cost of the enum, if any.
    bench("Encoder::write 4B (runtime backend)", || {
        let src = black_box(four);
        let mut e = if black_box(false) {
            Encoder::with_capacity(0)
        } else {
            Encoder::on_slice(&mut buf)
        };
        write_many(&mut e, &src, REPEAT);
        black_box(e.as_bytes());
    });
    allocs("Encoder::write 4B x16 (owned)", || {
        let mut e = Encoder::with_capacity(1024);
        write_many(&mut e, &four, REPEAT);
        black_box(e.as_bytes());
    });
    println!();

    println!("-- one length prefix + 6 ASCII bytes, x{REPEAT} --");
    bench("flat: 2 writes (prefix, payload)", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            flat_many(&mut buf, &mut pos, &[black_box(6u8)], 1);
            flat_many(&mut buf, &mut pos, &payload, 1);
        }
        black_box(&buf[..pos]);
    });
    bench("Encoder: 2 writes (prefix, payload)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            e.write(&[black_box(6u8)]).expect("fits");
            e.write(&payload).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("write_prefixed_bounded(6, write 6B)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            e.write_prefixed_bounded(black_box(6), |sub| sub.write(&payload))
                .expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("write_prefixed(write 6B)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            let p = black_box(payload);
            e.write_prefixed(|sub| sub.write(&p)).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("String::encode_with_prefix", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            black_box(&strings)
                .encode_with_prefix(&mut e)
                .expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("Vec<u8>::encode_with_prefix", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            black_box(&vec_payload)
                .encode_with_prefix(&mut e)
                .expect("fits");
        }
        black_box(e.as_bytes());
    });
    println!();

    // The `write_prefixed` path is the only prefix path that has to move payload bytes.
    // A 20-byte payload takes the 4-byte header branch (no memmove), so it
    // isolates the cost of the runtime-length `copy_within` on the short path.
    // The flat cursors produce the same wire with none of the encoder machinery.
    // An empty-payload row is deliberately absent: fat LTO folds the whole loop
    // into a constant 1-byte store (it measured 1.25 ns, identical to a single
    // flat write), so it cannot be used for attribution.
    println!("-- prefix path anatomy: where compaction cost comes from (x{REPEAT}) --");
    let long = black_box(*b"01234567890123456789");
    bench("write_prefixed(20B, wide header)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            let p = black_box(long);
            e.write_prefixed(|sub| sub.write(&p)).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("write_prefixed_bounded(20, 20B payload)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            e.write_prefixed_bounded(black_box(20), |sub| sub.write(&long))
                .expect("fits");
        }
        black_box(e.as_bytes());
    });
    // Lower bound for the *current* strategy: 4-byte slot, payload written at
    // +4, then moved to +1. Same wire, none of the encoder machinery.
    bench("flat: 4B slot + 6B + memmove", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            let s = black_box(payload);
            buf[pos..pos + 4].fill(0);
            buf[pos + 4..pos + 4 + s.len()].copy_from_slice(&s);
            buf[pos] = s.len() as u8;
            buf.copy_within(pos + 4..pos + 4 + s.len(), pos + 1);
            pos += 1 + s.len();
        }
        black_box(&buf[..pos]);
    });
    // Lower bound for a 1-byte slot strategy: no memmove on the short path.
    // Only valid if the writer can grow the header in place for long payloads.
    bench("flat: 1B slot + 6B + backfill", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            let s = black_box(payload);
            buf[pos] = 0;
            buf[pos + 1..pos + 1 + s.len()].copy_from_slice(&s);
            buf[pos] = s.len() as u8;
            pos += 1 + s.len();
        }
        black_box(&buf[..pos]);
    });
    // `write_prefixed` knows `actual <= 16` only as a runtime value, so its
    // `copy_within` cannot be turned into a fixed-size move and degrades to a
    // general memmove call. Making just the length opaque reproduces that shape
    // on a flat cursor — the delta to the row above is the price of the call.
    // A "fixed 8-byte move" row was tried and removed: writing two bytes past
    // the reported `pos` creates a cross-iteration dependency that LLVM cannot
    // hoist, so it measures loop scheduling rather than the move itself.
    bench("flat: 4B slot + 6B + opaque len", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            let s = black_box(payload);
            let n = black_box(s.len());
            buf[pos..pos + 4].fill(0);
            buf[pos + 4..pos + 4 + n].copy_from_slice(&s[..n]);
            buf[pos] = n as u8;
            buf.copy_within(pos + 4..pos + 4 + n, pos + 1);
            pos += 1 + n;
        }
        black_box(&buf[..pos]);
    });
    // Rejected candidate, kept as a control for the no-slack alternative:
    // compile-time-sized chunks (8/8/4/2/1) selected by the opaque length.
    // Every `copy_within` has a constant length and the source never reaches
    // past the payload. The library uses the C#/C++ `u64` shape instead; see
    // DESIGN §19.15.7.
    bench("flat: 4B slot + 6B + stepped fixed moves", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            let s = black_box(payload);
            let n = black_box(s.len());
            buf[pos..pos + 4].fill(0);
            buf[pos + 4..pos + 4 + s.len()].copy_from_slice(&s);
            buf[pos] = s.len() as u8;
            // Two 8-byte blocks cover 8 and 16; the low three bits of the
            // remainder select 4 / 2 / 1. All lengths are compile-time
            // constants and every source range stays inside the payload.
            let mut k = 0;
            if n >= 8 {
                buf.copy_within(pos + 4 + k..pos + 4 + k + 8, pos + 1 + k);
                k += 8;
            }
            if n >= 16 {
                buf.copy_within(pos + 4 + k..pos + 4 + k + 8, pos + 1 + k);
                k += 8;
            }
            let rem = n - k;
            if rem & 4 != 0 {
                buf.copy_within(pos + 4 + k..pos + 4 + k + 4, pos + 1 + k);
                k += 4;
            }
            if rem & 2 != 0 {
                buf.copy_within(pos + 4 + k..pos + 4 + k + 2, pos + 1 + k);
                k += 2;
            }
            if rem & 1 != 0 {
                buf.copy_within(pos + 4 + k..pos + 4 + k + 1, pos + 1 + k);
            }
            pos += 1 + s.len();
        }
        black_box(&buf[..pos]);
    });
    // The C#/C++ shape always writes a whole 8-byte word, so for a 6-byte
    // payload two bytes land past the reported result. In a tight x16 loop the
    // next iteration writes over those bytes, which LLVM cannot hoist. This
    // flat row reproduces that aliasing without any encoder machinery.
    bench("flat: 4B slot + 6B + u64 8B write", || {
        let mut pos = 0;
        for _ in 0..REPEAT {
            let s = black_box(payload);
            buf[pos..pos + 4].fill(0);
            buf[pos + 4..pos + 4 + s.len()].copy_from_slice(&s);
            buf[pos] = s.len() as u8;
            let mut chunk = [0u8; 8];
            chunk.copy_from_slice(&buf[pos + 4..pos + 12]);
            buf[pos + 1..pos + 9].copy_from_slice(&chunk);
            pos += 1 + s.len();
        }
        black_box(&buf[..pos]);
    });
    println!();

    // FinishAnchor A/B, mirroring the C++ microbenchmark's control design
    // (DESIGN §19.15.7). One shared buffer, records either 64 B apart or
    // packed back-to-back; payload either stored right before the move or
    // left cold. That separates two effects the x16 rows above conflate:
    //   cold  packed vs cold  spaced -> 8-byte write hitting the next header
    //   cold  spaced vs written spaced -> store-to-load forwarding
    // 64 B is the C++ 32 B design widened to also cover the 50 B control.
    println!("-- FinishAnchor A/B: spacing x forwarding (x{RECORDS}) --");
    let mut spaced = vec![0x5Au8; 2048];
    macro_rules! anchor_row {
        ($label:expr, $n:literal, $stride:literal, $v:literal, $w:literal, $f:literal) => {
            bench($label, || {
                let src = black_box([0x5Au8; $n]);
                for i in 0..RECORDS {
                    anchor_record::<$n, $stride, $v, $w, $f>(&mut spaced, i, &src);
                }
                black_box(&spaced[..]);
            });
        };
    }
    anchor_row!("spaced cold | old 6B", 6, 64, 0, false, false);
    anchor_row!("spaced cold | u64 6B", 6, 64, 1, false, false);
    anchor_row!("spaced cold | memmove 6B", 6, 64, 2, false, false);
    anchor_row!("spaced written | old 6B", 6, 64, 0, true, false);
    anchor_row!("spaced written | u64 6B", 6, 64, 1, true, false);
    anchor_row!("spaced written | memmove 6B", 6, 64, 2, true, false);
    anchor_row!("spaced written | u64 6B +fence", 6, 64, 1, true, true);
    // Fence-only controls: how much the SeqCst fence itself costs, with no
    // forwarding (old) or with a cold payload (u64). Needed to tell whether
    // the u64 +fence row is "no relief" or "relief exactly cancelled by cost".
    anchor_row!("spaced written | old 6B +fence", 6, 64, 0, true, true);
    anchor_row!("spaced cold | u64 6B +fence", 6, 64, 1, false, true);
    anchor_row!("packed cold | u64 6B", 6, 7, 1, false, false);
    anchor_row!("packed written | u64 6B", 6, 7, 1, true, false);
    anchor_row!("packed cold | u64 10B", 10, 11, 1, false, false);
    anchor_row!("spaced cold | u64 10B", 10, 64, 1, false, false);
    anchor_row!("spaced written | u64 10B", 10, 64, 1, true, false);
    println!();

    // Length sweep over the whole compaction range. Both variants are spaced
    // and written, i.e. the realistic "payload just stored" path. This pins
    // down the exact byte counts where the exact-length move wins, and where
    // it degenerates to the u64 shape's cost.
    println!("-- FinishAnchor A/B: length sweep 1..=16 (spaced, written) --");
    macro_rules! sweep_pair {
        ($n:literal) => {
            anchor_row!(
                concat!("sweep ", stringify!($n), "B | u64"),
                $n,
                64,
                1,
                true,
                false
            );
            anchor_row!(
                concat!("sweep ", stringify!($n), "B | memmove"),
                $n,
                64,
                2,
                true,
                false
            );
        };
    }
    sweep_pair!(1);
    sweep_pair!(2);
    sweep_pair!(3);
    sweep_pair!(4);
    sweep_pair!(5);
    sweep_pair!(6);
    sweep_pair!(7);
    sweep_pair!(8);
    sweep_pair!(9);
    sweep_pair!(10);
    sweep_pair!(11);
    sweep_pair!(12);
    sweep_pair!(13);
    sweep_pair!(14);
    sweep_pair!(15);
    sweep_pair!(16);
    anchor_row!("spaced written | old 50B", 50, 64, 0, true, false);
    anchor_row!("spaced written | u64 50B", 50, 64, 1, true, false);
    anchor_row!("spaced written | memmove 50B", 50, 64, 2, true, false);
    println!();

    println!("-- one named member (Id = 1024), x{REPEAT} --");
    let id_member_wire = black_box([2u8, b'I', b'd', 4, 0, 4, 0, 0]);
    bench("flat: 8 bytes in one write", || {
        let src = black_box(id_member_wire);
        let mut pos = 0;
        flat_many(&mut buf, &mut pos, &src, REPEAT);
        black_box(&buf[..pos]);
    });
    bench("write_named_member(Id, i32)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            let n = black_box(name_id);
            write_named_member(&mut e, &n, &black_box(1024i32)).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("manual: 4 writes", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            e.write(&[black_box(2u8)]).expect("fits");
            e.write(&name_id).expect("fits");
            e.write(&[black_box(4u8)]).expect("fits");
            e.write(&black_box(1024i32).to_le_bytes()).expect("fits");
        }
        black_box(e.as_bytes());
    });
    bench("write_named_member(Name, String)", || {
        let mut e = Encoder::on_slice(&mut buf);
        for _ in 0..REPEAT {
            write_named_member(&mut e, black_box(&b"Name"[..]), black_box(&strings)).expect("fits");
        }
        black_box(e.as_bytes());
    });
    println!();

    println!("-- decode: copy vs validation vs allocation (1 KiB ASCII) --");
    let mut e = Encoder::new();
    let big = "x".repeat(1024);
    big.encode_auto(&mut e).expect("fits");
    let big_bytes = e.finish().expect("owned");
    // Strip the outer auto prefix so each row starts at the same payload.
    let mut d = Decoder::new(&big_bytes);
    let big_payload = d.read_prefix().expect("prefix");

    bench_ret("decode &[u8] 1KiB (zero copy)", || {
        let mut d = Decoder::new(black_box(big_payload));
        <&[u8] as Decode>::decode(&mut d).expect("valid")
    });
    bench_ret("decode &str 1KiB (zero copy + UTF-8)", || {
        let mut d = Decoder::new(black_box(big_payload));
        <&str as Decode>::decode(&mut d).expect("valid")
    });
    bench_ret("decode String 1KiB (alloc + UTF-8)", || {
        let mut d = Decoder::new(black_box(big_payload));
        <String as Decode>::decode(&mut d).expect("valid")
    });
    bench_ret("decode Vec<u8> 1KiB (bulk copy)", || {
        let mut d = Decoder::new(black_box(big_payload));
        <Vec<u8> as Decode>::decode(&mut d).expect("valid")
    });
    println!();

    println!("-- allocations on one decode --");
    let tuple = tuple_value();
    let mut scratch2 = vec![0u8; 1024];
    let tuple_raw = {
        let mut e = Encoder::on_slice(&mut scratch2);
        tuple.encode(&mut e).expect("fits");
        e.as_bytes().to_vec()
    };
    assert_eq!(
        tuple_raw.len(),
        50,
        "raw tuple wire must match C++ byte for byte"
    );
    allocs("decode tuple (raw, full)", || {
        let mut d = Decoder::new(&tuple_raw);
        black_box(<TypeX1 as Decode>::decode(&mut d).expect("valid"));
    });
    allocs("decode String 1KiB", || {
        let mut d = Decoder::new(big_payload);
        black_box(<String as Decode>::decode(&mut d).expect("valid"));
    });
    let strings8: Vec<String> = (0..8).map(|i| format!("s{i}")).collect();
    let mut e = Encoder::new();
    strings8.encode_auto(&mut e).expect("fits");
    let strings8_bytes = e.finish().expect("owned");
    allocs("decode Vec<String> x8", || {
        let mut d = Decoder::new(&strings8_bytes);
        black_box(<Vec<String> as Decode>::decode_auto(&mut d).expect("valid"));
    });
    allocs("decode Vec<&str> x8 (zero copy)", || {
        let mut d = Decoder::new(&strings8_bytes);
        black_box(<Vec<&str> as Decode>::decode_auto(&mut d).expect("valid"));
    });
    bench_ret("decode Vec<&str> x8 (auto)", || {
        let mut d = Decoder::new(black_box(&strings8_bytes));
        <Vec<&str> as Decode>::decode_auto(&mut d).expect("valid")
    });
    bench_ret("decode Vec<String> x8 (auto)", || {
        let mut d = Decoder::new(black_box(&strings8_bytes));
        <Vec<String> as Decode>::decode_auto(&mut d).expect("valid")
    });
    let strings64: Vec<String> = (0..64).map(|i| format!("s{i}")).collect();
    let mut e = Encoder::new();
    strings64.encode_auto(&mut e).expect("fits");
    let strings64_bytes = e.finish().expect("owned");
    allocs("decode Vec<String> x64", || {
        let mut d = Decoder::new(&strings64_bytes);
        black_box(<Vec<String> as Decode>::decode_auto(&mut d).expect("valid"));
    });
    bench_ret("decode Vec<String> x64 (auto)", || {
        let mut d = Decoder::new(black_box(&strings64_bytes));
        <Vec<String> as Decode>::decode_auto(&mut d).expect("valid")
    });
    bench_ret("decode Vec<&str> x64 (auto, zero copy)", || {
        let mut d = Decoder::new(black_box(&strings64_bytes));
        <Vec<&str> as Decode>::decode_auto(&mut d).expect("valid")
    });
    println!();

    println!("-- named decode: parse prefixes only vs full value decode --");
    let named = named_value();
    let mut scratch = vec![0u8; 1024];
    let named_raw = {
        let mut e = Encoder::on_slice(&mut scratch);
        named.encode(&mut e).expect("fits");
        e.as_bytes().to_vec()
    };
    assert_eq!(
        named_raw.len(),
        85,
        "raw named wire must match C++ byte for byte"
    );
    bench_ret("NamedMembers::parse (skip all)", || {
        let mut d = Decoder::new(black_box(&named_raw));
        NamedMembers::<'_, 4>::parse(&mut d, |_| None).expect("valid")
    });
    allocs("NamedMembers::parse (skip all)", || {
        let mut d = Decoder::new(&named_raw);
        black_box(NamedMembers::<'_, 4>::parse(&mut d, |_| None).expect("valid"));
    });
    bench_ret("decode named (raw, full)", || {
        let mut d = Decoder::new(black_box(&named_raw));
        <Type01 as Decode>::decode(&mut d).expect("valid")
    });
    allocs("decode named (raw, full)", || {
        let mut d = Decoder::new(&named_raw);
        black_box(<Type01 as Decode>::decode(&mut d).expect("valid"));
    });
}
