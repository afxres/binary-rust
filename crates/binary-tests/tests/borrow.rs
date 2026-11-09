//! Zero-copy decoding through derived types.
//!
//! `&'de str` and `&'de [u8]` already decode without copying at the `Decode`
//! impl level; what was missing was a way to *declare* them on a struct, because
//! the derive rejected every parameter list. One lifetime parameter is now
//! accepted and bound to the decode lifetime — `impl<'de> Decode<'de> for
//! Frame<'de>` — so a decoded value points straight into the input buffer.
//!
//! That is the whole of `#[binary(borrow)]` in this crate: borrowing is
//! expressed by the member type, not by an attribute. A field written `&'de str`
//! borrows, a field written `String` owns; the wire is identical either way, and
//! there is no second switch that could disagree with the type.

use binary::{write_named_member, Decode, Decoder, Encode, Encoder, Error};

fn encode_auto<T: Encode>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode_auto(&mut e).unwrap();
    e.finish().unwrap()
}

/// Unlike the helper in the owned tests this one ties `T` to the buffer's
/// lifetime, which is what a borrowing type needs: `T: for<'de> Decode<'de>`
/// would only accept types that own their contents.
fn decode_auto<'de, T: Decode<'de>>(bytes: &'de [u8]) -> T {
    let mut d = Decoder::new(bytes);
    T::decode_auto(&mut d).unwrap()
}

/// Borrowed members in a named object, next to an optional one.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Frame<'de> {
    #[binary(rename = "Kind")]
    kind: &'de str,
    #[binary(optional)]
    body: &'de [u8],
}

/// The owned twin: same wire, different ownership.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct FrameOwned {
    #[binary(rename = "Kind")]
    kind: String,
    #[binary(optional)]
    body: Vec<u8>,
}

#[test]
fn test_borrowed_members_share_the_owned_wire() {
    let borrowed = Frame {
        kind: "ping",
        body: b"\x01\x02",
    };
    let owned = FrameOwned {
        kind: "ping".to_string(),
        body: vec![1, 2],
    };
    assert_eq!(encode_auto(&borrowed), encode_auto(&owned));

    let bytes = encode_auto(&borrowed);
    let back: Frame<'_> = decode_auto(&bytes);
    assert_eq!(back, borrowed);

    // The optional member follows the owned rule: empty is omitted.
    let empty = Frame {
        kind: "ping",
        body: b"",
    };
    assert_eq!(
        encode_auto(&empty),
        encode_auto(&FrameOwned {
            kind: "ping".to_string(),
            body: Vec::new()
        })
    );
    assert_eq!(decode_auto::<Frame<'_>>(&encode_auto(&empty)), empty);
}

/// The point of the whole exercise: decoding allocates nothing, so the returned
/// slices must lie inside the input buffer, not in a fresh `String` / `Vec`.
#[test]
fn test_borrowed_members_point_into_the_input() {
    let bytes = encode_auto(&Frame {
        kind: "ping",
        body: b"\xde\xad",
    });
    let back: Frame<'_> = decode_auto(&bytes);

    let start = bytes.as_ptr() as usize;
    let range = start..start + bytes.len();
    assert!(
        range.contains(&(back.kind.as_ptr() as usize)),
        "kind must borrow from the input"
    );
    assert!(
        range.contains(&(back.body.as_ptr() as usize)),
        "body must borrow from the input"
    );
    assert_eq!(back.body, b"\xde\xad");
}

#[test]
fn test_borrowed_str_rejects_invalid_utf8() {
    // A named object whose `Kind` payload is not UTF-8. The bytes are valid
    // wire — the failure comes from `str::from_utf8`, exactly as it would for a
    // hand-written `&'de str` member.
    let mut e = Encoder::new();
    write_named_member(&mut e, b"Kind", &[0xffu8, 0xfe][..]).unwrap();
    let bytes = e.finish().unwrap();

    let mut d = Decoder::new(&bytes);
    assert_eq!(
        Frame::decode(&mut d).unwrap_err().to_string(),
        "invalid UTF-8"
    );
}

/// A tuple object of borrowed members: `auto` for all but the last, raw last,
/// no header of its own.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct Pair<'de> {
    left: &'de str,
    right: &'de str,
}

#[test]
fn test_borrowed_tuple_objects_flatten() {
    let pair = Pair {
        left: "a",
        right: "bc",
    };
    assert_eq!(
        encode_auto(&pair),
        encode_auto(&("a".to_string(), "bc".to_string()))
    );
    // Variable-size members make the whole tuple variable-size.
    assert_eq!(<Pair<'_> as binary::Layout>::LEN, 0);

    let bytes = encode_auto(&pair);
    let back: Pair<'_> = decode_auto(&bytes);
    assert_eq!(back, pair);
}

/// A fixed-size member before a borrowed one: the `i32` stays raw, the string
/// keeps its prefix, and the sum is still variable-size.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(tuple)]
struct Mixed<'de> {
    id: i32,
    name: &'de str,
}

#[test]
fn test_borrowed_member_after_a_fixed_one() {
    let v = Mixed { id: 7, name: "abc" };
    assert_eq!(encode_auto(&v), encode_auto(&(7i32, "abc".to_string())));
    let bytes = encode_auto(&v);
    assert_eq!(decode_auto::<Mixed<'_>>(&bytes), v);
}

/// A tagged union whose payload borrows.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(union)]
enum Msg<'de> {
    Ping,
    Text(&'de str),
}

#[test]
fn test_borrowed_union_variant() {
    let text = Msg::Text("hi");
    let bytes = encode_auto(&text);
    let back: Msg<'_> = decode_auto(&bytes);
    assert_eq!(back, text);

    let ping = Msg::Ping;
    let bytes = encode_auto(&ping);
    assert_eq!(bytes, [0x00]);
    assert_eq!(decode_auto::<Msg<'_>>(&bytes), ping);
}

/// A lifetime parameter that is *not* used for borrowing is still accepted: the
/// derive only cares that the item has at most one, and binds it to `'de`.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Marker<'a> {
    id: i32,
    tag: &'a str,
}

#[test]
fn test_lifetime_member_without_borrow_helper() {
    let v = Marker { id: 1, tag: "t" };
    let bytes = encode_auto(&v);
    assert_eq!(decode_auto::<Marker<'_>>(&bytes), v);
}

/// Error case that must stay a plain `Error`, not a panic: a truncated payload
/// for a borrowed member.
#[test]
fn test_borrowed_member_truncated_payload() {
    // `Kind` declares 4 bytes but only 2 are present.
    let bytes = [0x04, b'K', b'i', b'n', b'd', 0x04, b'a', b'b'];
    let mut d = Decoder::new(&bytes);
    // `NamedMembers::parse` reads member payloads with `read_prefix`, which
    // reports `NotEnoughBytes` (not `PayloadTruncated` — that one belongs to
    // `with_prefix`, i.e. a value's own prefix).
    assert_eq!(Frame::decode(&mut d).unwrap_err(), Error::NotEnoughBytes);
}
