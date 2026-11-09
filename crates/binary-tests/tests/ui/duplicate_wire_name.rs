// Two members cannot share a wire name: encode would emit it twice and decode
// would report `DuplicateMember` on the second occurrence.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct Person {
    #[binary(rename = "id")]
    a: i32,
    #[binary(rename = "id")]
    b: i32,
}

fn main() {}
