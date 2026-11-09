use crate::error::{Error, Result};
use crate::length::{encode_prefix_wide, prefix_header_len};
use alloc::vec::Vec;
use core::fmt;

use crate::hex::Hex;

/// Payload length up to which the `write_prefixed` path compacts its 4-byte
/// header into a 1-byte one. Above this the 4-byte header is kept as-is.
///
/// Not a free parameter — C# and C++ use the same limit — and deliberately not
/// `pub`: both widths are valid wire, so publishing it would turn a tuning
/// decision into a semver commitment.
pub(crate) const PREFIX_SHRINK_LIMIT: usize = 16;

/// How many bytes an owned encoder commits (zero-fills) up front, capped by the
/// capacity the caller asked for.
///
/// `Vec::len` is a *watermark*, not the write position: only `pos` says what has
/// been written, so the committed tail never reaches the wire, and the common
/// single-object payload never hits `Vec::resize` from inside the write path.
/// Doing the same growth inside `ensure_capacity` instead deoptimized the
/// borrowed per-element loop (§19.15.13).
///
/// 128 covers the shapes the benchmark set uses, and is small enough that a tiny
/// payload does not pay much for it.
const COMMIT_AHEAD: usize = 128;

/// Payloads this long go through `Vec::extend_from_slice` on the owned backend
/// instead of `Vec::resize` + `memcpy`, which zero-fills the bytes the copy is
/// about to overwrite. Below this the zero-fill costs less than the branch
/// (§19.15.17).
const BULK_WRITE_MIN: usize = 256;

/// One fixed-size `u64` byte move.
///
/// Loads the whole word into a value *before* storing it, which is what makes
/// the two overlapping 8-byte blocks of the prefix compaction safe. The value is
/// never interpreted numerically, so this is endian-independent.
///
/// # Safety
///
/// Callers must prove `src + 8` and `dst + 8` are inside `slice` (done by
/// `Encoder::ensure_prefix_tail`). No alignment is required.
#[inline]
unsafe fn move_u64(slice: &mut [u8], src: usize, dst: usize) {
    let ptr = slice.as_mut_ptr();
    let word = core::ptr::read_unaligned(ptr.add(src).cast::<u64>());
    core::ptr::write_unaligned(ptr.add(dst).cast::<u64>(), word);
}

/// Single concrete encoder type with two backends.
///
/// - `Owned`: self-growing `Vec<u8>`
/// - `Borrowed`: writes into caller-provided `&mut [u8]`, error on overflow
pub struct Encoder<'a> {
    inner: Inner<'a>,
    pos: usize,
}

enum Inner<'a> {
    Owned(Vec<u8>),
    Borrowed(&'a mut [u8]),
}

