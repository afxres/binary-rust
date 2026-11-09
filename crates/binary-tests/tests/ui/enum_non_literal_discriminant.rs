use binary::{Decode, Encode};

const BASE: i32 = 3;

#[derive(Encode, Decode)]
#[binary(int)]
enum E {
    A = BASE,
}

fn main() {}
