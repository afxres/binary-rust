use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S {
    #[binary(nope)]
    a: i32,
}

fn main() {}
