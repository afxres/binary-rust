//! Cross-language wire fixtures: the C# side.
//!
//! The table in `fixtures/csharp.tsv` is produced by the real `Mikodev.Binary`
//! library (`Converter.EncodeAuto`), not by re-implementing its rules. The
//! throwaway .NET generator that produced it is *not* committed (it needs an
//! external C# checkout); the recipe is in DESIGN §12.5. The table itself is,
//! and it is the source of truth this test asserts against. Two buckets:
//!
//! - `same` — C#, C++ and Rust agree byte for byte; the wire is asserted exactly.
//! - `csharp-only` — C# chose a wider header (it sizes string headers from the
//!   `3 * chars + 3` bound, DESIGN §12.3). Only the header width may differ: the
//!   payload must be identical, and Rust must still decode the C# bytes.
//!
//! The C++ table (`fixtures/cxx.tsv`, `tests/cxx_fixtures.rs`) asserts the same
//! values through the same table in `common::cases`, so the two files cannot
//! disagree about what a case name means. Adding a case means adding a row to
//! the table(s) and an entry in `common::cases`; an unknown name panics rather
//! than being skipped.

mod common;

use common::cases::{encode_case, reencode_case};
use common::{from_hex, split_prefixed, to_hex};

const FIXTURES: &str = include_str!("../fixtures/csharp.tsv");

#[test]
fn test_csharp_fixtures() {
    let mut seen = 0usize;
    for line in FIXTURES.lines() {
        // Tolerate a BOM: the table is regenerated on Windows.
        let line = line.trim_start_matches('\u{feff}').trim_end();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.split('\t');
        let name = cols.next().expect("missing name");
        let bucket = cols.next().expect("missing bucket");
        let hex = cols.next().expect("missing hex");
        assert!(cols.next().is_none(), "trailing column in {name}");

        let expected = from_hex(hex);
        let actual = encode_case(name);
        seen += 1;

        match bucket {
            "same" => assert_eq!(to_hex(&actual), hex, "case {name}"),
            "csharp-only" => {
                let (_, our_payload) = split_prefixed(&actual);
                let (_, cs_payload) = split_prefixed(&expected);
                assert_eq!(
                    to_hex(our_payload),
                    to_hex(cs_payload),
                    "case {name}: payload must match, only header width may differ"
                );
                // Rust must read the C# bytes and reproduce its own wire.
                assert_eq!(reencode_case(name, &expected), actual, "case {name}");
            }
            other => panic!("unknown bucket {other:?} for case {name}"),
        }
    }
    assert_eq!(
        seen, 33,
        "fixture table changed size; update the macro table too"
    );
}
