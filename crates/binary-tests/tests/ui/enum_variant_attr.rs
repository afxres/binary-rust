// Variant-level `#[binary(...)]` used to be skipped silently. Variant options
// are not supported, so say so instead of ignoring the attribute.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
enum E {
    #[binary(rename = "x")]
    A,
    B,
}

fn main() {}
