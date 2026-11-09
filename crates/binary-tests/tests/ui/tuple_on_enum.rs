use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(tuple)]
enum E {
    A,
}

fn main() {}
