//! Wire-level tests for named objects.
//!
//! `Person` is a hand-written stand-in for what the derive macro will generate:
//! required `id` / `name`, plus an optional `age` that is omitted when default.

use binary::{
    read_named_member, write_named_member, Decode, Decoder, Encode, Encoder, Error, Layout,
    NamedMembers,
};

mod common;

use common::{split_prefixed, with_header};

#[derive(Debug, PartialEq, Eq)]
struct Person {
    id: i32,
    name: String,
    age: u8,
}

impl Layout for Person {
    // Named objects are always variable-length: member bytes include names and
    // two prefixes per member, so they never collapse to sum(LEN).
    const LEN: usize = 0;
}

impl Encode for Person {
    // No length prediction: the default `encode_with_prefix` uses
    // `Encoder::write_prefixed` for the object, while each member's value still
    // takes the O(1) precise path (fixed-size `LEN`, or `String::len()`).
    fn encode(&self, e: &mut Encoder<'_>) -> Result<(), Error> {
        write_named_member(e, b"id", &self.id)?;
        write_named_member(e, b"name", &self.name)?;
        if self.age != 0 {
            write_named_member(e, b"age", &self.age)?;
        }
        Ok(())
    }
}

fn find_member(name: &[u8]) -> Option<usize> {
    match name {
        b"id" => Some(0),
        b"name" => Some(1),
        b"age" => Some(2),
        _ => None,
    }
}

impl<'de> Decode<'de> for Person {
    fn decode(d: &mut Decoder<'de>) -> Result<Self, Error> {
        let m = NamedMembers::<3>::parse(d, find_member)?;
        Ok(Person {
            id: m.value(0, "id")?,
            name: m.value(1, "name")?,
            age: m.optional(2)?,
        })
    }
}

fn person() -> Person {
    Person {
        id: 42,
        name: "Bob".to_string(),
        age: 0,
    }
}

fn encode_auto(p: &Person) -> Vec<u8> {
    let mut e = Encoder::new();
    p.encode_auto(&mut e).unwrap();
    e.finish().unwrap()
}

fn decode_auto(bytes: &[u8]) -> Person {
    let mut d = Decoder::new(bytes);
    Person::decode_auto(&mut d).unwrap()
}

/// Rebuild a length-prefixed object with extra bytes inserted at the front of
/// its payload. The outer header width is detected, not assumed.
fn with_extra_payload(bytes: &[u8], extra: &[u8]) -> Vec<u8> {
    let (_, payload) = split_prefixed(bytes);
    let mut padded = extra.to_vec();
    padded.extend_from_slice(payload);
    with_header(&padded)
}

#[test]
fn test_named_wire_exact() {
    let bytes = encode_auto(&person());
    // outer prefix (17, 4-byte because 17 > PREFIX_SHRINK_LIMIT) | prefix("id") "id"
    // | prefix(4) 42i32 LE | ...  — matches C# `FinishAnchor` byte for byte.
    let expected = vec![
        0x80, 0x00, 0x00, 0x11, //
        0x02, b'i', b'd', //
        0x04, 0x2a, 0x00, 0x00, 0x00, //
        0x04, b'n', b'a', b'm', b'e', //
        0x03, b'B', b'o', b'b',
    ];
    assert_eq!(bytes, expected);
    // Fixed-size values still carry a prefix, unlike tuple objects.
    // [0..4]=outer prefix, [4..7]=prefix("id")+"id", [7]=prefix(4) for the i32.
    assert_eq!(bytes[7], 0x04);
}

#[test]
fn test_named_round_trip() {
    let p = person();
    assert_eq!(decode_auto(&encode_auto(&p)), p);
}

#[test]
fn test_named_unknown_member_skipped() {
    let p = person();
    let bytes = encode_auto(&p);
    // Unknown member: prefix("extra") "extra", prefix(2) [7, 8]
    let extra = [0x05, b'e', b'x', b't', b'r', b'a', 0x02, 7, 8];
    assert_eq!(decode_auto(&with_extra_payload(&bytes, &extra)), p);
}

#[test]
fn test_named_duplicate_member() {
    let bytes = encode_auto(&person());
    let (_, payload) = split_prefixed(&bytes);
    // Repeat member 0 (8 bytes: name header + value prefix + 4-byte value).
    let mut repeated = payload.to_vec();
    repeated.extend_from_slice(&payload[..8]);
    let out = with_header(&repeated);

    let mut d = Decoder::new(&out);
    assert_eq!(Person::decode_auto(&mut d), Err(Error::DuplicateMember));
}

#[test]
fn test_named_missing_required() {
    // Only the "name" member is present; "id" is required.
    let payload = [0x04, b'n', b'a', b'm', b'e', 0x03, b'B', b'o', b'b'];
    let out = with_header(&payload);

    let mut d = Decoder::new(&out);
    assert_eq!(Person::decode_auto(&mut d), Err(Error::MissingMember("id")));
}

#[test]
fn test_named_optional_missing_default() {
    // Legal wire with no "age" member; optional member falls back to Default (0).
    let payload = [
        0x02, b'i', b'd', 0x04, 0x07, 0x00, 0x00, 0x00, //
        0x04, b'n', b'a', b'm', b'e', 0x01, b'A',
    ];
    let out = with_header(&payload);

    assert_eq!(
        decode_auto(&out),
        Person {
            id: 7,
            name: "A".to_string(),
            age: 0,
        }
    );
}

