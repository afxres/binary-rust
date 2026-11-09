//! Local benchmark harness (DESIGN §18.8 / §9 item 5).
//!
//! Hand-rolled and dependency-free on purpose: the numbers have to be comparable
//! with google benchmark (C++) and a stopwatch loop (C#), and criterion's
//! statistical model would make that comparison an apples-to-oranges one.
//!
//! ```text
//! cargo run --release -p binary-bench -- wire         # the four rows C# and C++ also measure
//! cargo run --release -p binary-bench -- objects      # tuple / tuple object / named / owned (§19.15.8)
//! cargo run --release -p binary-bench -- collections  # 1024-element bulk vs per element
//! cargo run --release -p binary-bench -- errors       # enum vs Box<dyn Error>
//! cargo run --release -p binary-bench -- profile      # attributing the C++ gap (§19.15)
//! cargo run --release -p binary-bench -- lookup       # named-key lookup: match vs table schemes
//! cargo run --release -p binary-bench                 # all five groups (not `lookup`)
//! ```

mod collections;
mod errors;
mod harness;
mod lookup;
mod objects;
mod profile;
mod wire;

fn main() {
    let mut args = std::env::args().skip(1);
    let what = args.next();
    match what.as_deref() {
        Some("wire") => wire::run(),
        Some("objects") => objects::run(),
        Some("collections") => collections::run(),
        Some("errors") => errors::run(),
        Some("profile") => profile::run(),
        Some("lookup") => lookup::run(args.next().as_deref() == Some("full")),
        Some(other) => {
            eprintln!("unknown benchmark group: {other}");
            eprintln!("usage: binary-bench [wire|objects|collections|errors|profile|lookup]");
            eprintln!("       binary-bench lookup [full]");
            std::process::exit(2);
        }
        None => {
            wire::run();
            objects::run();
            collections::run();
            errors::run();
            profile::run();
        }
    }
}
