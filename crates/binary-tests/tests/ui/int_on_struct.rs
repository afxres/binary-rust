use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
struct S {
    a: i32,
}

fn main() {}
