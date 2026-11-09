use binary::{Decode, Encode};

#[derive(Encode, Decode)]
union U {
    a: i32,
    b: f32,
}

fn main() {}
