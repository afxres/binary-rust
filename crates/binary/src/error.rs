use alloc::borrow::Cow;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::fmt;

/// Type-erased foreign error, carried by [`Error::Wrapped`].
///
/// A newtype rather than a bare `Arc<dyn Error>`, because the latter cannot be
/// compared and `Error` must stay `PartialEq`. Equality here is textual (see the
/// `eq` impl); `Arc` rather than `Box` keeps `Clone` a refcount bump. The
/// `Send + Sync + 'static` bound keeps `Error` `Send + Sync`; a non-`Send`
/// source has to be degraded with [`Error::message`].
#[derive(Debug, Clone)]
pub struct ErasedError(Arc<dyn core::error::Error + Send + Sync + 'static>);

impl ErasedError {
    /// Take ownership of any error, erasing its type.
    ///
    /// This always allocates once.
    #[inline]
    pub fn new(err: impl core::error::Error + Send + Sync + 'static) -> Self {
        Self(Arc::new(err))
    }

    /// Borrow the erased error — the handle for `downcast_ref`.
    ///
    /// ```
    /// use binary::Error;
    ///
    /// #[derive(Debug)]
    /// struct OutOfRange(u8);
    ///
    /// impl std::fmt::Display for OutOfRange {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "count {} out of range", self.0)
    ///     }
    /// }
    ///
    /// impl std::error::Error for OutOfRange {}
    ///
    /// let err = Error::wrapped(OutOfRange(9));
    /// let src = std::error::Error::source(&err).unwrap();
    /// assert_eq!(src.downcast_ref::<OutOfRange>().unwrap().0, 9);
    /// ```
    #[inline]
    pub fn inner(&self) -> &(dyn core::error::Error + Send + Sync + 'static) {
        &*self.0
    }
}

impl fmt::Display for ErasedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.0, f)
    }
}

impl PartialEq for ErasedError {
    /// Textual equality: two erasures are equal when their `Display` output is.
    ///
    /// There is nothing else to compare — `dyn core::error::Error` exposes no
    /// type id. Accepted consequence: two unrelated payloads that print the same
    /// text are equal here.
    fn eq(&self, other: &Self) -> bool {
        self.0.to_string() == other.0.to_string()
    }
}

impl Eq for ErasedError {}

// Delegating the chain keeps a wrapping error's `source()` walkable one level
// below ours: `outer -> ErasedError -> payload -> payload.source()`.
impl core::error::Error for ErasedError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.inner())
    }
}

