//! `Error` (closed enum) vs `Box<dyn Error + Send + Sync>` (fully erased).
//!
//! The point is to price the four things the decision in DESIGN §18.8 turns on:
//!
//! 1. construction — `Error::NotEnoughBytes` is a constant, the boxed form is an
//!    allocation;
//! 2. propagation — does `?` through 3 layers cost anything, and does the boxed
//!    form re-box on the way up (it does not: `Box<dyn Error>` is the same type
//!    at every layer, so each hop is a move);
//! 3. the boundary — `binary::Result<T>` converted to the caller's erased type;
//! 4. the message path — a static variant vs `format!`-ted text, which is what
//!    every one of the 14 predefined construction sites would have to become.
//!
//! The happy path is measured too: if `Result<T, Error>` and
//! `Result<T, Box<dyn Error>>` compile to the same code, the "24 bytes vs 16
//! bytes" difference is not a real cost and should stop being cited as one.

use std::fmt;
use std::mem::size_of;

use binary::{Decode, Decoder, Encode, Error};

use crate::harness::{allocs, bench, bench_ret, header};

/// Erased error type, exactly as a `dyn Error` model would define it.
type DynErr = Box<dyn std::error::Error + Send + Sync>;
type DynResult<T> = Result<T, DynErr>;

/// Static-text error: the cheapest thing that can go in the box.
#[derive(Debug)]
struct StrError(&'static str);

impl fmt::Display for StrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StrError {}

/// Dynamic-text error: what a `format!`-based site would produce.
#[derive(Debug)]
struct MsgError(String);

impl fmt::Display for MsgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MsgError {}

// --- propagation: three frames, innermost decides ---------------------------

#[inline(never)]
fn layer3_enum(fail: bool) -> binary::Result<u32> {
    if fail {
        Err(Error::NotEnoughBytes)
    } else {
        Ok(1)
    }
}

#[inline(never)]
fn layer2_enum(fail: bool) -> binary::Result<u32> {
    Ok(layer3_enum(fail)? + 1)
}

#[inline(never)]
fn layer1_enum(fail: bool) -> binary::Result<u32> {
    Ok(layer2_enum(fail)? + 1)
}

#[inline(never)]
fn layer3_dyn(fail: bool) -> DynResult<u32> {
    if fail {
        Err(Box::new(StrError("not enough bytes")))
    } else {
        Ok(1)
    }
}

#[inline(never)]
fn layer2_dyn(fail: bool) -> DynResult<u32> {
    Ok(layer3_dyn(fail)? + 1)
}

#[inline(never)]
fn layer1_dyn(fail: bool) -> DynResult<u32> {
    Ok(layer2_dyn(fail)? + 1)
}

// --- the boundary the library would have to cross ----------------------------

#[inline(never)]
fn boundary(fail: bool) -> DynResult<u32> {
    Ok(layer1_enum(fail)?)
}

// --- happy path: same decode, two error types --------------------------------

const COUNT: usize = 256;

fn payload() -> Vec<i32> {
    (0..COUNT as i32).collect()
}

fn decode_enum(d: &mut Decoder<'_>) -> binary::Result<Vec<i32>> {
    Vec::<i32>::decode_auto(d)
}

fn decode_dyn(d: &mut Decoder<'_>) -> DynResult<Vec<i32>> {
    Ok(Vec::<i32>::decode_auto(d)?)
}

pub fn run() {
    header("error model: enum vs Box<dyn Error + Send + Sync>");

    println!(
        "size_of::<Error>()                 = {}",
        size_of::<Error>()
    );
    println!(
        "size_of::<Result<i32, Error>>()    = {}",
        size_of::<binary::Result<i32>>()
    );
    println!(
        "size_of::<Result<i32, DynErr>>()   = {}",
        size_of::<DynResult<i32>>()
    );
    println!(
        "size_of::<Box<dyn Error>>()        = {}",
        size_of::<DynErr>()
    );
    println!();

    println!("-- construction --");
    bench("Error::NotEnoughBytes", || {
        std::hint::black_box(Error::NotEnoughBytes);
    });
    bench("Error::message_static", || {
        std::hint::black_box(Error::message_static("not enough bytes"));
    });
    bench("Error::message(format!)", || {
        let v = std::hint::black_box(7);
        std::hint::black_box(Error::message(format_args!("bad count {v}")));
    });
    bench("Box::new(StrError)", || {
        std::hint::black_box(Box::new(StrError("not enough bytes")) as DynErr);
    });
    bench("Box::new(MsgError(format!))", || {
        let v = std::hint::black_box(7);
        std::hint::black_box(Box::new(MsgError(format!("bad version {v}"))) as DynErr);
    });
    println!();

    println!("-- propagation, 3 frames, innermost fails --");
    bench("enum: layer1(..)?", || {
        std::hint::black_box(layer1_enum(std::hint::black_box(true)).is_err());
    });
    bench("dyn : layer1(..)?", || {
        std::hint::black_box(layer1_dyn(std::hint::black_box(true)).is_err());
    });
    bench("enum -> dyn at boundary", || {
        std::hint::black_box(boundary(std::hint::black_box(true)).is_err());
    });
    println!();

    println!("-- happy path: decode Vec<i32> (n = {COUNT}) --");
    let mut e = binary::Encoder::new();
    payload().encode_auto(&mut e).expect("infallible");
    let bytes = e.finish().expect("owned");
    bench_ret("decode -> binary::Result", || {
        let mut d = Decoder::new(std::hint::black_box(&bytes));
        decode_enum(&mut d).expect("valid")
    });
    bench_ret("decode -> DynResult", || {
        let mut d = Decoder::new(std::hint::black_box(&bytes));
        decode_dyn(&mut d).expect("valid")
    });
    println!();

    println!("-- error message rendering --");
    let enum_err = Error::message(format_args!("bad version {}", 7));
    let dyn_err: DynErr = Box::new(MsgError(String::from("bad version 7")));
    bench_ret("Custom(..).to_string()", || {
        std::hint::black_box(&enum_err).to_string()
    });
    bench_ret("Box<dyn>.to_string()", || {
        std::hint::black_box(&dyn_err).to_string()
    });
    println!();

    println!("-- allocations per one failure --");
    allocs("enum: NotEnoughBytes", || {
        std::hint::black_box(Error::NotEnoughBytes);
    });
    allocs("enum: message_static", || {
        std::hint::black_box(Error::message_static("not enough bytes"));
    });
    allocs("enum: message(format!)", || {
        let v = std::hint::black_box(7);
        std::hint::black_box(Error::message(format_args!("bad count {v}")));
    });
    allocs("enum: wrapped(StrError)", || {
        std::hint::black_box(Error::wrapped(StrError("not enough bytes")));
    });
    allocs("dyn : Box::new(StrError)", || {
        std::hint::black_box(Box::new(StrError("not enough bytes")) as DynErr);
    });
    allocs("dyn : Box::new(MsgError)", || {
        let v = std::hint::black_box(7);
        std::hint::black_box(Box::new(MsgError(format!("bad version {v}"))) as DynErr);
    });
    allocs("enum -> dyn at boundary", || {
        std::hint::black_box(boundary(std::hint::black_box(true)).is_err());
    });

    // Keep `payload()` honest: the decode benches above must decode a real
    // vector, not an empty one.
    assert_eq!(payload().len(), COUNT);
}
