use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S<T> {
    a: T,
}

fn main() {}
