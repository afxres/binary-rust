use core::marker::PhantomData;

fn main() {
    // A type-level marker carries no value, so it has no converter: a struct
    // that uses one marks the field `#[binary(skip)]` instead.
    let _ = binary::encode_value(&PhantomData::<u8>);
}
