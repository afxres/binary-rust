#!/bin/bash
#
# Build the benchmark crate in release mode, then run one group.
#
#   bash scripts/run-benchmark.sh              # wire — the integration payloads
#   bash scripts/run-benchmark.sh objects      # tuple / tuple object / named
#   bash scripts/run-benchmark.sh collections  # 1024-element bulk vs per element
#   bash scripts/run-benchmark.sh errors       # enum vs Box<dyn Error>
#   bash scripts/run-benchmark.sh profile      # attributing the C++ gap
#   bash scripts/run-benchmark.sh lookup       # named-key lookup: match vs table schemes
#   bash scripts/run-benchmark.sh all          # all five groups, one after another
#
# `wire` is the default because its four rows are the ones C#
# `Benchmarks.IntegrationTests` and C++ `binary-benchmarks/integration-tests`
# also measure (§19.15), so the three repositories can diff them row by row.
# Cargo's build chatter goes to stderr on purpose: stdout belongs to the
# benchmark, so a report can pipe it straight into JSON.

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GROUP="${1:-wire}"

usage() {
    printf 'usage: %s [wire|objects|collections|errors|profile|lookup|all]\n' "${0##*/}" >&2
    printf '  wire (default)  tuple / named, the rows C# and C++ also measure\n' >&2
    printf '  objects         tuple / tuple object / named, owned and scalar baselines\n' >&2
    printf '  collections     1024-element bulk vs per element\n' >&2
    printf '  errors          enum vs Box<dyn Error>\n' >&2
    printf '  profile         attribute the C++ gap\n' >&2
    printf '  lookup          named-key lookup; "lookup full" runs the whole matrix\n' >&2
    printf '  all             all five groups\n' >&2
}

case "$GROUP" in
    -h | --help)
        sed -n '2,17p' "${BASH_SOURCE[0]}" | sed -e 's/^# \{0,1\}//'
        exit 0
        ;;
    wire | objects | collections | errors | profile | lookup | all) ;;
    *)
        printf 'unknown benchmark group: %s\n' "$GROUP" >&2
        usage
        exit 2
        ;;
esac
if [ $# -gt 1 ]; then
    printf 'unexpected extra arguments: %s\n' "${*:2}" >&2
    usage
    exit 2
fi

BENCH=(cargo run --release -p binary-bench --manifest-path "$PROJECT_ROOT/Cargo.toml")
if [ "$GROUP" = all ]; then
    exec "${BENCH[@]}"
fi
exec "${BENCH[@]}" -- "$GROUP"
