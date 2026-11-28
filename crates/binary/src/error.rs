use alloc::borrow::Cow;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::fmt;

/// The concrete error of this crate.
///
/// Five variants, one per distinct failure a caller can act on. Three are
/// type-independent `Allocator` conditions: [`Error::BufferFull`] is
/// the only recoverable one (grow the buffer, retry the write), and
/// [`Error::NotEnoughBytes`] is the failure a decoder meets most often.
/// [`Error::LengthTooLarge`] guards the one predefined wire type, the length
/// prefix. Everything else is specific to some value or protocol shape, and a
/// specific error is not promoted to a variant — it travels as text or as a
/// wrapped foreign error, exactly like a caller's own failure. There is no
/// hierarchy of errors: once the three guards above have had their say, a
/// builtin condition and a user's condition are both just
/// [`Error::Message`] / [`Error::Wrapped`].
///
/// One closed enum instead of a generic `E`: there is exactly one wire format,
/// so a type parameter would be paid for at every call site and buy nothing,
/// while `PartialEq` + `Clone` are what let callers `assert_eq!` a variant.
///
/// A custom `Encode` / `Decode` impl cannot return its own error type, so it
/// degrades it at the trait boundary — cheapest first: [`Error::message_static`],
/// [`Error::message`], a local `From<MyError>` impl, or [`Error::wrapped`] when
/// the type and the `source()` chain have to survive.
#[derive(Debug, Clone)]
pub enum Error {
    /// Writing past the end of a caller-provided (borrowed) buffer.
    ///
    /// The write side of "is there room?"; `NotEnoughBytes` is the read side.
    /// Only `Allocator::on_slice` can produce it — an owned allocator grows instead.
    /// An estimate that turns out to be too small is **not** this error; see
    /// `Allocator::append_with_length_prefix_bounded`.
    BufferFull,
    /// A read needed more bytes than were available.
    ///
    /// Also the shape "the header was readable, but the payload it promised is
    /// not there" — one failure, not two: the split existed for streaming
    /// retry semantics this wire format does not promise, and a caller that
    /// cannot act on the distinction has no use for two variants of the same
    /// short read.
    NotEnoughBytes,
    /// A length prefix declared more than `i32::MAX` bytes.
    ///
    /// The wire stores a length in a signed 31-bit field, so 2 GiB is a hard
    /// ceiling.
    LengthTooLarge,
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
    /// original value back, and matching the variant hands out the `Arc`
    /// itself — cloneable, downcastable, usable in a caller's own error enum.
    /// Cost: one `Arc` allocation (none if the caller already holds the
    /// payload in an `Arc` and constructs the variant directly), and equality
    /// on this variant is textual — there is no type id to compare, so two
    /// wrappers are equal when their `Display` output is. `Arc` rather than
    /// `Box` keeps `Clone` a refcount bump. The `Send + Sync + 'static` bound
    /// keeps `Error` `Send + Sync`; a non-`Send` source has to be degraded
    /// with [`Error::message`].
    Wrapped(Arc<dyn core::error::Error + Send + Sync + 'static>),
}

impl Error {
    /// Degrade any `Display` error into [`Error::Message`], allocating.
    ///
    /// This is the default bridge for a custom `Encode` / `Decode` impl, and the
    /// cheapest one that still carries runtime detail:
    ///
    /// ```
    /// use binary::{Encode, Allocator, Error, Layout, Result};
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
    ///     fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
    ///         check_count(self.0)?; // OutOfRange -> Error, via the `From` impl
    ///         e.append_byte(self.0)
    ///     }
    /// }
    ///
    /// let mut e = Allocator::new();
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
    /// A custom impl sees only the payload slice; the honest thing it can
    /// report is *how far into the value* it gave up, measured against the
    /// slice length it received.
    /// ```
    /// # use binary::{Decode, Error, Layout, Result};
    /// # struct Count(u8);
    /// # impl Layout for Count {}
    /// impl<'de> Decode<'de> for Count {
    ///     fn decode(payload: &'de [u8]) -> Result<Self> {
    ///         let start = payload.len();
    ///         let (&n, rest) = payload.split_first()
    ///             .ok_or(Error::NotEnoughBytes)?;
    ///         if n > 4 {
    ///             return Err(Error::message(format_args!(
    ///                 "count {n} out of range {} byte(s) into the value",
    ///                 start - rest.len()
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
    /// Costs one `Arc` allocation and makes equality textual for this variant,
    /// which is why [`Error::message`] remains the
    /// default; reach for `wrapped` when the caller has to act on the *kind* of
    /// failure, not just print it. The payload must be `Send + Sync + 'static`
    /// so that `Error` itself stays `Send + Sync`.
    ///
    /// ```
    /// use binary::{Encode, Allocator, Error, Layout, Result};
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
    ///     fn encode(&self, e: &mut Allocator<'_>) -> Result<()> {
    ///         if self.0 > 4 {
    ///             return Err(OutOfRange(self.0).into());
    ///         }
    ///         e.append_byte(self.0)
    ///     }
    /// }
    ///
    /// let mut e = Allocator::new();
    /// let err = Count(9).encode(&mut e).unwrap_err();
    /// assert_eq!(err.to_string(), "count 9 out of range");
    ///
    /// // The type survived the erasure, so a caller can branch on it.
    /// let src = std::error::Error::source(&err).unwrap();
    /// assert_eq!(src.downcast_ref::<OutOfRange>().unwrap().0, 9);
    /// ```
    pub fn wrapped(err: impl core::error::Error + Send + Sync + 'static) -> Self {
        Error::Wrapped(Arc::new(err))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BufferFull => write!(f, "buffer full"),
            Error::NotEnoughBytes => write!(f, "not enough bytes"),
            Error::LengthTooLarge => write!(f, "length prefix exceeds i32::MAX"),
            Error::Message(msg) => write!(f, "{msg}"),
            // The payload's own `Display`; no extra context is invented, so
            // `to_string()` on the wrapper and on the payload agree.
            Error::Wrapped(err) => write!(f, "{err}"),
        }
    }
}

// Hand-written because the trait object cannot derive `PartialEq`: variants
// compare by their payload, and the erased one falls back to its `Display`
// text — there is no type id to compare, so two wrappers printing the same
// text are equal. Accepted consequence: unrelated payload types that print
// identically compare equal. Variants never cross, whatever the text says.
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::BufferFull, Self::BufferFull)
            | (Self::NotEnoughBytes, Self::NotEnoughBytes)
            | (Self::LengthTooLarge, Self::LengthTooLarge) => true,
            (Self::Message(a), Self::Message(b)) => a == b,
            (Self::Wrapped(a), Self::Wrapped(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

impl Eq for Error {}

// `core::error::Error`, not `std::error::Error`: since Rust 1.81 the latter is a
// re-export, so one impl covers `std` and `--no-default-features` alike.
impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Error::Wrapped(err) => Some(&**err),
            _ => None,
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;
