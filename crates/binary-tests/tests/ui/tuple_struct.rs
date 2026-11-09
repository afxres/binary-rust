// Phase 1 covers named structs only. A tuple struct would need its own layout
// rules; `#[binary(tuple)]` on a named struct already expresses a tuple object.
use binary::{Decode, Encode};

#[derive(Encode, Decode)]
struct S(i32, i32);

fn main() {}
