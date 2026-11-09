//! Tests for the zero-dependency derive macros.
//!
//! `DerivedPerson` deliberately mirrors the hand-written `Person` in
//! `named.rs` so the generated code can be checked against the known wire.

use binary::{write_named_member, Decode, Decoder, Encode, Encoder};

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct DerivedPerson {
    id: i32,
    name: String,
    #[binary(optional)]
    age: u8,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct WithAttrs {
    id: i32,
    #[binary(rename = "Name")]
    name: String,
    #[binary(optional)]
    age: u8,
    #[binary(skip)]
    cache: u32,
}

#[derive(Debug, PartialEq, Eq, Encode)]
struct EncodeOnly {
    value: i32,
}

/// C# `record Person(int Id, string Name)`, translated.
///
/// A positional record is a **named object** on the wire, keyed by property
/// name — it is not positional in the binary sense. Two consequences:
///
/// - a Rust tuple struct is rejected by the derive (`tuple structs and unit
///   structs are not supported`), and `#[binary(tuple)]` would be a *different*
///   layout (no names, no per-member prefix), so this must be a named struct;
/// - C# property names are PascalCase, so every field carries a `rename`.
///   Snake-case field names are the Rust convention, so the renames cannot be
///   dropped even though they cost nothing at runtime.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct RecordPerson {
    #[binary(rename = "Id")]
    id: i32,
    #[binary(rename = "Name")]
    name: String,
}

/// Tuple object: no member names, no header; all members fixed → `LEN = 8`.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct TupleTwoInt {
    a: i32,
    b: i32,
}

/// Tuple object with a variable tail member: `LEN = 0`, still no header.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct TupleIntStr {
    a: i32,
    b: String,
}

/// `rename` with non-ASCII text: the name is UTF-8 on the wire.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct CjkName {
    #[binary(rename = "名字")]
    n: i32,
}

fn encode_auto<T: Encode>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    e.finish().unwrap()
}

