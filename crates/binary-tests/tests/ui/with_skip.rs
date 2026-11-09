use binary::{Decode, Encode};

mod conv {
    pub type Repr = i64;

    pub fn to(_: &u8) -> Repr {
        0
    }

    pub fn from(_: Repr) -> binary::Result<u8> {
        Ok(0)
    }
}

#[derive(Encode, Decode)]
struct S {
    #[binary(with = "conv", skip)]
    a: u8,
}

fn main() {}