#[test]
fn test_named_optional_present() {
    let p = Person {
        id: 1,
        name: "A".to_string(),
        age: 33,
    };
    let bytes = encode_auto(&p);
    assert!(bytes.windows(4).any(|w| w.starts_with(b"\x03age")));
    assert_eq!(decode_auto(&bytes), p);
}

#[test]
fn test_named_order_independent() {
    let p = person();
    let bytes = encode_auto(&p);
    let (_, payload) = split_prefixed(&bytes);
    let (id_part, name_part) = payload.split_at(8);
    let mut reordered = name_part.to_vec();
    reordered.extend_from_slice(id_part);
    let out = with_header(&reordered);
    assert_eq!(decode_auto(&out), p);
}

#[test]
fn test_named_long_value_4byte_prefix() {
    let p = Person {
        id: 1,
        name: "x".repeat(200),
        age: 0,
    };
    let bytes = encode_auto(&p);

    // Object payload > 127 → 4-byte outer prefix with bit 31 set.
    assert_eq!(bytes[0] & 0x80, 0x80);
    let outer = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x7fff_ffff;
    assert_eq!(outer as usize, bytes.len() - 4);

    // The "name" member's value prefix is also 4 bytes, holding 200.
    let at = bytes
        .windows(5)
        .position(|w| w.starts_with(b"\x04name"))
        .unwrap();
    let value_prefix = &bytes[at + 5..at + 9];
    assert_eq!(value_prefix[0] & 0x80, 0x80);
    let value_len = u32::from_be_bytes([
        value_prefix[0],
        value_prefix[1],
        value_prefix[2],
        value_prefix[3],
    ]) & 0x7fff_ffff;
    assert_eq!(value_len, 200);

    assert_eq!(decode_auto(&bytes), p);
}

#[test]
fn test_named_long_name_4byte_prefix() {
    // A member *name* over 127 bytes keeps a 4-byte prefix, same rule as a long
    // value. Pinned here because the name is now written by one reservation
    // (`Encoder::write_prefixed_bytes`) rather than two `write` calls, and the
    // prefix width has to stay a function of the name length, not of the buffer.
    struct LongName;

    impl Layout for LongName {
        const LEN: usize = 0;
    }

    impl Encode for LongName {
        fn encode(&self, e: &mut Encoder<'_>) -> Result<(), Error> {
            let name = "n".repeat(200);
            write_named_member(e, name.as_bytes(), &7i32)
        }
    }

    let mut e = Encoder::new();
    LongName.encode(&mut e).unwrap();
    let bytes = e.finish().unwrap();

    let name_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x7fff_ffff;
    assert_eq!(name_len, 200);
    assert_eq!(bytes[0] & 0x80, 0x80, "wide prefix sets bit 31");
    assert_eq!(&bytes[4..204], &[b'n'; 200][..]);
    assert_eq!(bytes[204], 4, "i32 value prefix holds its byte length");
    assert_eq!(&bytes[205..209], &7i32.to_le_bytes());
    assert_eq!(bytes.len(), 209);
}

#[test]
fn test_read_named_member_pairs_name_with_raw_payload() {
    // The read half of `write_named_member`: both prefixes are consumed by one
    // call, and the payload comes back without its prefix, so a slot is decoded
    // with `T::decode` and not `auto`.
    let mut e = Encoder::new();
    write_named_member(&mut e, b"id", &7i32).unwrap();
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    let (name, payload) = read_named_member(&mut d).unwrap();
    assert_eq!(name, b"id");
    assert_eq!(payload, &7i32.to_le_bytes()[..]);
    assert_eq!(d.remaining(), 0);
}

#[test]
fn test_named_decode_encode_idempotent() {
    let bytes = encode_auto(&person());
    let decoded = decode_auto(&bytes);
    assert_eq!(encode_auto(&decoded), bytes);
}

#[test]
fn test_named_truncated_member() {
    // A name header with no value prefix behind it.
    let payload = [0x02, b'i', b'd'];
    let out = with_header(&payload);
    let mut d = Decoder::new(&out);
    assert_eq!(Person::decode_auto(&mut d), Err(Error::NotEnoughBytes));
}

struct Opaque(Vec<u8>);

impl Layout for Opaque {
    const LEN: usize = 0;
}

impl Encode for Opaque {
    fn encode(&self, e: &mut Encoder<'_>) -> Result<(), Error> {
        e.write(&self.0)
    }
}

#[test]
fn test_named_write_prefixed_path() {
    // Both the object and its member fall back to the `write_prefixed` path.
    let mut e = Encoder::new();
    e.write_prefixed(|e| write_named_member(e, b"blob", &Opaque(vec![1, 2, 3])))
        .unwrap();
    let bytes = e.finish().unwrap();

    // payload = prefix("blob") "blob" (5) + prefix([1,2,3]) (1) + 3 = 9
    assert_eq!(bytes[0], 9);

    let mut d = Decoder::new(&bytes);
    let value: Vec<u8> = d
        .with_prefix(|d| {
            let m = NamedMembers::<1>::parse(d, |n| match n {
                b"blob" => Some(0),
                _ => None,
            })?;
            Ok(m.require(0, "blob")?.to_vec())
        })
        .unwrap();
    assert_eq!(value, vec![1, 2, 3]);
}
