//! Collection group: the bulk path against the per-element one, 1024 elements.
//!
//! These rows have no counterpart in the other two repositories — C# measures
//! collections in its own `Benchmarks.CollectionTests` suite over different
//! payloads, and the C++ integration suite has only the two objects. What they
//! answer is internal: what `i32`'s `encode_slice` override and the POD bulk
//! decode buy, and what keeping `bool` off both paths costs (§19.10).
//!
//! Buffers and measurement rules are the ones from `wire`: one borrowed buffer
//! reused across iterations, `min` over 7 rounds of 50 ms, `black_box` on every
//! result.

use binary::{Decode, Decoder, Encode, Encoder, Layout};

use crate::harness::{bench, bench_ret, header};

/// `i32` with the bulk hooks left at their defaults, i.e. the per-element
/// encode path — and exactly what a type from another crate looks like.
///
/// Exists only to measure that path against the bulk one on the same 4-byte
/// payload: `Vec<LoopI32>` and `Vec<i32>` differ in nothing but the
/// `encode_slice` override, so the gap between the two rows is the value of the
/// fast path.
#[derive(Clone, Copy)]
struct LoopI32(i32);

impl Layout for LoopI32 {
    const LEN: usize = 4;
}

impl Encode for LoopI32 {
    fn encode(&self, e: &mut Encoder<'_>) -> binary::Result<()> {
        e.write(&self.0.to_le_bytes())
    }
}

/// Encode into the caller's buffer through the natural entry point, returning
/// the byte length. On a container `auto` adds the 4-byte prefix and nothing
/// else, which is the same shape as C# `EnumerableConverter` and C++
/// `ContainerConverter`.
fn encode_into<T: Encode>(value: &T, buf: &mut [u8]) -> usize {
    let mut e = Encoder::on_slice(buf);
    value.encode_auto(&mut e).expect("8 KiB is enough");
    e.len()
}

fn decode_from<'a, T: Decode<'a>>(bytes: &'a [u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode_auto(&mut d).expect("round-tripped above")
}

pub fn run() {
    header("collections: bulk vs per element (Rust)");

    // Container fast path: 1024 `i32` = 4096 payload bytes. The two encode rows
    // are the same bytes written two ways — bulk (`i32`'s `encode_slice`) vs per
    // element (`LoopI32`'s default) — so their difference is exactly what the
    // override buys. C++ decides the same question in `ContainerConverter`
    // (`copy` flag) and C# in `Benchmarks.CollectionTests`.
    let vec_pod: Vec<i32> = (0..1024i32).collect();
    let vec_loop: Vec<LoopI32> = (0..1024i32).map(LoopI32).collect();
    let mut vec_buf = vec![0u8; 8 * 1024];

    let vec_len = encode_into(&vec_pod, &mut vec_buf);
    let vec_bytes = vec_buf[..vec_len].to_vec();
    assert_eq!(vec_len, 4 + 4096, "prefix + 1024 * 4");
    let loop_len = encode_into(&vec_loop, &mut vec_buf);
    assert_eq!(vec_len, loop_len, "bulk and loop must share one wire");
    let round: Vec<i32> = decode_from(&vec_bytes);
    assert_eq!(round, vec_pod);
    println!("vec i32 x1024: {vec_len} bytes");
    println!();

    bench("encode vec<i32> x1024 (auto, bulk)", || {
        let n = encode_into(std::hint::black_box(&vec_pod), &mut vec_buf);
        std::hint::black_box(n);
    });

    bench("encode vec<loop> x1024 (auto, per element)", || {
        let n = encode_into(std::hint::black_box(&vec_loop), &mut vec_buf);
        std::hint::black_box(n);
    });

    bench_ret("decode vec<i32> x1024", || {
        decode_from::<Vec<i32>>(std::hint::black_box(&vec_bytes))
    });

    // `bool` overrides neither hook because its decoder is lenient; `u8` is the
    // byte-identical bulk control. The gap between the two rows is the price of
    // that exclusion, on each side.
    let vec_bool: Vec<bool> = (0..1024).map(|i| i % 3 == 0).collect();
    let vec_u8: Vec<u8> = vec_bool.iter().map(|&b| b as u8).collect();
    let bool_len = encode_into(&vec_bool, &mut vec_buf);
    let u8_len = encode_into(&vec_u8, &mut vec_buf);
    assert_eq!(bool_len, u8_len, "bool and u8 share one wire");
    let bool_bytes = vec_buf[..bool_len].to_vec();
    assert_eq!(decode_from::<Vec<bool>>(&bool_bytes), vec_bool);

    bench("encode vec<bool> x1024 (auto, per element)", || {
        let n = encode_into(std::hint::black_box(&vec_bool), &mut vec_buf);
        std::hint::black_box(n);
    });
    bench("encode vec<u8> x1024 (auto, bulk)", || {
        let n = encode_into(std::hint::black_box(&vec_u8), &mut vec_buf);
        std::hint::black_box(n);
    });
    bench_ret("decode vec<bool> x1024", || {
        decode_from::<Vec<bool>>(std::hint::black_box(&bool_bytes))
    });
    bench_ret("decode vec<u8> x1024", || {
        decode_from::<Vec<u8>>(std::hint::black_box(&bool_bytes))
    });
}
