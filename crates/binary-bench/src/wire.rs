//! Wire benchmarks on the payload the C# and C++ suites already use.
//!
//! `TypeX1` / `Type01` are copied verbatim from
//! - `binary-cxx/code/binary-benchmarks/integration-tests/src/Common.hpp`
//! - `binary/code/Benchmarks.IntegrationTests/IntegrationBenchmarks.cs`
//!
//! so the three binaries encode the *same bytes*, not merely comparable shapes.
//!
//! Only the four rows both sides actually measure live here: encode and decode
//! of the tuple and of the named object, **raw** form, i.e. C#
//! `Converter<T>::Encode(ref allocator, value)` / `Decode(bytes)` and C++
//! `Converter<T>::Encode(allocator, value)` / `Decode(span)`. `auto` is not
//! timed: a tuple's `auto` is its raw form, named's is raw plus one 4-byte
//! prefix, and the two rows sat within noise of each other (§19.7 / §19.15.2).
//!
//! Everything that used to share this group moved out, so that a diff against
//! the other two repositories stays one-to-one: the owned-`Vec` rows and the
//! `i32` baselines are in `objects`, the 1024-element container rows in
//! `collections`.
//!
//! Buffer handling mirrors C++: one 1024-byte buffer reused across iterations,
//! nothing allocated for the result. C# does the same with `new Allocator(buf)`.

use binary::{Decode, Decoder, Encode, Encoder};

use crate::harness::{bench, bench_ret, header};

/// `(double, List<string>)`
pub(crate) type TypeX2 = (f64, Vec<String>);
/// `(int, string, int[], (double, List<string>))`
pub(crate) type TypeX1 = (i32, String, Vec<i32>, TypeX2);

#[derive(Debug, Encode, Decode)]
pub(crate) struct Type02 {
    #[binary(rename = "Data")]
    data: f64,
    #[binary(rename = "Tags")]
    tags: Vec<String>,
}

#[derive(Debug, Encode, Decode)]
pub(crate) struct Type01 {
    #[binary(rename = "Id")]
    id: i32,
    #[binary(rename = "Name")]
    name: String,
    #[binary(rename = "List")]
    list: Vec<i32>,
    #[binary(rename = "Item")]
    item: Type02,
}

pub(crate) fn tuple_value() -> TypeX1 {
    (
        1024,
        String::from("csharp"),
        vec![7, 11, 555, 1313],
        (
            2.2,
            vec![
                String::from("one"),
                String::from("two"),
                String::from("three"),
            ],
        ),
    )
}

pub(crate) fn named_value() -> Type01 {
    Type01 {
        id: 1024,
        name: String::from("csharp"),
        list: vec![7, 11, 555, 1313],
        item: Type02 {
            data: 2.2,
            tags: vec![
                String::from("one"),
                String::from("two"),
                String::from("three"),
            ],
        },
    }
}

/// The *raw* form: no own length prefix.
///
/// This is what C# `converter.Encode(ref allocator, value)` and C++
/// `Converter<T>::Encode(...)` do, and what every row in this group measures.
/// `on_slice` is the borrowed backend: no growth, overflow returns
/// `Error::BufferFull` (the same contract as C++ `Allocator(span)`).
fn encode_raw_into<T: Encode>(value: &T, buf: &mut [u8]) -> usize {
    let mut e = Encoder::on_slice(buf);
    value.encode(&mut e).expect("1024 bytes is enough");
    e.len()
}

/// Decode the raw form (no leading prefix), matching C# `Decode` and C++
/// `Decode`.
fn decode_raw_from<'a, T: Decode<'a>>(bytes: &'a [u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode(&mut d).expect("round-tripped above")
}

pub fn run() {
    header("wire: Rust vs C# vs C++");

    let tuple = tuple_value();
    let named = named_value();
    let mut buf = vec![0u8; 1024];

    // Byte-for-byte what C# `Encode` and C++ `Encode` produce.
    let tuple_raw_len = encode_raw_into(&tuple, &mut buf);
    let tuple_raw = buf[..tuple_raw_len].to_vec();
    let named_raw_len = encode_raw_into(&named, &mut buf);
    let named_raw = buf[..named_raw_len].to_vec();

    println!(
        "payload  : tuple {} bytes; named {} bytes",
        tuple_raw_len, named_raw_len
    );
    println!("tuple : {}", hex(&tuple_raw));
    println!("named : {}", hex(&named_raw));
    println!();

    // Sanity: decode must give the value back, otherwise the numbers measure
    // nothing.
    let round: TypeX1 = decode_raw_from(&tuple_raw);
    assert_eq!(round, tuple);
    let round: Type01 = decode_raw_from(&named_raw);
    assert_eq!(round.name, named.name);

    bench("encode tuple (raw, on_slice)", || {
        let n = encode_raw_into(std::hint::black_box(&tuple), &mut buf);
        std::hint::black_box(n);
    });

    bench_ret("decode tuple (raw)", || {
        decode_raw_from::<TypeX1>(std::hint::black_box(&tuple_raw))
    });

    bench("encode named (raw, on_slice)", || {
        let n = encode_raw_into(std::hint::black_box(&named), &mut buf);
        std::hint::black_box(n);
    });

    bench_ret("decode named (raw)", || {
        decode_raw_from::<Type01>(std::hint::black_box(&named_raw))
    });
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
