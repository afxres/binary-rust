//! Hex preview shared by the `Debug` impls.
//!
//! Wire bugs are byte bugs: printing `Encoder { len: 17 }` without the bytes
//! leaves the reader to reconstruct them from memory. A capped hex dump is the
//! one piece of state that actually explains a mismatch, and it stays small
//! enough to read in a panic message.

use core::fmt;

/// How many bytes a preview shows before truncating.
pub(crate) const PREVIEW: usize = 32;

/// Lowercase hex dump of `bytes`, truncated to [`PREVIEW`].
pub(crate) struct Hex<'a>(pub(crate) &'a [u8]);

impl fmt::Debug for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        let shown = bytes.len().min(PREVIEW);
        write!(f, "\"")?;
        for b in &bytes[..shown] {
            write!(f, "{b:02x}")?;
        }
        if shown < bytes.len() {
            write!(f, ".. ({} bytes)", bytes.len())?;
        }
        write!(f, "\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::String;

    #[test]
    fn short_and_truncated() {
        assert_eq!(format!("{:?}", Hex(&[0x00, 0x7f, 0xff])), "\"007fff\"");
        assert_eq!(format!("{:?}", Hex(&[])), "\"\"");

        let long = [0xABu8; PREVIEW + 1];
        let shown = String::from("\"") + &"ab".repeat(PREVIEW) + ".. (33 bytes)\"";
        assert_eq!(format!("{:?}", Hex(&long)), shown);
    }
}