fn decode_auto<T: for<'de> Decode<'de>>(bytes: &[u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode_auto(&mut d).unwrap()
}

#[test]
fn test_derive_wire_matches_handwritten_layout() {
    let p = DerivedPerson {
        id: 42,
        name: "Bob".to_string(),
        age: 0,
    };
    let bytes = encode_auto(&p);
    // 4-byte outer header: payload is 17 > PREFIX_SHRINK_LIMIT, same as C#.
    let expected = vec![
        0x80, 0x00, 0x00, 0x11, //
        0x02, b'i', b'd', //
        0x04, 0x2a, 0x00, 0x00, 0x00, //
        0x04, b'n', b'a', b'm', b'e', //
        0x03, b'B', b'o', b'b',
    ];
    assert_eq!(bytes, expected);
    assert_eq!(decode_auto::<DerivedPerson>(&bytes), p);
}

#[test]
fn test_csharp_record_wire() {
    let p = RecordPerson {
        id: 42,
        name: "Bob".to_string(),
    };

    // Ground truth from the real library (Mikodev.Binary 11.x):
    //
    //   record Person(int Id, string Name);
    //   Generator.Encode(new Person(42, "Bob"))
    //     -> 024964 042a000000 044e616d65 03426f62
    //   Converter<Person>.EncodeAuto(..)
    //     -> 80000011 + the same 17 bytes (payload > PREFIX_SHRINK_LIMIT)
    //
    // Raw first: C# sorts named members by name, Rust writes declaration order.
    // "Id" < "Name" holds either way, so this case is byte-identical for both
    // reasons at once; a type whose declaration order is not alphabetical would
    // match C# only after the ordering question in §1 is decided.
    let mut e = Encoder::new();
    p.encode(&mut e).unwrap();
    let raw = e.finish().unwrap();
    assert_eq!(
        raw,
        b"\x02Id\x04\x2a\x00\x00\x00\x04Name\x03Bob".to_vec(),
        "raw member bytes must match C# byte for byte"
    );

    let auto = encode_auto(&p);
    assert_eq!(
        &auto[..4],
        [0x80, 0x00, 0x00, 0x11],
        "17-byte payload keeps the wide header, same as C# FinishAnchor"
    );
    assert_eq!(&auto[4..], &raw[..]);

    // C# decodes its own wire; Rust must reach the same value from the same bytes.
    assert_eq!(decode_auto::<RecordPerson>(&auto), p);
}

#[test]
fn test_derive_optional_present() {
    let p = DerivedPerson {
        id: 1,
        name: "A".to_string(),
        age: 33,
    };
    let bytes = encode_auto(&p);
    assert!(bytes.windows(4).any(|w| w.starts_with(b"\x03age")));
    assert_eq!(decode_auto::<DerivedPerson>(&bytes), p);
}

#[test]
fn test_derive_rename_and_skip() {
    let v = WithAttrs {
        id: 42,
        name: "Bob".to_string(),
        age: 0,
        cache: 999,
    };
    let bytes = encode_auto(&v);
    assert!(bytes.windows(5).any(|w| w.starts_with(b"\x04Name")));
    assert!(!bytes.windows(6).any(|w| w.starts_with(b"\x05cache")));

    let decoded = decode_auto::<WithAttrs>(&bytes);
    assert_eq!(decoded.id, 42);
    assert_eq!(decoded.name, "Bob");
    assert_eq!(decoded.age, 0);
    assert_eq!(decoded.cache, 0, "skipped field must take Default");
}

#[test]
fn test_derive_decode_encode_idempotent() {
    let p = DerivedPerson {
        id: -7,
        name: "你好".to_string(),
        age: 12,
    };
    let bytes = encode_auto(&p);
    assert_eq!(encode_auto(&decode_auto::<DerivedPerson>(&bytes)), bytes);
}

#[test]
fn test_derive_encode_only() {
    // `#[derive(Encode)]` alone provides Layout + Encode; no Decode needed.
    let v = EncodeOnly { value: 7 };
    let mut e = Encoder::new();
    v.encode(&mut e).unwrap();
    let bytes = e.finish().unwrap();
    assert!(bytes.windows(6).any(|w| w.starts_with(b"\x05value")));
    assert_eq!(bytes[6], 0x04);
    assert_eq!(&bytes[7..11], &[7, 0, 0, 0]);
}

#[test]
fn test_derive_tuple_object_wire() {
    // Member bytes without names and without any header of their own —
    // byte-identical to C# `[TupleObject]` (fixtures: tupleobj_two_int /
    // tupleobj_int_str) and to the N-tuple with the same members.
    assert_eq!(
        encode_auto(&TupleTwoInt { a: 1, b: 2 }),
        [1, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(
        encode_auto(&TupleIntStr {
            a: 1,
            b: "a".to_string()
        }),
        [1, 0, 0, 0, 1, b'a']
    );
    assert_eq!(
        encode_auto(&TupleTwoInt { a: 1, b: 2 }),
        encode_auto(&(1i32, 2i32)),
        "tuple object must equal the flat tuple"
    );

    let v = TupleIntStr {
        a: -5,
        b: "hello".to_string(),
    };
    assert_eq!(decode_auto::<TupleIntStr>(&encode_auto(&v)), v);
}

#[test]
fn test_derive_tuple_object_fixed() {
    use binary::Layout;

    // C# `TupleObject.GetConverterLength`: sum of member lengths, 0 if any is
    // variable. `LEN` decides the header width of enclosing collections.
    assert_eq!(TupleTwoInt::LEN, 8);
    assert_eq!(TupleIntStr::LEN, 0);

    let v = vec![
        TupleTwoInt { a: 1, b: 2 },
        TupleTwoInt { a: 3, b: 4 },
        TupleTwoInt { a: 5, b: 6 },
    ];
    let bytes = encode_auto(&v);
    assert_eq!(bytes[0], 24, "3 * 8 bytes predicted exactly");
    assert_eq!(bytes.len(), 1 + 24);
    assert_eq!(decode_auto::<Vec<TupleTwoInt>>(&bytes), v);
}

#[test]
fn test_derive_rename_non_ascii() {
    let bytes = encode_auto(&CjkName { n: 7 });
    // outer prefix 12 | 06 "名字"(UTF-8) | 04 07 00 00 00
    assert_eq!(
        bytes,
        [0x0C, 0x06, 0xE5, 0x90, 0x8D, 0xE5, 0xAD, 0x97, 0x04, 7, 0, 0, 0]
    );
    assert_eq!(decode_auto::<CjkName>(&bytes), CjkName { n: 7 });
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(int)]
enum Level {
    Low,
    High = 7,
    Bumped,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(int)]
#[repr(u8)]
enum Small {
    Zero = 0,
    Max = 255,
}

#[test]
fn test_derive_enum_default_width() {
    use binary::Layout;
    // C# `enum : int` → 4 bytes little-endian (fixture `enum_green`).
    assert_eq!(Level::LEN, 4);
    assert_eq!(encode_auto(&Level::Low), 0i32.to_le_bytes());
    assert_eq!(encode_auto(&Level::High), 7i32.to_le_bytes());
    // Implicit discriminants continue from the previous value.
    assert_eq!(encode_auto(&Level::Bumped), 8i32.to_le_bytes());

    assert_eq!(
        decode_auto::<Level>(&encode_auto(&Level::High)),
        Level::High
    );
    assert_eq!(
        decode_auto::<Level>(&encode_auto(&Level::Bumped)),
        Level::Bumped
    );
}

#[test]
fn test_derive_enum_repr_u8() {
    use binary::Layout;
    assert_eq!(Small::LEN, 1);
    assert_eq!(encode_auto(&Small::Zero), [0]);
    assert_eq!(encode_auto(&Small::Max), [255]);
    assert_eq!(decode_auto::<Small>(&[255]), Small::Max);
}

#[test]
fn test_derive_enum_invalid_discriminant() {
    // 32 is not a declared discriminant: decoding must fail, not alias a variant.
    let mut d = Decoder::new(&[0x20, 0, 0, 0]);
    assert!(Level::decode_auto(&mut d).is_err(), "32 is not a Level");
}

// ---- `#[binary(union)]` ----

/// Tagged union: compact-int tag (declaration order) + tuple-object payload.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum Shape {
    Dot,
    Circle(f64),
    Text(String),
    Rect { w: i32, h: i32 },
}

/// Raw and `auto` payload forms only differ for variable-size members.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum Msg {
    Empty,
    Count(i32),
    Pair(i32, String),
}

/// A union used as a named-object member: the member value goes through
/// `encode_with_prefix`, i.e. the `write_prefixed` path over the raw form.
#[derive(Debug, PartialEq, Encode, Decode)]
struct Wrapped {
    shape: Shape,
}

fn encode_raw<T: Encode>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode(&mut e).unwrap();
    e.finish().unwrap()
}

