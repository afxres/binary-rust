//! How a custom type's own error type reaches the caller of `encode` / `decode`.
//!
//! The trait signatures are fixed to `binary::Result<T>`, so a foreign error
//! cannot travel *through* them as-is — it is degraded at the boundary. These
//! tests pin the two degradations and their costs: `message` (message only, no
//! allocation when static) and `wrapped` (type retained behind an `Arc`, textual
//! equality). Plus the `?` bridge and the position folding that comes with it.
//!
//! The last section pins the *names* themselves: each variant has to describe
//! the failure it actually reports (DESIGN §18.9).

use binary::{Decode, Decoder, Encode, Encoder, Error, Layout, Result};
use std::borrow::Cow;

/// A domain error owned by the (simulated) downstream crate.
#[derive(Debug, PartialEq)]
struct OutOfRange(u8);

impl core::fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "count {} out of range", self.0)
    }
}

/// The bridge that makes `?` work inside a custom impl.
///
/// Legal under the orphan rule: `OutOfRange` is local, `Error` is foreign, and the
/// local type is the trait's type parameter.
impl From<OutOfRange> for Error {
    fn from(e: OutOfRange) -> Self {
        Error::message(e)
    }
}

fn check_count(v: u8) -> core::result::Result<(), OutOfRange> {
    if v > 100 {
        Err(OutOfRange(v))
    } else {
        Ok(())
    }
}

/// Leaf of the chain built for the `wrapped` tests.
#[derive(Debug)]
struct Checksum;

impl core::fmt::Display for Checksum {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "checksum mismatch")
    }
}

impl std::error::Error for Checksum {}

/// A mid-level error that owns its source, like a real frame parser would.
#[derive(Debug)]
struct BadFrame(u32);

impl core::fmt::Display for BadFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "bad frame {}", self.0)
    }
}

impl std::error::Error for BadFrame {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        static CHECKSUM: Checksum = Checksum;
        Some(&CHECKSUM)
    }
}

fn check_frame(len: usize) -> core::result::Result<(), BadFrame> {
    if len == 4 {
        Ok(())
    } else {
        Err(BadFrame(len as u32))
    }
}

/// The same `?` bridge as `OutOfRange`'s, but keeping the type this time.
impl From<BadFrame> for Error {
    fn from(e: BadFrame) -> Self {
        Error::wrapped(e)
    }
}

#[test]
fn test_from_impl_makes_question_mark_work() {
    /// Stand-in for a downstream type that validates inside its impl.
    ///
    /// `check_count(...)?` fails *inside* `encode` / `decode`, which is the
    /// path under test: the caller must receive `Error::Message`, not `OutOfRange`.
    /// `payload` keeps the type variable-length so the happy-path round-trip
    /// also covers a length prefix, i.e. the impl is not a stub that only ever
    /// errors.
    #[derive(Debug, PartialEq)]
    struct Dense {
        count: u8,
        payload: Vec<u8>,
    }

    impl Layout for Dense {}

    impl Encode for Dense {
        fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
            check_count(self.count)?;
            e.write_byte(self.count)?;
            self.payload.encode_auto(e)
        }
    }

    impl<'de> Decode<'de> for Dense {
        fn decode(d: &mut Decoder<'de>) -> Result<Self> {
            let count = d.take_byte()?;
            check_count(count)?;
            let payload = <Vec<u8> as Decode>::decode_auto(d)?;
            Ok(Self { count, payload })
        }
    }

    fn encode_raw<T: Encode>(v: &T) -> Result<Vec<u8>> {
        let mut e = Encoder::new();
        v.encode(&mut e)?;
        e.finish()
    }

    // Encode side: `check_count(...)?` converts `OutOfRange` into `Error::Message`.
    let bad = Dense {
        count: 200,
        payload: vec![],
    };
    let err = encode_raw(&bad).unwrap_err();
    assert_eq!(err.to_string(), "count 200 out of range");
    assert!(matches!(err, Error::Message(_)));

    // Decode side: same message, same variant.
    let mut d = Decoder::new(&[200]);
    let err = <Dense as Decode>::decode(&mut d).unwrap_err();
    assert_eq!(err.to_string(), "count 200 out of range");

    // And the happy path still round-trips.
    let ok = Dense {
        count: 3,
        payload: vec![1, 2, 3],
    };
    let bytes = encode_raw(&ok).unwrap();
    assert_eq!(bytes, [0x03, 0x03, 1, 2, 3]);
    assert_eq!(
        <Dense as Decode>::decode(&mut Decoder::new(&bytes)).unwrap(),
        ok
    );
}

