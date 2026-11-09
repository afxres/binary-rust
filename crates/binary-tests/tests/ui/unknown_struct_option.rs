// Struct-level attributes used to be dropped silently, handing back a named
// object to someone who asked for something else.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(nope)]
struct S {
    a: i32,
}

fn main() {}
