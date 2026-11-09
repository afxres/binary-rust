//! Compile-fail tests for `binary-derive`.
//!
//! Every file in `tests/ui` must fail to compile with exactly the diagnostics
//! recorded in the neighbouring `.stderr`. Regenerate snapshots after an
//! intentional message change with:
//!
//! ```text
//! TRYBUILD=overwrite cargo test -p binary-tests --test ui
//! ```
//!
//! The snapshots pin three things at once: the message text, the caret position
//! (it must land on the offending field or option, not on `#[derive(...)]`), and
//! the fact that each mistake is reported once.
//!
//! Miri skips this test: `trybuild` globs the filesystem (`statx`) and shells
//! out to rustc, neither of which works under Miri's isolation. These are
//! compile-time diagnostics anyway — there is no runtime `unsafe` to check.
//! The real binaries still run these fixtures under `cargo test` and under
//! qemu on s390x (§19.11).

#[cfg(not(miri))]
#[test]
fn ui() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
