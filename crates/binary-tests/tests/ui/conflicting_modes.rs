// Two layout modes at once. The old parser let the last one win, which silently
// produced a named object for `#[binary(tuple, nope)]`-style typos.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(tuple, union)]
enum E {
    A(i32),
}

fn main() {}
