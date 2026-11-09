#!/bin/bash
# Check the test suite for undefined behaviour under Miri, on the host and on
# the big-endian abstract machine. Complements `scripts/test-multiarch.sh s390x`:
# qemu executes real MSB ELF binaries + real libc, Miri interprets MIR and
# checks the `unsafe` in Vec<T>::decode's bulk arm (DESIGN §19.10 / §19.12).
# Neither substitutes for the other.
#
# Usage:
#   ./scripts/check-miri.sh          # host + s390x abstract machine
#   ./scripts/check-miri.sh host     # host only
#   ./scripts/check-miri.sh be       # big-endian abstract machine only
#   MIRIFLAGS=-Zmiri-strict-provenance ./scripts/check-miri.sh host
#
# Miri requires nightly; the script installs the toolchain + components when
# missing. `tests/ui.rs` is gated with `#[cfg(not(miri))]`: trybuild globs the
# filesystem and shells out to rustc, which Miri's isolation forbids, and
# compile-fail diagnostics have no runtime UB to check anyway. The real
# binaries still run those fixtures under `cargo test` and under qemu.

set -euo pipefail

TOOLCHAIN="nightly"
BE_TARGET="s390x-unknown-linux-gnu"
PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-all}"

case "$MODE" in
    all | host | be) ;;
    *)
        echo "usage: $0 [all|host|be]" >&2
        exit 2
        ;;
esac

if ! rustup toolchain list | grep -q "^${TOOLCHAIN}-"; then
    echo "installing $TOOLCHAIN with miri + rust-src"
    rustup toolchain install "$TOOLCHAIN" --profile minimal --component miri,rust-src
fi
for component in miri rust-src; do
    if ! rustup component list --toolchain "$TOOLCHAIN" --installed | grep -qx "$component"; then
        echo "adding $component to $TOOLCHAIN"
        rustup component add --toolchain "$TOOLCHAIN" "$component"
    fi
done

cd "$PROJECT_ROOT"

if [ "$MODE" != "be" ]; then
    echo "miri: host — full workspace (trybuild ui skipped via cfg(miri))"
    cargo "+$TOOLCHAIN" miri test --all
fi
if [ "$MODE" != "host" ]; then
    echo "miri: $BE_TARGET — big-endian abstract machine"
    cargo "+$TOOLCHAIN" miri test --target "$BE_TARGET" -p binary -p binary-tests
fi