#[test]
fn test_message_shape_static_vs_allocated() {
    // A literal still goes through `Display`, so `message` allocates. The crate's
    // own paths use `message_static` and stay allocation-free.
    let owned = Error::message("boom");
    assert!(matches!(owned, Error::Message(Cow::Owned(_))));

    let borrowed = Error::message_static("boom");
    assert!(matches!(borrowed, Error::Message(Cow::Borrowed(_))));

    // Ownership is not part of equality: `Cow` compares contents, so assertions
    // written against a variant keep working regardless of how it was built.
    assert_eq!(owned, borrowed);
    assert_eq!(owned.clone(), borrowed);
}

#[test]
fn test_degraded_error_cannot_be_recovered() {
    // The caller can still match on *our* variant and read the message...
    let err = Error::message(OutOfRange(200));
    assert!(matches!(err, Error::Message(_)));

    // ...but not get `OutOfRange` back: the `Display` output is all that survives.
    // Consequence for API design — an impl that must keep the structured error
    // has to say so at the degradation point: `Error::wrapped(e)` instead of
    // `Error::message(e)` (pinned by the `wrapped` tests below). Without that,
    // wire conversion belongs in the impl and domain validation one level up,
    // where the error type is still visible.
    assert_eq!(err.to_string(), "count 200 out of range");

    // The documented workaround is an outer enum that keeps both sides apart.
    #[derive(Debug)]
    enum AppError {
        Wire(Error),
        Domain(OutOfRange),
    }
    impl From<Error> for AppError {
        fn from(e: Error) -> Self {
            AppError::Wire(e)
        }
    }
    let wrapped = AppError::from(err);
    assert!(matches!(wrapped, AppError::Wire(Error::Message(_))));

    // The other side of the comparison: had the check lived *outside* the impl,
    // the original type would still be here, untouched.
    let domain = AppError::Domain(OutOfRange(200));
    assert!(matches!(domain, AppError::Domain(OutOfRange(200))));
}

#[test]
fn test_failure_offset_folds_through_nested_prefixes() {
    // outer scope: 2-byte payload; inner scope: 1-byte payload holding 0xAA.
    let bytes = [0x02, 0x01, 0xAA];
    let mut d = Decoder::new(&bytes);

    let err = d
        .with_prefix(|d| {
            d.with_prefix(|d| {
                d.take(1)?; // consumes 0xAA, inner cursor = 1
                d.take(1).map(|_| ()) // fails; `take` leaves the cursor alone
            })
        })
        .unwrap_err();

    assert_eq!(err, Error::NotEnoughBytes);
    // The absolute offset is the caller's arithmetic — `total - remaining`, the
    // same expression C#/C++ use because there the span *is* the cursor.
    // Consumed: 1 (outer prefix) + 1 (inner prefix) + 1 (0xAA) = 3, i.e. the end
    // of the buffer. Without the fold inside `with_prefix` this would report 0.
    assert_eq!(bytes.len() - d.remaining(), 3);
}

#[test]
fn test_offset_reported_from_inside_a_custom_impl() {
    // A custom impl only ever sees a *scoped* decoder, so the honest thing it
    // can report is how far into the value it stopped; turning that into an
    // absolute byte is the caller's job, since only it holds the buffer.
    #[derive(Debug)]
    struct Probe;
    impl Layout for Probe {}
    impl<'de> Decode<'de> for Probe {
        fn decode(d: &mut Decoder<'de>) -> Result<Self> {
            let start = d.remaining();
            let _ = d.take(1)?;
            Err(Error::message(format_args!(
                "stopped {} byte(s) into the value",
                start - d.remaining()
            )))
        }
    }

    let mut d = Decoder::new(&[0xAA, 0xBB]);
    let err = <Probe as Decode>::decode(&mut d).unwrap_err();
    assert_eq!(err.to_string(), "stopped 1 byte(s) into the value");
    assert_eq!(
        2 - d.remaining(),
        1,
        "absolute offset is one subtraction away"
    );
}

// --- Error::wrapped: the type-erased channel ---------------------------------

#[test]
fn test_wrapped_error_keeps_type_and_source_chain() {
    let err = Error::wrapped(BadFrame(7));

    // `Display` delegates to the payload; no context is invented, so the wrapper
    // and the source print the same thing.
    assert!(matches!(err, Error::Wrapped(_)));
    assert_eq!(err.to_string(), "bad frame 7");

    // `source()` hands the payload back, and the payload's own chain still works.
    let src = std::error::Error::source(&err).expect("Source must expose its payload");
    assert_eq!(src.to_string(), "bad frame 7");
    assert_eq!(
        std::error::Error::source(src)
            .expect("BadFrame has a source")
            .to_string(),
        "checksum mismatch"
    );

    // The whole point: the *type* survives the erasure, so a caller can branch on
    // the kind of failure instead of parsing the message string.
    assert_eq!(src.downcast_ref::<BadFrame>().map(|e| e.0), Some(7));
    assert!(src.downcast_ref::<Checksum>().is_none());

    // `Error` stays `Send + Sync + 'static`; that is what the bound on `wrapped`
    // buys, and what moving an error to another thread relies on.
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Error>();

    // `Clone` is a refcount bump: the clone shares the allocation, so it is
    // equal and still downcasts.
    let cloned = err.clone();
    assert_eq!(cloned, err);
    assert!(std::error::Error::source(&cloned)
        .unwrap()
        .downcast_ref::<BadFrame>()
        .is_some());
}

