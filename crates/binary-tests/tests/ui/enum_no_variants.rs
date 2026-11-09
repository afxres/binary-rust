use binary::{Decode, Encode};

#[derive(Encode, Decode)]
#[binary(int)]
enum E {}

fn main() {}
