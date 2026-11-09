# Binary (Rust)

Serialization library for Rust. It shares its binary layout with the
[C# version](https://github.com/afxres/binary) and the [C++ version](https://github.com/afxres/binary-cxx):
data written by one can be read by the other when member names and types match.

The C# library builds converters at run time with reflection. This one is static — three traits
(`Layout`, `Encode`, `Decode`) plus derive macros that implement them. There is no generator, no
converter registry and no `dyn Any` downcast, so an unsupported type is a compile error rather than a
runtime failure.

## Getting Started

Requires Rust 1.87. Zero runtime dependencies: nothing is pulled in beyond the optional derive macro,
which itself does not use `syn` / `quote`.

```toml
[dependencies]
binary = { path = "path/to/binary-rust/crates/binary" }
```

| Feature  | Default | Effect                                                    |
| :------- | :------ | :-------------------------------------------------------- |
| `std`    | yes     | `HashMap` / `HashSet` impls; off gives `no_std` + `alloc` |
| `derive` | yes     | `#[derive(Encode, Decode)]`                               |

`no_std` here means `no_std` + `alloc`, not allocation-free: `Vec`, `String` and the `alloc`
containers keep working, and `cargo test -p binary --no-default-features` is part of the test matrix.

## Basic Usage

```rust
use binary::{Decode, Encode, Decoder, Encoder, from_slice, to_vec};

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Person {
    id: i32,
    name: String,
    /// Not written when it equals `Default`; filled back in when absent.
    #[binary(optional)]
    age: u8,
}

fn main() {
    let person = Person { id: 42, name: "Bob".to_owned(), age: 0 };

    let buffer = to_vec(&person).unwrap();
    let result: Person = from_slice(&buffer).unwrap();
    assert_eq!(person, result);
}
```

`to_vec` / `from_slice` are the one-shot helpers: they allocate, write a **raw** value (no length
prefix of its own) and require the whole buffer to be consumed. When the allocation matters, drive
the encoder over a caller-provided slice instead:

```rust
let person = Person { id: 7, name: "Alice".to_owned(), age: 30 };

let mut scratch = [0u8; 256];
let mut encoder = Encoder::on_slice(&mut scratch);
person.encode(&mut encoder).unwrap();
let written = encoder.len();
let bytes = &scratch[..written];

let mut decoder = Decoder::new(bytes);
let back = Person::decode(&mut decoder).unwrap();
assert_eq!(decoder.remaining(), 0);
```

A value stored inside a container, or concatenated with its own length prefix, uses the `auto`
position — `encode_auto` / `decode_auto`. That is what containers and `Option<T>` call internally,
and it is how a run of values is written and read back one at a time:

```rust
let people = vec![Person { id: 1, name: "A".to_owned(), age: 0 }];

let mut encoder = Encoder::new();
for person in &people {
    person.encode_auto(&mut encoder).unwrap();
}
let buffer = encoder.finish().unwrap();

let mut decoder = Decoder::new(&buffer);
let mut result = Vec::new();
while decoder.remaining() > 0 {
    result.push(Person::decode_auto(&mut decoder).unwrap());
}
assert_eq!(result, people);
```

### Layouts

`#[derive(Encode, Decode)]` picks one of four layouts. A plain struct is a named object; the other
three are opt-in per type:

```rust
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(union)]
enum Event {
    Ping,
    Move { x: i32, y: i32 },
    Text(String),
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(int)]
enum Kind {
    File,
    Directory,
}
```

`#[binary(int)]` writes the discriminant as the enum's `#[repr]` integer, `i32` when none is given.

### Attributes

| Attribute                   | Effect                                                       |
| :-------------------------- | :----------------------------------------------------------- |
| *(none)*                    | named object: every member is written as name plus value     |
| `#[binary(tuple)]`          | tuple object: declaration order, no names, no member headers |
| `#[binary(rename = "...")]` | wire name different from the field name                      |
| `#[binary(optional)]`       | omitted when equal to `Default`, restored when absent        |
| `#[binary(skip)]`           | never on the wire, always `Default` on decode                |
| `#[binary(with = "...")]`   | member-level converter: a module with `Repr` + `to` / `from` |
| `#[binary(int)]`            | enum encoded as its underlying integer                       |
| `#[binary(union)]`          | enum encoded as a tag plus payload                           |
| `&'de str`, `&'de [u8]`     | zero-copy decode: expressed by the field type, no attribute  |

## Supported Types

| Category  | Types                                                                                                    |
| :-------- | :------------------------------------------------------------------------------------------------------- |
| Primitive | `u8` to `u128`, `i8` to `i128`, `f32`, `f64`, `bool`, `usize`, `isize`, `char` (4 bytes, i.e. C# `Rune`) |
| String    | `String`, `&str`: UTF-8, validated on decode                                                             |
| Sequence  | `Vec<T>`, `[T; N]`, `&[u8]`, `VecDeque<T>`, `LinkedList<T>`, `BinaryHeap<T>`                             |
| Map / set | `BTreeMap<K, V>`, `BTreeSet<T>`, `HashMap<K, V, S>`, `HashSet<T, S>` (any hasher, feature `std`)         |
| Tuple     | `()`, `(T,)`, up to 12 members                                                                           |
| Optional  | `Option<T>`                                                                                              |
| Borrowed  | `Cow<'_, str>`, `Cow<'_, [T]>`: decode always produces the owned variant                                 |
| Pointer   | `Box<T>`, `Rc<T>`, `Arc<T>`: wire transparent                                                            |
| Derived   | named structs, `#[binary(tuple)]` structs, C-like enums, tagged unions                                   |

Container elements use the `auto` position, so a `Vec<i32>` and a `Vec<String>` both work without
extra annotation; maps and sets are written as `(K, V)` entries. Decoding a borrowed member (`&str`,
`&[u8]`) returns a slice into the input buffer, so no copy happens on that path.

## Features

- **Four layouts from one derive**: named object, tuple object, C-like enum (`int`), tagged union.
  All four are byte-compatible with the corresponding C# and C++ converters.
- **Structural forward compatibility**: an unknown member name is skipped on decode, so new optional
  members can be added without breaking old data. No version number, no negotiation.
- **Zero copy on the read path** for `&str` and `&[u8]`, plus a borrowed encoder backend
  (`Encoder::on_slice`) that never allocates.
- **The public API is safe Rust**: `unsafe` is confined to the bulk-copy module and two internal
  helpers (fixed-array initialisation, prefix compaction).
- **`no_std` + `alloc`**, verified without default features.
- **Errors are a concrete enum** with zero-allocation static messages; boxing is opt-in
  (`Error::message`, `Error::message_static`, `Error::wrapped`), so `?` stays cheap and
  `core::error::Error::source()` still works.
- **Byte-level compatibility is tested, not assumed**: 33 C# fixtures, 27 C++ fixtures and 5 F#
  union fixtures are asserted byte for byte, and 27 compile-fail snapshots pin the derive's
  diagnostics (message, caret position, no duplicates).
- **Benchmarks are in the repo** (`crates/binary-bench`): wire, object, collection, error, profile
  and named-key lookup groups, reusing the payloads the C# and C++ benchmark devices use.

Not provided, by design: runtime converter registration / reflection (there is no reflection to
build it on), `rename_all` (write one `rename` per field), and generic targets beyond a single
lifetime parameter for borrowed structs.

## Development

```bash
cargo test --all                      # unit, integration, derive UI and doc tests
bash scripts/run-benchmark.sh         # one of wire | objects | collections | errors | profile | lookup
bash scripts/test-multiarch.sh --all  # x86_64 / i686 / aarch64 / armv7 / riscv64 / powerpc / s390x
```

`DESIGN.md` holds the rationale for every decision above, including the measurements behind the
performance-sensitive parts.

## License

[MIT](LICENSE)
