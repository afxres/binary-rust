//! Timing + allocation harness.
//!
//! Two rules keep the numbers usable:
//!
//! - every closure result goes through [`black_box`], otherwise the optimiser
//!   deletes the whole loop (it did, the first time);
//! - the reported value is the **minimum** over rounds, not the mean. The
//!   minimum is the least noisy estimator when other processes share the CPU,
//!   which is the case here (WSL, no pinning). Google benchmark's `Time` column
//!   is a mean-of-iterations, so treat the two as comparable only in magnitude.
//!
//! Allocation counting is opt-in through a relaxed flag rather than a second
//! allocator: the gate costs one relaxed load per allocation, which keeps the
//! timed runs honest.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
// `AtomicUsize`, not `AtomicU64`: 32-bit targets without 64-bit atomics
// (powerpc) have no `AtomicU64` at all, and the counters are reset before
// every measurement, so they never approach 4 GiB.
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Target wall time per measurement round.
const BUDGET_SECS: f64 = 0.05;
const ROUNDS: usize = 7;

/// Measure `f` and return `ns/op` without printing.
///
/// Iteration count is calibrated per round (doubling until the round exceeds
/// [`BUDGET_SECS`]), so cheap and expensive cases both land near the budget.
/// Warmup matters more than usual here: the first iterations pay for page faults
/// (encoder buffer, decoded `Vec`) and for branch-predictor cold start.
///
/// Every call goes through [`black_box`], including warmup, so the optimiser
/// cannot peel the first iterations out of the loop. Callers that only need the
/// number (a scheme matrix, where per-row printing would hide the shape) use
/// this directly.
pub fn measure<R>(mut f: impl FnMut() -> R) -> f64 {
    for _ in 0..2_000 {
        black_box(f());
    }
    let mut best = f64::INFINITY;
    for _ in 0..ROUNDS {
        let mut n = 1u64;
        loop {
            let start = Instant::now();
            for _ in 0..n {
                black_box(f());
            }
            let elapsed = start.elapsed().as_secs_f64();
            if elapsed >= BUDGET_SECS || n >= 1 << 32 {
                let per_op = elapsed / n as f64;
                if per_op < best {
                    best = per_op;
                }
                break;
            }
            n = n.saturating_mul(2);
        }
    }
    best * 1e9
}

/// Measure `f` and print `ns/op`.
pub fn bench(name: &str, f: impl FnMut()) {
    let ns = measure(f);
    println!("{name:<34} {ns:>9.2} ns/op");
}

/// Same as [`bench()`], but `f` is `FnMut() -> R` and the result is fed back
/// through `black_box` — for cases where the value, not a side effect, is the
/// thing that must not be optimised away.
pub fn bench_ret<R>(name: &str, f: impl FnMut() -> R) {
    let ns = measure(f);
    println!("{name:<34} {ns:>9.2} ns/op");
}

/// Report allocations performed by one call of `f`.
///
/// Bytes are the requested sizes summed; a `String` produced by `format!` shows
/// up as one allocation with the formatted length, a `Box` on top of it as a
/// second one.
pub fn allocs(label: &str, f: impl FnOnce()) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    f();
    COUNTING.store(false, Ordering::Relaxed);
    let allocs = ALLOCS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    println!("{label:<34} {allocs:>3} alloc(s), {bytes:>5} bytes");
}

/// Print the machine/compiler line that every number in this file depends on.
pub fn header(what: &str) {
    let model = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("?").trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("== {what} ==");
    println!("cpu   : {model}");
    let rustc = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "rustc (not on PATH)".into());
    println!("tool  : {rustc}");
    println!(
        "build : {}",
        if cfg!(debug_assertions) {
            "debug — numbers are meaningless, rerun with --release"
        } else {
            "release"
        }
    );
    println!("note  : min over {ROUNDS} rounds, {BUDGET_SECS} s budget each; no CPU pinning");
    println!();
}
