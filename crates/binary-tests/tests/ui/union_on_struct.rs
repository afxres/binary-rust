// `#[binary(union)]` on a struct: a struct has no variants, so there is no tag.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(union)]
struct S {
    a: i32,
}

fn main() {}
