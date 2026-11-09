// A tuple object has no member names, so `rename` / `optional` / `skip` cannot
// be honoured.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(tuple)]
struct S {
    a: i32,
    #[binary(skip)]
    b: i32,
}

fn main() {}