#[test]
fn test_wrapped_equality_is_textual() {
    /// Prints the same message as `BadFrame(7)`, but is an unrelated type.
    #[derive(Debug)]
    struct Impostor;

    impl core::fmt::Display for Impostor {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "bad frame 7")
        }
    }

    impl std::error::Error for Impostor {}

    let real = Error::wrapped(BadFrame(7));
    let fake = Error::wrapped(Impostor);

    // Accepted consequence, documented on `ErasedError`: erasure leaves no type id
    // to compare, so equality falls back to `Display`. Two unrelated types that
    // print the same text compare equal.
    assert_eq!(real, fake);
    assert_eq!(real.clone(), fake.clone());

    // Same type, different payload: the message differs, so they differ.
    assert_ne!(real, Error::wrapped(BadFrame(8)));

    // Variants never cross. `Source` is not `Custom`, even with identical text...
    assert_ne!(real, Error::message_static("bad frame 7"));

    // ...while `Custom` keeps comparing by content, whatever built it: `custom`
    // allocates, `message_static` borrows, and the payload's type is irrelevant
    // because only its `Display` was used.
    assert_eq!(
        Error::message_static("bad frame 7"),
        Error::message(BadFrame(7))
    );
}

#[test]
fn test_wrapped_error_crosses_the_trait_boundary() {
    /// Encodes its payload, but refuses anything that is not 4 bytes.
    struct Frame(Vec<u8>);

    impl Layout for Frame {}

    impl Encode for Frame {
        fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
            check_frame(self.0.len())?; // BadFrame -> Error::Wrapped
            self.0.encode_auto(e)
        }
    }

    let mut e = Encoder::new();
    let err = Frame(vec![1, 2]).encode(&mut e).unwrap_err();

    assert!(matches!(err, Error::Wrapped(_)));
    assert_eq!(err.to_string(), "bad frame 2");

    // The caller can tell *which* domain error it was — exactly what the
    // `message` path in `test_degraded_error_cannot_be_recovered` loses.
    let src = std::error::Error::source(&err).unwrap();
    assert!(matches!(src.downcast_ref::<BadFrame>(), Some(BadFrame(2))));

    // And the happy path is untouched: 4 bytes go through the length prefix.
    let mut e = Encoder::new();
    Frame(vec![1, 2, 3, 4]).encode(&mut e).unwrap();
    assert_eq!(e.finish().unwrap(), [0x04, 1, 2, 3, 4]);
}

// --- variant semantics -------------------------------------------------------
//
// One test per rename that was about *meaning*, not spelling. Each one fails
// under the old name, which is why the old name was wrong (DESIGN §18.9).

#[test]
fn test_truncated_payload_is_not_a_short_read() {
    // (a) The header itself is cut: not even a 4-byte prefix can be completed.
    let buf = [0x80u8, 0x00];
    let mut d = Decoder::new(&buf);
    assert_eq!(
        d.with_prefix(|_| Ok(())).unwrap_err(),
        Error::NotEnoughBytes
    );

    // (b) The header is complete and declares 4 bytes; only 1 is present.
    let buf = [0x04u8, 0xAA];
    let mut d = Decoder::new(&buf);
    assert_eq!(
        d.with_prefix(|_| Ok(())).unwrap_err(),
        Error::PayloadTruncated
    );

    // Two different failures at the same wire position — the reason the old
    // `LengthPrefixMismatch` had to be split. A caller that retries on a short
    // read should not retry on a truncated payload.
}

#[test]
fn test_misaligned_fixed_size_elements() {
    // `Vec<i32>::decode` (raw) infers the count from the remaining bytes, since
    // `auto` already consumed the outer prefix. 6 bytes is not a whole number of
    // 4-byte elements, so there is no honest count to decode.
    let buf = [1u8, 0, 0, 0, 2, 0];
    let mut d = Decoder::new(&buf);
    assert_eq!(
        Vec::<i32>::decode(&mut d).unwrap_err(),
        Error::MisalignedElements
    );

    // 8 bytes is, and the same code path decodes it: the error is about the
    // payload shape, not about `Vec` being unsupported.
    let buf = [1u8, 0, 0, 0, 2, 0, 0, 0];
    let mut d = Decoder::new(&buf);
    assert_eq!(Vec::<i32>::decode(&mut d).unwrap(), vec![1, 2]);
}

