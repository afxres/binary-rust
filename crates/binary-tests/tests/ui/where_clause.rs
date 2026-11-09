use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S<'a>
where
    'a: 'static,
{
    a: &'a str,
}

fn main() {}
