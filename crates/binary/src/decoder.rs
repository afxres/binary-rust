use crate::error::{Error, Result};
use crate::hex::Hex;
use crate::length::decode_prefix;
use core::fmt;

/// Decoder over a borrowed byte slice.
pub struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Create a decoder over the given bytes.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Remaining bytes from the current position.
    ///
    /// Also meaningful after a failed call: `take` leaves the cursor at the
    /// start of the request it could not satisfy, and `with_prefix` folds a
    /// nested failure back into the parent's coordinates, so this keeps counting
    /// from the byte that stopped decoding. The absolute offset is the caller's
    /// own arithmetic — `buf.len() - d.remaining()`.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    // No `len()` / `is_empty()` / `position()`: the first two would report the
    // input buffer, and a scoped decoder starts at 0, so an "absolute" offset
    // would be scope-relative anyway.

    /// Take `n` bytes from the current position, returning a slice with lifetime `'a`.
    /// This enables zero-copy decoding (`&'a str`, `&'a [u8]`).
    #[inline]
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        // Compare against the remaining length instead of the end offset:
        // `self.pos + n` overflows for a large `n`. `pos <= buf.len()` holds by
        // construction, so the subtraction cannot underflow.
        if n > self.buf.len() - self.pos {
            return Err(Error::NotEnoughBytes);
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// Take a single byte.
    #[inline]
    pub fn take_byte(&mut self) -> Result<u8> {
        let buf = self.buf;
        if self.pos >= buf.len() {
            return Err(Error::NotEnoughBytes);
        }
        let b = buf[self.pos];
        self.pos += 1;
        Ok(b)
    }

    /// Read a length prefix and return the payload slice.
    /// Advances `pos` past both the prefix and the payload.
    #[inline]
    pub fn read_prefix(&mut self) -> Result<&'a [u8]> {
        let buf = self.buf;
        let len = decode_prefix(buf, &mut self.pos)?;
        let start = self.pos;
        if start + len > buf.len() {
            return Err(Error::NotEnoughBytes);
        }
        let slice = &buf[start..start + len];
        self.pos += len;
        Ok(slice)
    }

    /// Decode within a length-prefixed scope.
    /// Reads the prefix, then constrains the decoder to the payload region.
    pub fn with_prefix<T>(&mut self, f: impl FnOnce(&mut Decoder<'a>) -> Result<T>) -> Result<T> {
        let buf = self.buf; // copy out the &'a [u8] reference
        let len = decode_prefix(buf, &mut self.pos)?;
        let start = self.pos;
        let end = start + len;
        if end > buf.len() {
            return Err(Error::PayloadTruncated);
        }

        let payload = &buf[start..end];

        let mut sub = Decoder {
            buf: payload,
            pos: 0,
        };

        match f(&mut sub) {
            Ok(value) => {
                self.pos = end;
                Ok(value)
            }
            Err(err) => {
                // `sub` numbers positions inside `payload`; fold its cursor back
                // into the parent's coordinates so `remaining()` keeps counting
                // from the byte that actually failed.
                self.pos = start + sub.pos;
                Err(err)
            }
        }
    }

    /// Read a *compact integer* (see [`Encoder::write_compact_int`]): 1 byte when the
    /// value is `<= 127`, otherwise 4 bytes big-endian with bit 31 set.
    ///
    /// [`Encoder::write_compact_int`]: crate::Encoder::write_compact_int
    #[inline]
    pub fn read_compact_int(&mut self) -> Result<u32> {
        let buf = self.buf;
        decode_prefix(buf, &mut self.pos).map(|n| n as u32)
    }

    /// Peek at the next byte without advancing.
    #[inline]
    pub fn peek_byte(&self) -> Result<u8> {
        if self.pos >= self.buf.len() {
            return Err(Error::NotEnoughBytes);
        }
        Ok(self.buf[self.pos])
    }
}

impl fmt::Debug for Decoder<'_> {
    /// Prints the cursor, what is left, and a capped hex dump of the unread
    /// bytes — the state that answers "why did this value fail to decode".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rest = &self.buf[self.pos..];
        f.debug_struct("Decoder")
            .field("buf_len", &self.buf.len())
            .field("pos", &self.pos)
            .field("remaining", &rest.len())
            .field("next", &Hex(rest))
            .finish()
    }
}
