// Only unit variants: a payload-carrying variant needs a tag scheme the wire
// format does not define yet.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
enum E {
    A(i32),
    B,
}

fn main() {}
