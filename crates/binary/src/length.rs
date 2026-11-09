use crate::error::{Error, Result};

/// Maximum length expressible in a single 1-byte prefix.
pub const SINGLE_BYTE_MAX: usize = 0x7F;

/// Maximum length expressible in a 4-byte prefix (signed 31-bit).
pub const MAX_LENGTH: usize = i32::MAX as usize;

/// Returns the number of bytes needed to encode `len` as a length prefix.
///
/// - `len <= 127` → 1 byte
/// - `len > 127` → 4 bytes (big-endian, bit 31 set as marker)
#[inline]
pub const fn prefix_header_len(len: usize) -> usize {
    if len <= SINGLE_BYTE_MAX {
        1
    } else {
        4
    }
}

/// Writes a length prefix into `buf`. Returns the number of bytes written (1 or 4).
///
/// Both failure modes are values: `len > i32::MAX` is [`Error::LengthTooLarge`]
/// (checked before any byte is written) and a too-small `buf` is
/// [`Error::BufferFull`]. Callers that reserved `prefix_header_len(len)` bytes
/// can still `?` the result.
#[inline]
pub fn encode_prefix(buf: &mut [u8], len: usize) -> Result<usize> {
    if len > MAX_LENGTH {
        return Err(Error::LengthTooLarge);
    }
    if len <= SINGLE_BYTE_MAX {
        if buf.is_empty() {
            return Err(Error::BufferFull);
        }
        buf[0] = len as u8;
        Ok(1)
    } else {
        encode_prefix_wide(buf, len)
    }
}

/// Writes a length as a 4-byte prefix **unconditionally**, even when `len` would
/// fit in one byte. Returns 4.
///
/// Needed once a 4-byte placeholder has been reserved and the payload can no
/// longer be shifted: a 1-byte prefix would leave three stale bytes in front of
/// the payload.
#[inline]
pub fn encode_prefix_wide(buf: &mut [u8], len: usize) -> Result<usize> {
    if len > MAX_LENGTH {
        return Err(Error::LengthTooLarge);
    }
    if buf.len() < 4 {
        return Err(Error::BufferFull);
    }
    let v = (len as u32) | 0x8000_0000;
    buf[..4].copy_from_slice(&v.to_be_bytes());
    Ok(4)
}

/// Reads a length prefix from `buf` starting at `pos`.
/// Returns the decoded length and advances `pos` past the prefix.
///
/// Bit 7 of the first byte selects the width: clear → 1 byte, set → 4 bytes
/// big-endian with the high bit masked off.
#[inline]
pub fn decode_prefix(buf: &[u8], pos: &mut usize) -> Result<usize> {
    if *pos >= buf.len() {
        return Err(Error::NotEnoughBytes);
    }
    let first = buf[*pos];
    if first & 0x80 == 0 {
        *pos += 1;
        Ok(first as usize)
    } else {
        if *pos + 4 > buf.len() {
            return Err(Error::NotEnoughBytes);
        }
        let v = u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]);
        *pos += 4;
        Ok((v & 0x7FFF_FFFF) as usize)
    }
}
