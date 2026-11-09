// Union tags follow declaration order. An explicit discriminant would silently
// disagree with the wire, so it is rejected instead of ignored. (Unit variants
// only, so rustc itself has no opinion about the enum.)
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(union)]
enum E {
    A,
    B = 5,
}

fn main() {}