/// The concrete error of this crate.
///
/// Naming rules live in DESIGN §18.9 and are enforced by review. The short
/// version: say the symptom, not the guess (`BufferFull`, not `Overflow`); one
/// variant per distinct failure; and the name and its `Display` text must be
/// translations of each other.
///
/// One closed enum instead of a generic `E`: there is exactly one wire format,
/// so a type parameter would be paid for at every call site and buy nothing,
/// while `PartialEq` + `Clone` are what let callers `assert_eq!` a variant.
///
/// A custom `Encode` / `Decode` impl cannot return its own error type, so it
/// degrades it at the trait boundary — cheapest first: [`Error::message_static`],
/// [`Error::message`], a local `From<MyError>` impl, or [`Error::wrapped`] when
/// the type and the `source()` chain have to survive.
///
/// `#[non_exhaustive]` keeps variant additions minor for a downstream `match`.
/// It buys only that: renames and removals still break callers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Writing past the end of a caller-provided (borrowed) buffer.
    ///
    /// The write side of "is there room?"; `NotEnoughBytes` is the read side.
    /// Only `Encoder::on_slice` can produce it — an owned encoder grows instead.
    /// An estimate that turns out to be too small is **not** this error; see
    /// `Encoder::write_prefixed_bounded`.
    BufferFull,
    /// A read needed more bytes than were available.
    NotEnoughBytes,
    /// A length prefix declared more than `i32::MAX` bytes.
    ///
    /// The wire stores a length in a signed 31-bit field, so 2 GiB is a hard
    /// ceiling.
    LengthTooLarge,
    /// A required named member did not appear on the wire.
    MissingMember(&'static str),
    /// The same named member appeared twice.
    DuplicateMember,
    /// A name lookup returned an index outside the declared member table.
    ///
    /// An invariant violation, not bad input: the generated `find` table and the
    /// constant `N` disagree. A genuinely unknown name is **not** an error —
    /// `NamedMembers::parse` skips it for forward compatibility.
    InvalidMemberIndex,
    /// A tag / discriminant that does not name a declared variant.
    ///
    /// Used by `Option` (tags 0 and 1) and by `#[binary(union)]` (variant
    /// indices).
    InvalidTag(u32),
    /// A length prefix declared more payload bytes than the buffer holds.
    ///
    /// The *header* was readable; the payload it promised was not. Compare
    /// `NotEnoughBytes`, which means the read ran out before a header.
    PayloadTruncated,
    /// The payload length is not a multiple of the fixed element size.
    ///
    /// Produced when decoding a prefix-less sequence of fixed-size elements: the
    /// count is inferred from the remaining bytes, so a remainder means the
    /// payload cannot be a whole number of elements.
    MisalignedElements,
    /// A foreign failure, degraded to a message — the text channel.
    ///
    /// `Cow::Borrowed` keeps the allocation-free path for static strings;
    /// [`Error::message`] produces `Cow::Owned`. Not called `Custom` because a
    /// foreign error is not "less built-in", it merely arrived as text.
    Message(Cow<'static, str>),
    /// A foreign failure, kept type-erased — the type channel.
    ///
    /// Built by [`Error::wrapped`]. Unlike [`Error::Message`], the payload keeps
    /// its type: `source()` chains through it and `downcast_ref` gets the
    /// original value back. Cost: one `Arc` allocation, and equality on this
    /// variant is textual (see [`ErasedError`]).
    Wrapped(ErasedError),
}

impl Error {
    /// Degrade any `Display` error into [`Error::Message`], allocating.
    ///
    /// This is the default bridge for a custom `Encode` / `Decode` impl, and the
    /// cheapest one that still carries runtime detail:
    ///
    /// ```
    /// use binary::{Encode, Encoder, Error, Layout, Result};
    ///
    /// #[derive(Debug)]
    /// struct OutOfRange(u8);
    ///
    /// impl std::fmt::Display for OutOfRange {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "count {} out of range", self.0)
    ///     }
    /// }
    ///
    /// // Legal: `OutOfRange` is local, so the orphan rule allows this.
    /// impl From<OutOfRange> for Error {
    ///     fn from(e: OutOfRange) -> Self {
    ///         Error::message(e)
    ///     }
    /// }
    ///
    /// fn check_count(n: u8) -> std::result::Result<(), OutOfRange> {
    ///     if n > 4 { Err(OutOfRange(n)) } else { Ok(()) }
    /// }
    ///
    /// struct Count(u8);
    ///
    /// impl Layout for Count {}
    ///
    /// impl Encode for Count {
    ///     fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
    ///         check_count(self.0)?; // OutOfRange -> Error, via the `From` impl
    ///         e.write_byte(self.0)
    ///     }
    /// }
    ///
    /// let mut e = Encoder::new();
    /// assert_eq!(
    ///     Count(9).encode(&mut e).unwrap_err().to_string(),
    ///     "count 9 out of range"
    /// );
    /// ```
    ///
    /// The original error's type and `source()` chain are dropped; what survives
    /// is the `Display` text. When the type must be recoverable, use
    /// [`Error::wrapped`]; when no runtime detail is needed, use
    /// [`Error::message_static`].
    ///
    /// # Adding a position
    ///
    /// `Decoder` has no absolute offset (see [`crate::Decoder::remaining`]); a
    /// custom impl can still report *how far into the value* it gave up, by
    /// sampling `remaining()` on entry.
    /// ```
    /// # use binary::{Decode, Decoder, Error, Layout, Result};
    /// # struct Count(u8);
    /// # impl Layout for Count {}
    /// impl<'de> Decode<'de> for Count {
    ///     fn decode(d: &mut Decoder<'de>) -> Result<Self> {
    ///         let start = d.remaining();
    ///         let n = d.take_byte()?;
    ///         if n > 4 {
    ///             return Err(Error::message(format_args!(
    ///                 "count {n} out of range {} byte(s) into the value",
    ///                 start - d.remaining()
    ///             )));
    ///         }
    ///         Ok(Count(n))
    ///     }
    /// }
    /// ```
    pub fn message(msg: impl fmt::Display) -> Self {
        Error::Message(Cow::Owned(format!("{msg}")))
    }

    /// Degrade a static message without allocating.
    ///
    /// Equivalent to `Error::Message(Cow::Borrowed(msg))`; used by the crate's
    /// own impls and by generated code.
    #[inline]
    pub const fn message_static(msg: &'static str) -> Self {
        Error::Message(Cow::Borrowed(msg))
    }

    /// Keep a foreign error, type-erased, so `source()` and `downcast_ref` still
    /// reach it.
    ///
    /// Costs one `Arc` allocation and makes equality textual for this variant
    /// (see [`ErasedError`]), which is why [`Error::message`] remains the
    /// default; reach for `wrapped` when the caller has to act on the *kind* of
    /// failure, not just print it. The payload must be `Send + Sync + 'static`
    /// so that `Error` itself stays `Send + Sync`.
    ///
    /// ```
    /// use binary::{Encode, Encoder, Error, Layout, Result};
    ///
    /// #[derive(Debug)]
    /// struct OutOfRange(u8);
    ///
    /// impl std::fmt::Display for OutOfRange {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "count {} out of range", self.0)
    ///     }
    /// }
    ///
    /// impl std::error::Error for OutOfRange {}
    ///
    /// impl From<OutOfRange> for Error {
    ///     fn from(e: OutOfRange) -> Self {
    ///         Error::wrapped(e) // keeps the type, unlike `Error::message`
    ///     }
    /// }
    ///
    /// struct Count(u8);
    ///
    /// impl Layout for Count {}
    ///
    /// impl Encode for Count {
    ///     fn encode(&self, e: &mut Encoder<'_>) -> Result<()> {
    ///         if self.0 > 4 {
    ///             return Err(OutOfRange(self.0).into());
    ///         }
    ///         e.write_byte(self.0)
    ///     }
    /// }
    ///
    /// let mut e = Encoder::new();
    /// let err = Count(9).encode(&mut e).unwrap_err();
    /// assert_eq!(err.to_string(), "count 9 out of range");
    ///
    /// // The type survived the erasure, so a caller can branch on it.
    /// let src = std::error::Error::source(&err).unwrap();
    /// assert_eq!(src.downcast_ref::<OutOfRange>().unwrap().0, 9);
    /// ```
    pub fn wrapped(err: impl core::error::Error + Send + Sync + 'static) -> Self {
        Error::Wrapped(ErasedError::new(err))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BufferFull => write!(f, "buffer full"),
            Error::NotEnoughBytes => write!(f, "not enough bytes"),
            Error::LengthTooLarge => write!(f, "length prefix exceeds i32::MAX"),
            Error::MissingMember(name) => write!(f, "missing required member: {name}"),
            Error::DuplicateMember => write!(f, "duplicate named member"),
            Error::InvalidMemberIndex => write!(f, "invalid member index"),
            Error::InvalidTag(tag) => write!(f, "invalid tag: {tag}"),
            Error::PayloadTruncated => write!(f, "payload truncated"),
            Error::MisalignedElements => write!(f, "misaligned element payload"),
            Error::Message(msg) => write!(f, "{msg}"),
            // The payload's own `Display`; no extra context is invented, so
            // `to_string()` on the wrapper and on the payload agree.
            Error::Wrapped(err) => write!(f, "{err}"),
        }
    }
}

// `core::error::Error`, not `std::error::Error`: since Rust 1.81 the latter is a
// re-export, so one impl covers `std` and `--no-default-features` alike.
impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Error::Wrapped(err) => Some(err.inner()),
            _ => None,
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