fn decode_raw<T: for<'de> Decode<'de>>(bytes: &[u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode(&mut d).unwrap()
}

#[test]
fn test_union_tag_wire() {
    use binary::Layout;

    // Always variable-size: the tag width depends on the variant index, so there
    // is no single fixed length (C# `UnionConverter` is a `Converter<T?>` too).
    assert_eq!(Shape::LEN, 0);

    // A unit variant is the tag alone; `auto` adds no header of its own.
    assert_eq!(encode_auto(&Shape::Dot), [0x00]);

    // Tag is a compact integer, payload is `auto`: fixed-size members stay raw.
    let circle = encode_auto(&Shape::Circle(1.5));
    assert_eq!(circle[0], 0x01);
    assert_eq!(&circle[1..], &1.5f64.to_le_bytes()[..]);
    assert_eq!(circle.len(), 9);

    // A struct variant is a tuple object of its members, in declaration order.
    assert_eq!(
        encode_auto(&Shape::Rect { w: 1, h: 2 }),
        [0x03, 1, 0, 0, 0, 2, 0, 0, 0]
    );

    // `auto` payload keeps the member's own prefix, the raw form does not.
    assert_eq!(
        encode_auto(&Shape::Text("a".to_string())),
        [0x02, 0x01, b'a']
    );
    assert_eq!(encode_raw(&Shape::Text("a".to_string())), [0x02, b'a']);
}

#[test]
fn test_union_raw_vs_auto_payload() {
    // Multi-member variant: all but the last member are `auto`, the last one is
    // raw (C# union case fields are encoded exactly like a tuple object).
    assert_eq!(
        encode_auto(&Msg::Pair(1, "x".to_string())),
        [0x02, 1, 0, 0, 0, 0x01, b'x']
    );
    assert_eq!(
        encode_raw(&Msg::Pair(1, "x".to_string())),
        [0x02, 1, 0, 0, 0, b'x']
    );

    assert_eq!(encode_auto(&Msg::Empty), [0x00]);
    assert_eq!(encode_raw(&Msg::Count(7)), [0x01, 7, 0, 0, 0]);
}

#[test]
fn test_union_round_trip() {
    for v in [
        Shape::Dot,
        Shape::Circle(1.5),
        Shape::Text("hello".to_string()),
        Shape::Rect { w: -1, h: 9 },
    ] {
        assert_eq!(decode_auto::<Shape>(&encode_auto(&v)), v);
        assert_eq!(decode_raw::<Shape>(&encode_raw(&v)), v);
    }

    let p = Msg::Pair(1, "x".to_string());
    assert_eq!(decode_auto::<Msg>(&encode_auto(&p)), p);
    assert_eq!(decode_raw::<Msg>(&encode_raw(&p)), p);
}

#[test]
fn test_union_in_vec() {
    let v = vec![
        Shape::Dot,
        Shape::Text("hi".to_string()),
        Shape::Circle(0.5),
    ];
    let bytes = encode_auto(&v);
    assert_eq!(decode_auto::<Vec<Shape>>(&bytes), v);
}

#[test]
fn test_union_invalid_tag() {
    // 7 is not a declared variant: decoding must fail instead of aliasing.
    let mut d = Decoder::new(&[0x07]);
    assert_eq!(
        Shape::decode_auto(&mut d).unwrap_err(),
        binary::Error::InvalidTag(7)
    );

    let mut d = Decoder::new(&[0x07]);
    assert_eq!(
        Shape::decode(&mut d).unwrap_err(),
        binary::Error::InvalidTag(7)
    );
}

/// `Option<T>` is, on the wire, exactly the two-variant union `None | Some(T)`.
///
/// The implementations are separate — `option.rs` writes a fixed one-byte tag,
/// the derive writes a compact int — so the coincidence is pinned here rather
/// than shared in code. It holds because both tags (0/1) sit in the
/// single-byte range of the compact int.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum OptI32 {
    None,
    Some(i32),
}

