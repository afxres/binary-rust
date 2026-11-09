use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
#[repr(C)]
enum E {
    A,
}

fn main() {}
