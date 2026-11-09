use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S {
    #[binary(rename = 1)]
    a: i32,
}

fn main() {}