/// Same shape with a variable-size payload: the `auto` prefix must appear on
/// both sides identically.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum OptString {
    None,
    Some(String),
}

#[test]
fn test_union_two_variants_match_option() {
    // Encode: tag bytes and `auto` payload coincide, fixed-size and variable.
    assert_eq!(encode_auto(&OptI32::None), encode_auto(&None::<i32>));
    assert_eq!(encode_auto(&OptI32::Some(5)), encode_auto(&Some(5i32)));
    assert_eq!(encode_auto(&OptI32::Some(5)), [0x01, 5, 0, 0, 0]);
    assert_eq!(
        encode_auto(&OptString::Some("a".to_string())),
        encode_auto(&Some("a".to_string()))
    );
    assert_eq!(
        encode_auto(&OptString::Some("a".to_string())),
        [0x01, 0x01, b'a']
    );

    // Each implementation decodes the other's wire.
    assert_eq!(
        decode_auto::<Option<i32>>(&encode_auto(&OptI32::Some(5))),
        Some(5)
    );
    assert_eq!(
        decode_auto::<Option<i32>>(&encode_auto(&OptI32::None)),
        None
    );
    assert_eq!(
        decode_auto::<OptI32>(&encode_auto(&Some(5i32))),
        OptI32::Some(5)
    );
    assert_eq!(
        decode_auto::<OptString>(&encode_auto(&Some("a".to_string()))),
        OptString::Some("a".to_string())
    );
}