#[test]
fn test_length_beyond_i32_max_is_reported_before_writing() {
    // The wire stores a length in a signed 31-bit field. Asking for one more
    // byte must fail *before* the reservation, otherwise the release build would
    // OR the length into 4 bytes with the marker bit, truncating it silently.
    let mut e = Encoder::new();
    let err = e
        .write_prefixed_bounded(i32::MAX as usize + 1, |_| Ok(()))
        .unwrap_err();
    assert_eq!(err, Error::LengthTooLarge);
    assert_eq!(e.len(), 0, "the guard runs before anything is written");
}

#[test]
fn test_compact_int_beyond_i32_max_is_rejected() {
    // Same 31-bit field as a length prefix. For `v > i32::MAX`, `v | 0x8000_0000`
    // keeps bit 31 as *data* while the decoder masks that bit away, so
    // `0x8000_0001` would read back as `1`. A release build used to reach only a
    // `debug_assert!` here, i.e. it wrote a silently wrong tag.
    let mut e = Encoder::new();
    assert_eq!(
        e.write_compact_int(i32::MAX as u32 + 1).unwrap_err(),
        Error::LengthTooLarge
    );
    assert_eq!(e.len(), 0, "the guard runs before anything is written");

    // The largest legal tag still round-trips, and it is the all-ones wide form.
    e.write_compact_int(i32::MAX as u32).unwrap();
    let bytes = e.finish().unwrap();
    assert_eq!(bytes, [0xFF, 0xFF, 0xFF, 0xFF]);
    let mut d = Decoder::new(&bytes);
    assert_eq!(d.read_compact_int().unwrap(), i32::MAX as u32);
}

#[test]
fn test_take_past_the_end_is_not_enough_bytes() {
    let mut d = Decoder::new(&[1, 2, 3]);
    // `n` is attacker-sized: `pos + n` used to overflow, which panicked on the
    // slice expression (release) or on the addition (debug) instead of failing.
    assert_eq!(d.take(usize::MAX), Err(Error::NotEnoughBytes));
    assert_eq!(d.remaining(), 3, "a failed take leaves the cursor alone");

    assert_eq!(d.take(4), Err(Error::NotEnoughBytes));
    assert_eq!(d.take(3).unwrap(), &[1, 2, 3]);
    assert_eq!(d.take(1), Err(Error::NotEnoughBytes));
}

#[test]
fn test_unknown_name_is_skipped_but_bad_index_is_an_error() {
    use binary::NamedMembers;

    // One member on the wire: name "ab", value 0x2A.
    let buf = [0x02u8, b'a', b'b', 0x01, 0x2A];

    // An unknown *name* is skipped for forward compatibility — not an error.
    let mut d = Decoder::new(&buf);
    let members = NamedMembers::<1>::parse(&mut d, |_| None).unwrap();
    assert!(members.get(0).is_none());

    // A `find` table that answers with an out-of-range index is an invariant
    // violation, and that is what `InvalidMemberIndex` names. It is *not* the
    // "unknown member" the old name claimed.
    let mut d = Decoder::new(&buf);
    let err = NamedMembers::<1>::parse(&mut d, |_| Some(1)).err().unwrap();
    assert_eq!(err, Error::InvalidMemberIndex);
}

#[test]
fn test_display_text_matches_the_variant_name() {
    // Naming rule 6: the name and its message are translations of each other,
    // so a reader holding one can find the other. These are the pairs that were
    // out of sync before the rename (`Overflow` printed "buffer overflow",
    // `LengthPrefixMismatch` printed "does not match", `UnknownMember` printed
    // "unknown member" — three names, three lies).
    let cases = [
        (Error::BufferFull, "buffer full"),
        (Error::NotEnoughBytes, "not enough bytes"),
        (Error::LengthTooLarge, "length prefix exceeds i32::MAX"),
        (Error::DuplicateMember, "duplicate named member"),
        (Error::InvalidMemberIndex, "invalid member index"),
        (Error::PayloadTruncated, "payload truncated"),
        (Error::MisalignedElements, "misaligned element payload"),
    ];
    for (err, text) in cases {
        assert_eq!(err.to_string(), text);
    }

    // The two payload-carrying unit-like variants append their payload.
    assert_eq!(Error::InvalidTag(9).to_string(), "invalid tag: 9");
    assert_eq!(
        Error::MissingMember("id").to_string(),
        "missing required member: id"
    );
}
