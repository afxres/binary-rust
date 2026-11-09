//! Cross-language wire fixtures: the F# side.
//!
//! `fixtures/fsharp.tsv` is emitted by the real `Mikodev.Binary.FSharp` union
//! converter (`AddFSharpConverterCreators` → `UnionConverterCreator`), not by
//! re-implementing its rules. The generator is machine-local and not committed
//! (it needs an external C# checkout); the recipe is in DESIGN §12.5.
//!
//! Only the union rows live here. That converter is a *separate*
//! implementation from the C# one — it reflects over `FSharpType` / `UnionCaseInfo`
//! instead of `IUnion` — and it had never been compared byte for byte. The rest
//! of the corpus (named objects, tuple objects, containers) has no F# shape of
//! its own; `csharp.tsv` covers those through the same library.
//!
//! The cases run through `common::cases` like the other two tables, so a name
//! cannot mean different values here and there.

mod common;

use common::cases::encode_case;
use common::to_hex;

const FIXTURES: &str = include_str!("../fixtures/fsharp.tsv");

#[test]
fn test_fsharp_fixtures() {
    let mut seen = 0usize;
    for line in FIXTURES.lines() {
        let line = line.trim_start_matches('\u{feff}').trim_end();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut cols = line.split('\t');
        let name = cols.next().expect("missing name");
        let hex = cols.next().expect("missing hex");
        assert!(cols.next().is_none(), "trailing column in {name}");

        assert_eq!(to_hex(&encode_case(name)), hex, "case {name}");
        seen += 1;
    }
    assert_eq!(
        seen, 5,
        "fixture table changed size; update the union table too"
    );
}