#[test]
fn test_union_as_named_member() {
    let w = Wrapped {
        shape: Shape::Text("a".to_string()),
    };
    // 09 | 05 "shape" | 02 (value prefix, prefix-shrunk) | 02 'a' (raw union)
    assert_eq!(
        encode_auto(&w),
        [0x09, 0x05, b's', b'h', b'a', b'p', b'e', 0x02, 0x02, b'a']
    );
    assert_eq!(decode_auto::<Wrapped>(&encode_auto(&w)), w);
}

/// Degenerate shapes: an empty tuple variant and a field type that contains
/// commas inside its own delimiters.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum Edge {
    Nothing(),
    Pairs(Vec<(i32, i32)>),
}

#[test]
fn test_union_edge_shapes() {
    // `Nothing()` is a zero-member tuple variant: tag only, same as a unit one.
    assert_eq!(encode_auto(&Edge::Nothing()), [0x00]);
    assert_eq!(decode_auto::<Edge>(&[0x00]), Edge::Nothing());

    // `Vec<(i32, i32)>` parses as one type token sequence: `<`/`>` tracked,
    // `(i32, i32)` is a single delimited group so its comma does not split fields.
    let v = Edge::Pairs(vec![(1, 2), (3, 4)]);
    assert_eq!(decode_auto::<Edge>(&encode_auto(&v)), v);
    assert_eq!(decode_raw::<Edge>(&encode_raw(&v)), v);
}

// ---- `#[binary(with = "...")]`: member-level converters ----
//
// C# spells this `[Converter(typeof(...))]`, C++ `FixedConverterPolicy`; both
// attach a converter to a *member*, so the wire shape can differ from the
// language-level type. In Rust the field type sometimes cannot implement
// `Encode` at all — the orphan rule forbids it for `SystemTime`, `Uuid`, or
// anything else from another crate — which makes a member-level converter the
// only way to express the layout.

/// Milliseconds on the wire, an opaque duration in memory.
///
/// Implements **none** of `Layout` / `Encode` / `Decode`, exactly like a foreign
/// type: that is the case `with` exists for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Elapsed(i64);

/// Converter module: a `Repr` plus the two conversions.
///
/// The wire form is **`Repr`'s** wire form — prefix width, fixed-size treatment
/// and nesting all come from `Repr`'s own `Layout` / `Encode` / `Decode`. There
/// is no separate length contract to get wrong, which is why the attribute is a
/// type substitution rather than a pair of encode/decode functions.
mod ticks {
    use super::Elapsed;
    use binary::{Error, Result};

    pub type Repr = i64;

    pub fn to(v: &Elapsed) -> Repr {
        v.0
    }

    pub fn from(r: Repr) -> Result<Elapsed> {
        if r < 0 {
            return Err(Error::message_static("negative elapsed"));
        }
        Ok(Elapsed(r))
    }
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Timer {
    #[binary(rename = "Elapsed")]
    #[binary(with = "ticks")]
    elapsed: Elapsed,
    #[binary(optional)]
    #[binary(with = "ticks")]
    since: Elapsed,
}

/// The same named object without converters, for the wire-identity assertion.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct TimerRepr {
    #[binary(rename = "Elapsed")]
    elapsed: i64,
    #[binary(optional)]
    since: i64,
}