impl<'a> Encoder<'a> {
    /// Create an owned encoder with a small initial capacity.
    ///
    /// No `impl Default` on purpose: the one pattern that would need it
    /// (`mem::take` on a scratch encoder) is ruled out by `finish` consuming the
    /// buffer, and adding it later is non-breaking — hence the silenced lint.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::with_capacity(256)
    }

    /// Create an owned encoder with an initial capacity of `n` bytes.
    ///
    /// `n` is a budget the caller estimates, not a promise: writes past it still
    /// succeed, growing the `Vec` to what is needed. Construction is the only
    /// place a caller can avoid that growth — there is no per-value reservation
    /// API and no growth heuristic inside `ensure_capacity` (§19.15.15). The
    /// estimate must be an *upper* bound, including every length prefix; when
    /// the size is known exactly, `on_slice` skips the allocation altogether.
    ///
    /// The first `COMMIT_AHEAD` bytes (or `n`, if smaller) are committed here.
    /// They sit outside `pos`, so they never enter the result.
    pub fn with_capacity(n: usize) -> Self {
        let mut v = Vec::with_capacity(n);
        v.resize(n.min(COMMIT_AHEAD), 0);
        Self {
            inner: Inner::Owned(v),
            pos: 0,
        }
    }

    /// Create a borrowed encoder on a caller-provided slice.
    /// Writes beyond the slice length return `Err(Error::BufferFull)`.
    pub fn on_slice(buf: &'a mut [u8]) -> Self {
        Self {
            inner: Inner::Borrowed(buf),
            pos: 0,
        }
    }

    /// Number of bytes written so far.
    ///
    /// The only way to read the result of a **borrowed** encoder (`on_slice`),
    /// where `finish` is unavailable. Deliberately the only such accessor:
    /// `is_empty` is not coming back, so the lint is silenced rather than
    /// answered with a second method.
    #[allow(clippy::len_without_is_empty)]
    #[inline]
    pub fn len(&self) -> usize {
        self.pos
    }

    /// Write raw bytes.
    #[inline]
    pub fn write(&mut self, src: &[u8]) -> Result<()> {
        // Bulk payloads append into uninitialised spare capacity; the generic
        // path below grows with `Vec::resize`, i.e. zero-fills the bytes the
        // copy is about to overwrite (§19.15.17). Small writes keep the plain
        // shape: there the zero-fill is a few bytes, and the extra branch is not
        // free on the per-scalar path.
        if src.len() >= BULK_WRITE_MIN {
            if let Inner::Owned(v) = &mut self.inner {
                let at = self.pos;
                let committed = v.len();
                if at < committed {
                    let head = (committed - at).min(src.len());
                    v[at..at + head].copy_from_slice(&src[..head]);
                    v.extend_from_slice(&src[head..]);
                } else {
                    v.extend_from_slice(src);
                }
                self.pos = at + src.len();
                return Ok(());
            }
        }
        self.ensure_capacity(src.len())?;
        let pos = self.pos;
        let dst = self.as_mut_slice();
        dst[pos..pos + src.len()].copy_from_slice(src);
        self.pos += src.len();
        Ok(())
    }

    /// Write `[prefix(len)] [bytes]`, reserving both with a single capacity check.
    ///
    /// Used by member names, and by values whose length is already exact —
    /// `String` / `str` / `[u8]`. `write_prefixed_bounded` would re-enter
    /// `ensure_capacity` and re-read `pos` around the payload, which is ~2.8 ns
    /// per member on a short string (§19.15.11).
    ///
    /// The 1-byte-header arm is the only one inlined; `>127` goes out of line,
    /// which measured faster where this is inlined into every write site.
    #[inline]
    pub(crate) fn write_prefixed_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let at = self.pos;
        let len = bytes.len();
        if len <= crate::length::SINGLE_BYTE_MAX {
            self.ensure_capacity(1 + len)?;
            let slice = self.as_mut_slice();
            slice[at] = len as u8;
            slice[at + 1..at + 1 + len].copy_from_slice(bytes);
            self.pos = at + 1 + len;
            return Ok(());
        }
        self.write_prefixed_bytes_wide(at, bytes)
    }

    /// The >127-byte arm of [`Encoder::write_prefixed_bytes`]: four-byte
    /// big-endian header with bit 31 set, width fixed even when the length would
    /// fit in one byte (the caller decided the width from the same `len`).
    #[cold]
    #[inline(never)]
    fn write_prefixed_bytes_wide(&mut self, at: usize, bytes: &[u8]) -> Result<()> {
        // Header first, payload through the bulk path: a 4-byte reservation is
        // worth a zero-fill, the payload is not (§19.15.17).
        self.ensure_capacity(4)?;
        encode_prefix_wide(&mut self.as_mut_slice()[at..at + 4], bytes.len())?;
        self.pos = at + 4;
        self.write(bytes)
    }

    /// Ensure at least `additional` bytes are available at the current position.
    #[inline]
    fn ensure_capacity(&mut self, additional: usize) -> Result<()> {
        match &mut self.inner {
            Inner::Owned(v) => {
                let needed = self.pos + additional;
                if v.len() < needed {
                    v.resize(needed, 0);
                }
                Ok(())
            }
            Inner::Borrowed(buf) => {
                if self.pos + additional > buf.len() {
                    Err(Error::BufferFull)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Make `[.., need)` initialized and writable for the fixed-size compaction,
    /// returning whether the compaction can run.
    ///
    /// `Owned` grows by zero-filling the tail; those bytes sit outside `pos` and
    /// never enter the result. `Borrowed` cannot grow, so the caller's spare
    /// capacity decides. Sizing the buffer up front keeps this on the fast path.
    #[inline]
    fn ensure_prefix_tail(&mut self, need: usize) -> bool {
        match &mut self.inner {
            Inner::Owned(v) => {
                if v.len() < need {
                    v.resize(need, 0);
                }
                true
            }
            Inner::Borrowed(buf) => buf.len() >= need,
        }
    }

    /// Get a mutable view of the backing buffer up to its current allocated length.
    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        match &mut self.inner {
            Inner::Owned(v) => &mut v[..],
            Inner::Borrowed(buf) => buf,
        }
    }

    /// Write `prefix + payload` when an O(1) upper bound on the payload length
    /// is available. The header width comes from `max_len` and stays fixed; the
    /// actual byte count is written back into it afterwards, so there is no
    /// memmove. `f` may write fewer than `max_len` bytes.
    ///
    /// Unlike `write_prefixed`, this needs an estimate rather than the exact
    /// length. `max_len` is a bound, not a reservation: `f` writes into **this**
    /// encoder right after the header, so an over-estimate costs no memory and a
    /// borrowed encoder is charged only for the bytes actually written. Safety
    /// needs no bounded window, because every write inside `f` goes through
    /// `ensure_capacity`, which either grows the owned tail or stops at the
    /// caller's slice end with [`Error::BufferFull`].
    ///
    /// # Errors
    ///
    /// [`Error::LengthTooLarge`] when `max_len` does not fit the wire's signed
    /// 31-bit field, checked *before* anything is written — a truncated prefix
    /// would be corrupt wire with no error. [`Error::Message`] when `f` writes
    /// more than `max_len`: a caller bug, not bad data. Every write failure here
    /// is terminal — the header is left unfilled, so the bytes after the anchor
    /// are not a member. Rolling `pos` back was measured at ~2 ns per call
    /// (§19.15.10).
    pub fn write_prefixed_bounded(
        &mut self,
        max_len: usize,
        f: impl FnOnce(&mut Encoder<'_>) -> Result<()>,
    ) -> Result<()> {
        // Out of line: the message must not join the hot path, and an estimate
        // smaller than the payload is a caller bug rather than data.
        #[cold]
        #[inline(never)]
        fn overshoot() -> Error {
            Error::message_static(
                "payload exceeds the length bound given to write_prefixed_bounded",
            )
        }

        if max_len > crate::length::MAX_LENGTH {
            return Err(Error::LengthTooLarge);
        }
        let h = prefix_header_len(max_len);
        let at = self.pos;
        // Header only: reserving `max_len` would charge an upper bound to an
        // owned buffer (zero-fill) and to a borrowed one (false `BufferFull`).
        self.ensure_capacity(h)?;
        self.pos = at + h;

        // Reborrow, not a wrapper: `f` gets this encoder and its backend.
        f(&mut *self)?;

        let actual = self.pos - at - h;
        if actual > max_len {
            return Err(overshoot());
        }

        let slice = self.as_mut_slice();
        if h == 1 {
            // `actual <= max_len <= 127`, so the narrow header always fits.
            slice[at] = actual as u8;
        } else {
            // Header width stays 4 even when `actual <= 127`.
            encode_prefix_wide(&mut slice[at..at + 4], actual)?;
        }
        Ok(())
    }

    /// Fallback for values with no O(1) length estimate: reserve the maximum
    /// 4-byte header, write the payload, then compact the header to 1 byte
    /// **only when the payload is short enough that the shift is cheap**.
    ///
    /// The payload is moved with `(length + 7) >> 3` fixed-size `u64` moves (one
    /// or two), not a runtime-length `memmove`, for a bounded latency
    /// distribution; the price is a tail margin of `(-length) & 7` bytes past
    /// the payload. Size the buffer up front so the owned backend never has to
    /// reallocate here.
    ///
    /// When the payload is longer than the threshold, or a borrowed buffer cannot
    /// supply that margin, the 4-byte header is kept — one arm, not two, like C#
    /// `Allocator.FinishAnchor`. Both widths are valid wire, so neither case is
    /// an error. The threshold is `PREFIX_SHRINK_LIMIT`.
    pub fn write_prefixed(&mut self, f: impl FnOnce(&mut Encoder<'_>) -> Result<()>) -> Result<()> {
        let anchor = self.pos;

        // Reserve 4 bytes (max possible prefix) as placeholder. Zeroing it is
        // unnecessary: the short path overwrites byte 0 and the move covers
        // 1..4, while the wide path is fully overwritten by encode_prefix_wide.
        self.ensure_capacity(4)?;
        self.pos = anchor + 4;

        // Write payload
        f(self)?;

        let actual = self.pos - anchor - 4;
        // Release-safe counterpart of the `write_prefixed_bounded` guard: the
        // payload is already written, so the encoder is left in an unspecified
        // state. Every caller treats an error as terminal.
        if actual > crate::length::MAX_LENGTH {
            return Err(Error::LengthTooLarge);
        }

        // One decision, the shape of C# `Allocator.FinishAnchor` / C++
        // `Allocator::FinishAnchor`: shrink only when the payload is short *and*
        // the tail margin is there, otherwise fill the 4-byte header already in
        // place. Keeping the two "do not shrink" reasons in one arm means a
        // single `encode_prefix_wide` call.
        let pad = (8 - (actual & 7)) & 7;
        if actual <= PREFIX_SHRINK_LIMIT && self.ensure_prefix_tail(anchor + 4 + actual + pad) {
            let header = (actual + 7) >> 3;
            let slice = self.as_mut_slice();
            // `anchor + 4 + actual + pad == anchor + 4 + header * 8` is the end
            // of the last 8-byte block; both blocks read and write strictly
            // inside it.
            slice[anchor] = actual as u8;
            // SAFETY: `ensure_prefix_tail` proved the bound above; both 8-byte
            // blocks read and write strictly inside it, and `move_u64` needs no
            // alignment.
            unsafe {
                if header != 0 {
                    move_u64(slice, anchor + 4, anchor + 1);
                }
                if header == 2 {
                    move_u64(slice, anchor + 12, anchor + 9);
                }
            }
            self.pos -= 3;
        } else {
            // 17..=127 keeps the 4-byte header too; the width must be forced
            // (§12.4), so this cannot be the narrow write used above.
            let slice = self.as_mut_slice();
            encode_prefix_wide(&mut slice[anchor..anchor + 4], actual)?;
        }

        Ok(())
    }

    /// Consume the encoder and return the written bytes.
    /// Only available for owned encoders.
    pub fn finish(self) -> Result<Vec<u8>> {
        match self.inner {
            Inner::Owned(mut v) => {
                v.truncate(self.pos);
                Ok(v)
            }
            Inner::Borrowed(_) => Err(Error::message_static("cannot finish a borrowed encoder")),
        }
    }

    /// Get a reference to the written bytes without consuming.
    pub fn as_bytes(&self) -> &[u8] {
        match &self.inner {
            Inner::Owned(v) => &v[..self.pos],
            Inner::Borrowed(buf) => &buf[..self.pos],
        }
    }

    /// Write a *compact integer*: 1 byte when `v <= 127`, otherwise 4 bytes
    /// big-endian with bit 31 set.
    ///
    /// Wire-identical to a length prefix by design — the same bytes a prefix of
    /// that length would produce. Used for union / variant tags, so a tag stays
    /// cheap for the first 128 variants and still covers `i32::MAX` variants.
    ///
    /// Directly comparable with [`crate::Encode`] for `usize`: a `usize` writes
    /// 8 fixed bytes, a compact integer writes 1 or 4.
    #[inline]
    pub fn write_compact_int(&mut self, v: u32) -> Result<()> {
        // Release-safe guard, same class as `write_prefixed_bounded`:
        // `v | 0x8000_0000` keeps bit 31 as *data* for values above `i32::MAX`,
        // and the decoder masks that bit away, so the value would change
        // silently. Nothing is written before the check.
        if v as usize > crate::length::MAX_LENGTH {
            return Err(Error::LengthTooLarge);
        }
        let h = prefix_header_len(v as usize);
        self.ensure_capacity(h)?;
        let pos = self.pos;
        {
            let slice = self.as_mut_slice();
            if h == 1 {
                slice[pos] = v as u8;
            } else {
                encode_prefix_wide(&mut slice[pos..pos + 4], v as usize)?;
            }
        }
        self.pos = pos + h;
        Ok(())
    }

    /// Write a single byte.
    #[inline]
    pub fn write_byte(&mut self, b: u8) -> Result<()> {
        self.ensure_capacity(1)?;
        let pos = self.pos;
        self.as_mut_slice()[pos] = b;
        self.pos += 1;
        Ok(())
    }
}

impl fmt::Debug for Encoder<'_> {
    /// Prints the backend, how much is written, and a capped hex dump of the
    /// bytes so far.
    ///
    /// Derived `Debug` would dump the whole `Vec<u8>` — including the reserved
    /// tail that is not part of the result.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Encoder");
        match &self.inner {
            Inner::Owned(v) => {
                s.field("backend", &"owned");
                s.field("capacity", &v.capacity());
            }
            Inner::Borrowed(buf) => {
                s.field("backend", &"borrowed");
                s.field("capacity", &buf.len());
            }
        }
        s.field("len", &self.pos)
            .field("bytes", &Hex(self.as_bytes()))
            .finish()
    }
}
