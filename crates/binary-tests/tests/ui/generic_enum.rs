use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
enum E<T> {
    A(T),
}

fn main() {}
