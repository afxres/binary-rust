//! Cross-language wire fixtures: the shared table.
//!
//! `fixtures/wire.tsv` is written by the real C# library and is byte-identical
//! to the copy in the C++ repository — one file, one source of truth. The fourth
//! column lists the implementations that must assert the row, so this test reads
//! every row carrying `rs` and checks both entry points at once.
//!
//! The auto column may list several legal wire forms separated by `|`: a length
//! header is 1 byte below 128 and 4 bytes otherwise and both are legal, so a row
//! lists each width an implementation may write. Producing any of them passes and
//! producing anything else fails — there is no decode fallback, because a value
//! comparison would hide a real byte difference.
//!
//! A row without `rs` is a type Rust has no equivalent for (`char` is 2 bytes in
//! C#, `Half`, `decimal`, the `System.Numerics` vector types, the C#-only
//! containers). `every_case_is_a_row` keeps that honest in one direction: a case
//! in `common::cases` must be backed by a row, so nothing can be asserted here
//! without also being asserted against the foreign bytes.

mod common;

use common::cases::{case_names, encode_bare_case, encode_case, reencode_case, verify_decode_case};
use common::from_hex;
use common::to_hex;
use std::collections::BTreeSet;

const FIXTURES: &str = include_str!("../fixtures/wire.tsv");

/// Cases asserted against `fsharp.tsv` instead: the union wire is not in the
/// shared table because `IUnion` only exists on net11 while `wire.tsv` is shared
/// with the net10 build.
const LOCAL_CASES: &[&str] = &[
    "union_dot",
    "union_circle",
    "union_text",
    "union_rect",
    "union_pair",
];

struct Row {
    name: &'static str,
    bare: &'static str,
    auto: &'static str,
    langs: &'static str,
}

impl Row {
    /// The accepted `EncodeAuto` byte strings for this row, never empty.
    fn auto_forms(&self) -> Vec<&'static str> {
        self.auto.split('|').collect()
    }
}

fn rows() -> Vec<Row> {
    FIXTURES
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim_end())
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut cols = line.split('\t');
            let name = cols.next().expect("missing name");
            let bare = cols.next().expect("missing bare");
            let auto = cols.next().expect("missing auto");
            let langs = cols.next().expect("missing langs");
            assert!(cols.next().is_none(), "trailing column in {name}");
            Row {
                name,
                bare,
                auto,
                langs,
            }
        })
        .collect()
}

fn has_rust(langs: &str) -> bool {
    langs.split(',').any(|x| x == "rs")
}

/// `-` is how the table spells "zero bytes": a tab-separated line cannot end in
/// an empty field without it looking like a missing column.
fn bare_hex(text: &str) -> &str {
    if text == "-" {
        ""
    } else {
        text
    }
}

/// Frames a hex payload with a 1 or 4 byte length header.
fn frame(width: usize, payload: &str) -> String {
    let length = payload.len() / 2;
    if width == 1 {
        format!("{length:02x}{payload}")
    } else {
        format!("{:08x}{payload}", 0x8000_0000u32 | length as u32)
    }
}

/// The width of the form's own length header, or 0 when it has none: a form only
/// carries one when it ends with the bare payload. The extra length alone is not
/// enough, because a tuple object writes its members' prefixes while adding
/// nothing of its own (`tuple_i32_str` is `01000000 0161` against a 5 byte bare).
fn header_width(form: &str, payload: &str) -> usize {
    if payload.is_empty() {
        return if form.len() == 2 || form.len() == 8 {
            form.len() / 2
        } else {
            0
        };
    }
    if form.len() <= payload.len() {
        return 0;
    }
    let width = (form.len() - payload.len()) / 2;
    if (width != 1 && width != 4) || payload.len() + width * 2 != form.len() {
        return 0;
    }
    if form.ends_with(payload) {
        width
    } else {
        0
    }
}

/// Both header widths a reader has to accept, whether or not the table lists
/// them. A decoder that only handles the width its own writer emits is a split
/// decoder, and the table's 1 byte form hides that.
fn decode_forms(row: &Row) -> Vec<String> {
    let payload = bare_hex(row.bare);
    let mut forms: Vec<String> = Vec::new();
    for form in row.auto_forms() {
        let form = bare_hex(form);
        if header_width(form, payload) == 0 {
            if !forms.iter().any(|x| x.as_str() == form) {
                forms.push(form.to_string());
            }
            continue;
        }
        if payload.len() / 2 < 128 {
            let narrow = frame(1, payload);
            if !forms.iter().any(|x| *x == narrow) {
                forms.push(narrow);
            }
        }
        let wide = frame(4, payload);
        if !forms.iter().any(|x| *x == wide) {
            forms.push(wide);
        }
    }
    forms
}

#[test]
fn test_shared_fixtures() {
    let mut seen = 0usize;
    for row in rows() {
        if !has_rust(row.langs) {
            continue;
        }
        let name = row.name;

        let bare = encode_bare_case(name);
        assert_eq!(to_hex(&bare), bare_hex(row.bare), "case {name}: Encode");

        let auto = encode_case(name);
        let auto_hex = to_hex(&auto);
        let forms = row.auto_forms();
        assert!(
            forms.iter().any(|form| *form == auto_hex),
            "case {name}: EncodeAuto {auto_hex} is not one of {forms:?}"
        );

        assert!(
            reencode_case(name, &auto) == auto,
            "case {name}: decoding the produced bytes does not reproduce them"
        );

        // Decode direction: the table bytes are the input, the corpus value is
        // the oracle. `Bare` feeds `Decode`, both header widths feed `DecodeAuto`,
        // and the decoded value must equal the corpus value structurally — the
        // table's 1 byte form would otherwise hide a decoder that only handles the
        // width this writer emits.
        verify_decode_case(name, &from_hex(bare_hex(row.bare)), false);
        for form in decode_forms(&row) {
            verify_decode_case(name, &from_hex(&form), true);
        }

        seen += 1;
    }
    assert_eq!(
        seen, 74,
        "rows carrying `rs` changed; check the langs column"
    );

    // Keep the widening above from quietly degrading to "only the listed form":
    // every variable length row must be decoded twice.
    let widened = rows()
        .iter()
        .filter(|row| has_rust(row.langs) && decode_forms(row).len() > 1)
        .count();
    assert!(
        widened > 20,
        "only {widened} rows exercise both header widths"
    );
}

#[test]
fn every_case_is_a_row() {
    let shared: BTreeSet<&str> = rows()
        .into_iter()
        .filter(|row| has_rust(row.langs))
        .map(|row| row.name)
        .collect();

    let missing: Vec<&str> = case_names()
        .iter()
        .copied()
        .filter(|name| !shared.contains(name) && !LOCAL_CASES.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "cases with no shared row and not listed in LOCAL_CASES: {missing:?}"
    );

    let stale: Vec<&str> = LOCAL_CASES
        .iter()
        .copied()
        .filter(|name| !case_names().contains(name))
        .collect();
    assert!(stale.is_empty(), "LOCAL_CASES without a case: {stale:?}");
}
