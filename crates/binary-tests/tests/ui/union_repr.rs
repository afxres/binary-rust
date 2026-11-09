// `#[repr(...)]` does not reach the wire for a tagged union: the tag is always a
// compact integer. Rejecting it beats letting the user believe it narrowed tags.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(union)]
#[repr(u8)]
enum E {
    A(i32),
}

fn main() {}
