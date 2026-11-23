#!/bin/bash
#
# Collect line coverage of the product crates into target/coverage/lcov.info.
#
#   bash scripts/run-coverage.sh
#
# Only the product crates are measured. crates/binary-tests is the test suite and
# crates/binary-bench a benchmark harness, so covering them would report test and
# harness code as product code -- the C++ side excludes *-tests/* for the same
# reason. crates/binary-derive stays in: the macro body really executes while the
# derived test types are compiled, and llvm-cov attributes those hits to it.
#
# lcov is the format Coveralls ingests. The file goes under target/ with the rest
# of the build output: it is generated, so it is ignored and never committed.
#
# Requires cargo-llvm-cov:  cargo install cargo-llvm-cov --locked
# Doctests are not measured; covering them needs a nightly toolchain.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

PROJECT_RESULT_DIR="$PWD/target/coverage"
COVERAGE_FILE_PATH="$PROJECT_RESULT_DIR/lcov.info"

if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "cargo-llvm-cov is not installed: cargo install cargo-llvm-cov --locked" >&2
    exit 1
fi

mkdir -p "$PROJECT_RESULT_DIR"
cargo llvm-cov --all \
    --ignore-filename-regex 'crates/(binary-tests|binary-bench)/' \
    --lcov --output-path "$COVERAGE_FILE_PATH"

# llvm-cov records absolute source paths. Strip the checkout prefix so the report
# reads the same on any machine and Coveralls matches it against the repo root.
sed -i "s|^SF:$PWD/|SF:|" "$COVERAGE_FILE_PATH"
