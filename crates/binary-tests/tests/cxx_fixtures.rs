//! Cross-language wire fixtures: the C++ side.
//!
//! `fixtures/cxx.tsv` is emitted by the real `binary-cxx` library
//! (`ConverterEncodeAuto`), not by re-implementing its rules. The generator is
//! machine-local and *not* committed — it includes a local C++ checkout and
//! links that checkout's `libbinary.a`; the recipe is in DESIGN 13.5. The
//! values come from `common::cases`, the same table the C# test uses, so the two
//! files cannot drift apart in what a case name means.
//!
//! Every row is asserted byte for byte. `binary-cxx` predicts the length header
//! for blittable containers, so it writes the same prefix width Rust does and no
//! bucket column is needed — unlike `csharp.tsv`, where `string` at 42..=127
//! characters still keeps a 4-byte anchor header.
//!
//! `cxx.tsv` has no rows for cases the C++ library cannot express at all (it has
//! no nullable converter). That is asserted below rather than assumed: the C++
//! name set must equal the C# name set minus `CXX_GAP`, so a case added to
//! `csharp.tsv` fails this test until it is either covered in `cxx.tsv` or
//! listed as a gap.

mod common;

use common::cases::encode_case;
use common::to_hex;
use std::collections::BTreeSet;

const FIXTURES: &str = include_str!("../fixtures/cxx.tsv");
const CSHARP_FIXTURES: &str = include_str!("../fixtures/csharp.tsv");

/// Rows with no C++ counterpart: `binary-cxx` has no nullable converter at all,
/// so `Option<T>` — and any object holding one — cannot be produced by it.
const CXX_GAP: &[&str] = &[
    "i32opt_none",
    "i32opt_some5",
    "vec_i32opt",
    "vec_i32opt_none",
    "named_with_opt_none",
    "named_with_opt_some",
];

/// `(name, hex)` per data row; comments and blank lines are skipped. `cxx.tsv`
/// carries no bucket column: every row is an exact match.
fn cxx_rows() -> Vec<(&'static str, &'static str)> {
    FIXTURES
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim_end())
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next().expect("missing name");
            let hex = cols.next().expect("missing hex");
            assert!(cols.next().is_none(), "trailing column in {name}");
            (name, hex)
        })
        .collect()
}

/// `(name, bucket, hex)` per data row of `csharp.tsv`; only the names are used
/// here, to pin the gap.
fn csharp_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    CSHARP_FIXTURES
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim_end())
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next().expect("missing name");
            let bucket = cols.next().expect("missing bucket");
            let hex = cols.next().expect("missing hex");
            assert!(cols.next().is_none(), "trailing column in {name}");
            (name, bucket, hex)
        })
        .collect()
}

#[test]
fn test_cxx_fixtures() {
    let rows = cxx_rows();
    for (name, hex) in &rows {
        assert_eq!(to_hex(&encode_case(name)), *hex, "case {name}");
    }
    assert_eq!(
        rows.len(),
        27,
        "fixture table changed size; update the gap list too"
    );
}

#[test]
fn test_cxx_covers_every_expressible_case() {
    let csharp: BTreeSet<&str> = csharp_rows().into_iter().map(|r| r.0).collect();
    let cxx: BTreeSet<&str> = cxx_rows().into_iter().map(|r| r.0).collect();
    let gap: BTreeSet<&str> = CXX_GAP.iter().copied().collect();

    assert!(
        gap.is_subset(&csharp),
        "CXX_GAP names must exist in csharp.tsv: {:?}",
        gap.difference(&csharp).collect::<Vec<_>>()
    );
    let expected: BTreeSet<&str> = csharp.difference(&gap).copied().collect();
    assert_eq!(
        cxx,
        expected,
        "cxx.tsv must cover every non-gap C# case; missing {:?}, unexpected {:?}",
        expected.difference(&cxx).collect::<Vec<_>>(),
        cxx.difference(&expected).collect::<Vec<_>>()
    );
}
