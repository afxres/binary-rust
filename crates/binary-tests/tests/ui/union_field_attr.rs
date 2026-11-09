// Union members are positional and always encoded: `skip` / `rename` /
// `optional` have no meaning, and dropping them silently would produce a layout
// the user did not ask for.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(union)]
enum E {
    A(#[binary(skip)] i32),
}

fn main() {}