#[test]
fn test_with_member_matches_repr_wire() {
    // `optional` keeps its rule: the default value is omitted, so only one
    // member is on the wire.
    let timer = Timer {
        elapsed: Elapsed(1500),
        since: Elapsed(0),
    };
    assert_eq!(
        encode_auto(&timer),
        encode_auto(&TimerRepr {
            elapsed: 1500,
            since: 0
        })
    );
    assert_eq!(decode_auto::<Timer>(&encode_auto(&timer)), timer);

    // Both members present.
    let timer = Timer {
        elapsed: Elapsed(1),
        since: Elapsed(7),
    };
    assert_eq!(
        encode_auto(&timer),
        encode_auto(&TimerRepr {
            elapsed: 1,
            since: 7
        })
    );
    assert_eq!(decode_auto::<Timer>(&encode_auto(&timer)), timer);
}

#[test]
fn test_with_converter_error_is_not_a_wire_error() {
    // `-1` is a perfectly valid `i64` payload; only the converter rejects it.
    let mut e = Encoder::new();
    write_named_member(&mut e, b"Elapsed", &-1i64).unwrap();
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    assert_eq!(
        Timer::decode(&mut d).unwrap_err().to_string(),
        "negative elapsed"
    );
}

/// Fixed-size converter in a tuple object: `Repr = i64` has `LEN = 8`, so the
/// tuple is fixed-size and both members go raw — no prefixes.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct Span {
    #[binary(with = "ticks")]
    start: Elapsed,
    #[binary(with = "ticks")]
    end: Elapsed,
}

#[test]
fn test_with_member_in_tuple_is_fixed_size() {
    let span = Span {
        start: Elapsed(1),
        end: Elapsed(2),
    };
    assert_eq!(encode_auto(&span), encode_auto(&(1i64, 2i64)));
    assert_eq!(decode_auto::<Span>(&encode_auto(&span)), span);
    assert_eq!(<Span as binary::Layout>::LEN, 16);
}

/// A byte blob written as hex text: the `Repr` is variable-size, so the member
/// gets a prefix in `auto` position and stays raw as the last tuple member.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Blob(Vec<u8>);

mod hex {
    use super::Blob;
    use binary::{Error, Result};

    pub type Repr = String;

    pub fn to(v: &Blob) -> Repr {
        v.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from(r: Repr) -> Result<Blob> {
        let bytes = r.as_bytes();
        if !bytes.len().is_multiple_of(2) {
            return Err(Error::message_static("odd hex length"));
        }
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => out.push((h * 16 + l) as u8),
                _ => return Err(Error::message_static("invalid hex digit")),
            }
        }
        Ok(Blob(out))
    }
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct Tagged {
    #[binary(with = "hex")]
    tag: Blob,
    n: i32,
}

#[test]
fn test_with_member_in_tuple_with_variable_repr() {
    let v = Tagged {
        tag: Blob(vec![0x0a, 0xff]),
        n: -3,
    };
    // Identical to `(String, i32)`: first member prefixed, last member raw.
    assert_eq!(encode_auto(&v), encode_auto(&("0aff".to_string(), -3i32)));
    assert_eq!(decode_auto::<Tagged>(&encode_auto(&v)), v);
    // `String::LEN == 0`, so the whole tuple object is variable-size.
    assert_eq!(<Tagged as binary::Layout>::LEN, 0);
}

#[test]
fn test_with_converter_rejects_malformed_payload() {
    let mut e = Encoder::new();
    ("0af".to_string(), 1i32).encode_auto(&mut e).unwrap();
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    assert_eq!(
        Tagged::decode_auto(&mut d).unwrap_err().to_string(),
        "odd hex length"
    );
}
