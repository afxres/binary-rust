use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S {
    #[binary(with = conv)]
    a: u8,
}

fn main() {}
