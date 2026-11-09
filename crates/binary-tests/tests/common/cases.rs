//! The one table of fixture values behind the `fixtures/*.tsv` assertions.
//!
//! `csharp.tsv`, `cxx.tsv` and `fsharp.tsv` are emitted by the real C#, C++ and
//! F# libraries, but all three are asserted against the *same* Rust values, so
//! the name → type → value mapping lives here instead of being duplicated per
//! test binary. (Every `tests/*.rs` file is its own crate, so this module is
//! compiled once per file — same as `common` itself.)
//!
//! Adding a case means adding a row to `csharp.tsv` (plus `cxx.tsv` when the C++
//! side can express it, and `fsharp.tsv` only for the union rows) and an entry
//! below; an unknown name panics rather than being skipped.

use binary::{Decode, Decoder, Encode, Encoder};

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Tiny {
    x: u8,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Person {
    id: i32,
    name: String,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Mid {
    a: i32,
    b: i32,
    c: i32,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct WithOpt {
    v: Option<i32>,
}

/// Tuple objects carry no member names and no header of their own; all members
/// fixed → `LEN` is their sum, so `auto` writes them raw (C# `[TupleObject]`).
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct TupleTwoInt {
    a: i32,
    b: i32,
}

/// Same, but the last member is a string: no fixed length, still no header.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct TupleIntStr {
    a: i32,
    b: String,
}

/// C# `enum Color { Red = 1, Green = 2 }` has an `int` underlying type: 4 bytes.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(int)]
enum Color {
    Red = 1,
    Green = 2,
}

/// A union whose case order is the tag order, so it must match `Shape` in the
/// C# / F# / C++ generators one for one.
///
/// `Dot` needs a hand-written converter on the C# and C++ sides: a zero-member
/// type has no generated converter (`No available member found`) and
/// `TupleConverter<std::tuple<>>` does not compile. Both hand-written ones set
/// `Length == 0` and override the auto entry points to add no prefix, which is
/// what the Rust derive does for a unit variant. See DESIGN §16.4.
#[derive(Debug, PartialEq, Encode, Decode)]
#[binary(union)]
enum Measure {
    Dot,
    Circle(f64),
    Text(String),
    Rect { w: i32, h: i32 },
    Pair(i32, String),
}

fn encode_auto<T: Encode>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    e.finish().unwrap()
}

/// One row per fixture case: name → typed value, so one table can both encode
/// the value and decode the foreign wire back into it.
macro_rules! cases {
    ($($name:literal => $ty:ty { $value:expr }),* $(,)?) => {
        /// Encode the case into an encoder the caller owns, so a test can pick
        /// the backend and the buffer's initial contents.
        pub fn encode_case_into(name: &str, e: &mut Encoder<'_>) {
            match name {
                $($name => { let v: $ty = $value; v.encode_auto(e).unwrap(); })*
                _ => panic!("fixture case has no Rust value: {name}"),
            }
        }

        pub fn encode_case(name: &str) -> Vec<u8> {
            let mut e = Encoder::new();
            encode_case_into(name, &mut e);
            e.finish().unwrap()
        }

        /// Every case name, in table order.
        pub fn case_names() -> &'static [&'static str] {
            &[$($name),*]
        }

        /// Decode `wire` as the case's type, then re-encode it.
        pub fn reencode_case(name: &str, wire: &[u8]) -> Vec<u8> {
            match name {
                $($name => {
                    let mut d = Decoder::new(wire);
                    let v: $ty = Decode::decode_auto(&mut d).unwrap_or_else(|e| panic!("decode {name}: {e:?}"));
                    encode_auto(&v)
                })*
                _ => panic!("fixture case has no Rust value: {name}"),
            }
        }
    };
}

cases! {
    "i32_42" => i32 { 42 },
    "i32_neg7" => i32 { -7 },
    "i32opt_none" => Option<i32> { None },
    "i32opt_some5" => Option<i32> { Some(5) },
    "vec_i32opt" => Vec<Option<i32>> { vec![None, Some(5)] },
    "vec_i32opt_none" => Vec<Option<i32>> { vec![None] },
    "named_with_opt_none" => WithOpt { WithOpt { v: None } },
    "named_with_opt_some" => WithOpt { WithOpt { v: Some(5) } },
    "tupleobj_two_int" => TupleTwoInt { TupleTwoInt { a: 1, b: 2 } },
    "tupleobj_int_str" => TupleIntStr { TupleIntStr { a: 1, b: "a".to_string() } },
    "enum_green" => Color { Color::Green },
    "vec_enum" => Vec<Color> { vec![Color::Red, Color::Green] },
    "string_empty" => String { String::new() },
    "string_16a" => String { "a".repeat(16) },
    "string_17a" => String { "a".repeat(17) },
    "string_41a" => String { "a".repeat(41) },
    "string_42a" => String { "a".repeat(42) },
    "string_127a" => String { "a".repeat(127) },
    "string_128a" => String { "a".repeat(128) },
    "string_20zh" => String { "中".repeat(20) },
    "vec_i32_3" => Vec<i32> { vec![1, 2, 3] },
    "vec_i32_5" => Vec<i32> { vec![1, 2, 3, 4, 5] },
    "vec_string_4x4" => Vec<String> { vec![
        "aaaa".to_string(), "bbbb".to_string(), "cccc".to_string(), "dddd".to_string(),
    ] },
    "vec_vec_i32" => Vec<Vec<i32>> { vec![vec![1], vec![2]] },
    "tuple_i32_str" => (i32, String) { (1, "a".to_string()) },
    "named_tiny" => Tiny { Tiny { x: 7 } },
    "named_person" => Person { Person { id: 42, name: "Bob".to_string() } },
    "named_mid" => Mid { Mid { a: 1, b: 2, c: 3 } },
    "union_dot" => Measure { Measure::Dot },
    "union_circle" => Measure { Measure::Circle(1.5) },
    "union_text" => Measure { Measure::Text("a".to_string()) },
    "union_rect" => Measure { Measure::Rect { w: 1, h: 2 } },
    "union_pair" => Measure { Measure::Pair(1, "x".to_string()) },
}
