//! Object-shape group: tuple / tuple object / named object, one row each.
//!
//! `wire` already covers the tuple and the named object. This group exists for
//! the C++ comparison in DESIGN §19.15.8, which needs the third shape as well:
//! C++ has a separate `TupleObjectConverter` (PFR struct, no names) next to its
//! `TupleConverter` (real `std::tuple`) and `NamedObjectConverter`, and Rust's
//! `#[binary(tuple)]` is the same thing as the first one.
//!
//! Both tuple shapes are byte-identical on the wire by construction (§11); the
//! group asserts that rather than trusting it, because a divergence would make
//! the encode comparison below meaningless. The named object writes member
//! names in declaration order, which is also what C++'s `NamedObjectConverter`
//! does, so the hex printed here can be diffed against the C++ binary's.
//!
//! Buffers and measurement rules are the ones from `wire`: one 1024-byte
//! borrowed buffer, `min` over 7 rounds of 50 ms, `black_box` on every result.
//! Rows are the raw form only — see `wire`'s module docs for why (§19.15.2).

use binary::{Decode, Decoder, Encode, Encoder};

use crate::harness::{bench, bench_ret, header};
use crate::wire::{hex, named_value, tuple_value, Type01, TypeX1};

/// `Type02` as a tuple object: same members, no names, no own header.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(tuple)]
pub(crate) struct Type02Tuple {
    data: f64,
    tags: Vec<String>,
}

/// `Type01` as a tuple object — the shape `TypeX1` flattens to.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(tuple)]
pub(crate) struct Type01Tuple {
    id: i32,
    name: String,
    list: Vec<i32>,
    item: Type02Tuple,
}

fn tuple_object_value() -> Type01Tuple {
    Type01Tuple {
        id: 1024,
        name: String::from("csharp"),
        list: vec![7, 11, 555, 1313],
        item: Type02Tuple {
            data: 2.2,
            tags: vec![
                String::from("one"),
                String::from("two"),
                String::from("three"),
            ],
        },
    }
}

/// Borrowed backend, no growth: overflow is `Error::BufferFull`, same contract
/// as C++ `Allocator(span)`.
fn encode_raw<T: Encode + ?Sized>(value: &T, buf: &mut [u8]) -> usize {
    let mut e = Encoder::on_slice(buf);
    value.encode(&mut e).expect("1024 bytes is enough");
    e.len()
}

fn decode_raw<'a, T: Decode<'a>>(bytes: &'a [u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode(&mut d).expect("encoded above")
}

pub fn run() {
    header("objects: tuple / tuple object / named (Rust)");

    let tuple = tuple_value();
    let named = named_value();
    let tuple_object = tuple_object_value();
    let mut buf = vec![0u8; 1024];

    let tuple_raw_len = encode_raw(&tuple, &mut buf);
    let tuple_raw = buf[..tuple_raw_len].to_vec();
    let obj_raw_len = encode_raw(&tuple_object, &mut buf);
    let obj_raw = buf[..obj_raw_len].to_vec();
    let named_raw_len = encode_raw(&named, &mut buf);
    let named_raw = buf[..named_raw_len].to_vec();

    // A tuple object is the same members with the tuple's prefix rules, so the
    // bytes must match the tuple exactly. If this ever fails, the two rows
    // below stop being comparable and the hex diff against C++ is void.
    assert_eq!(obj_raw, tuple_raw, "tuple object must equal the tuple wire");
    assert_eq!(
        tuple_raw_len, 50,
        "i32 + string + 4×i32 + (f64 + 3 strings)"
    );
    assert_eq!(named_raw_len, 85, "named adds 4 names + 4 value prefixes");

    let round: TypeX1 = decode_raw(&tuple_raw);
    assert_eq!(round, tuple);
    let round: Type01Tuple = decode_raw(&obj_raw);
    assert_eq!(round, tuple_object);
    // `Type01`'s fields are private to `wire`; re-encoding the decoded value is
    // the same check without widening them — the wire form is canonical, so a
    // field lost in decode cannot come back byte-identical.
    let mut again = vec![0u8; 1024];
    let round: Type01 = decode_raw(&named_raw);
    let n = encode_raw(&round, &mut again);
    assert_eq!(&again[..n], &named_raw[..], "named raw round-trip");

    println!(
        "sizes    : tuple {} / tuple object {} / named {} bytes",
        tuple_raw_len, obj_raw_len, named_raw_len
    );
    println!("tuple raw : {}", hex(&tuple_raw));
    println!("obj raw   : {}", hex(&obj_raw));
    println!("named raw : {}", hex(&named_raw));
    println!();

    bench("encode tuple (raw, on_slice)", || {
        let n = encode_raw(std::hint::black_box(&tuple), &mut buf);
        std::hint::black_box(n);
    });
    bench_ret("decode tuple (raw)", || {
        decode_raw::<TypeX1>(std::hint::black_box(&tuple_raw))
    });

    bench("encode tuple object (raw)", || {
        let n = encode_raw(std::hint::black_box(&tuple_object), &mut buf);
        std::hint::black_box(n);
    });
    bench_ret("decode tuple object (raw)", || {
        decode_raw::<Type01Tuple>(std::hint::black_box(&obj_raw))
    });

    bench("encode named (raw, on_slice)", || {
        let n = encode_raw(std::hint::black_box(&named), &mut buf);
        std::hint::black_box(n);
    });
    bench_ret("decode named (raw)", || {
        decode_raw::<Type01>(std::hint::black_box(&named_raw))
    });
    // Attribution row: names + value prefixes only, no value decoding. The gap
    // between this and the row above is what the members themselves cost, and
    // the row itself is what name lookup costs on each side (§19.15.4 measured
    // 7.5 ns for the same thing; this is the same-session control).
    bench("named parse only (4 names)", || {
        let mut d = Decoder::new(std::hint::black_box(&named_raw[..]));
        let members = binary::NamedMembers::<4>::parse(&mut d, |name| match name {
            b"Id" => Some(0),
            b"Name" => Some(1),
            b"List" => Some(2),
            b"Item" => Some(3),
            _ => None,
        })
        .expect("encoded above");
        std::hint::black_box(members.get(3));
    });
    // The ergonomic path: a fresh `Encoder` per call, i.e. what a caller gets
    // without hand-managing a buffer. C++ `Allocator()` (heap) is the same
    // shape; C# `Encode(T)` returning `byte[]` too.
    bench_ret("encode tuple (owned Vec)", || {
        let mut e = Encoder::new();
        std::hint::black_box(&tuple)
            .encode_auto(&mut e)
            .expect("infallible");
        std::hint::black_box(e).finish().expect("owned")
    });
    bench_ret("encode named (owned Vec)", || {
        let mut e = Encoder::new();
        std::hint::black_box(&named)
            .encode_auto(&mut e)
            .expect("infallible");
        std::hint::black_box(e).finish().expect("owned")
    });

    // Scalar baseline: if a single `i32` is far off a memcpy, the harness is
    // the problem, not the object layer.
    bench("encode i32 (raw)", || {
        let mut e = Encoder::on_slice(&mut buf);
        std::hint::black_box(1024i32).encode(&mut e).expect("fits");
        std::hint::black_box(e.len());
    });

    let scalar = [0x00u8, 0x04, 0x00, 0x00];
    bench_ret("decode i32 (raw)", || {
        let mut d = Decoder::new(std::hint::black_box(&scalar));
        <i32 as Decode>::decode(&mut d).expect("4 bytes")
    });
}
