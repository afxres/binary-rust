use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S<'a, 'b> {
    a: &'a str,
    b: &'b str,
}

fn main() {}
