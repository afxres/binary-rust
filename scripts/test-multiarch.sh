#!/bin/bash
#
# Cross-test the workspace for one or more target architectures.
#
#   bash scripts/test-multiarch.sh                                # host only
#   bash scripts/test-multiarch.sh x86_64 i686 aarch64 armv7 s390x
#   bash scripts/test-multiarch.sh --all                          # every known target,
#                                                                 # skipping missing toolchains
#   bash scripts/test-multiarch.sh --clean s390x                  # rebuild from scratch
#   bash scripts/test-multiarch.sh s390x -- --test wire_fixtures  # extra cargo args
#
# Covered widths: 64-bit little endian (x86_64, aarch64, riscv64), 32-bit little
# endian (i686, armv7), 64-bit big endian (s390x), 32-bit big endian (powerpc).
# Add a target by adding one line to `rust_info`.
#
# Every non-host target gets CARGO_TARGET_<TRIPLE>_LINKER and _RUNNER, so cargo
# runs the test binaries through qemu-user with that target's sysroot and
# binfmt_misc is not required. A freshly built test binary's ELF header is
# checked against the requested width and endianness, so a mislabelled
# toolchain cannot pass as a successful target.
#
# Toolchains, e.g. for s390x (repeat per target):
#   sudo apt install gcc-s390x-linux-gnu libc6-dev-s390x-cross qemu-user
#   rustup target add s390x-unknown-linux-gnu   (this script does it if missing)
#
# Environment: JOBS (default: online CPUs), LOG_ROOT (default: target/multiarch).

set -uo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
JOBS="${JOBS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)}"
LOG_ROOT="${LOG_ROOT:-$PROJECT_ROOT/target/multiarch}"

ALL_TARGETS="x86_64 i686 aarch64 armv7 riscv64 powerpc s390x"
CLEAN=0
ALLOW_MISSING=0
SKIPPED=0
FAILED=0

usage() {
    sed -n '3,27p' "${BASH_SOURCE[0]}" | sed -e 's/^# \{0,1\}//'
}

# Sets ARCH RUST_TARGET TRIPLE QEMU DEBARCH BITS ENDIAN for a target name or alias.
rust_info() {
    case "$1" in
        x86_64 | amd64 | x64) ARCH=x86_64 RUST_TARGET=x86_64-unknown-linux-gnu TRIPLE=x86_64-linux-gnu QEMU=qemu-x86_64 DEBARCH=amd64 BITS=64 ENDIAN=little ;;
        i686 | i386 | x86) ARCH=i686 RUST_TARGET=i686-unknown-linux-gnu TRIPLE=i686-linux-gnu QEMU=qemu-i386 DEBARCH=i386 BITS=32 ENDIAN=little ;;
        aarch64 | arm64) ARCH=aarch64 RUST_TARGET=aarch64-unknown-linux-gnu TRIPLE=aarch64-linux-gnu QEMU=qemu-aarch64 DEBARCH=arm64 BITS=64 ENDIAN=little ;;
        armv7 | armhf | armv7l) ARCH=armv7 RUST_TARGET=armv7-unknown-linux-gnueabihf TRIPLE=arm-linux-gnueabihf QEMU=qemu-arm DEBARCH=armhf BITS=32 ENDIAN=little ;;
        riscv64) ARCH=riscv64 RUST_TARGET=riscv64gc-unknown-linux-gnu TRIPLE=riscv64-linux-gnu QEMU=qemu-riscv64 DEBARCH=riscv64 BITS=64 ENDIAN=little ;;
        powerpc | ppc) ARCH=powerpc RUST_TARGET=powerpc-unknown-linux-gnu TRIPLE=powerpc-linux-gnu QEMU=qemu-ppc DEBARCH=powerpc BITS=32 ENDIAN=big ;;
        s390x) ARCH=s390x RUST_TARGET=s390x-unknown-linux-gnu TRIPLE=s390x-linux-gnu QEMU=qemu-s390x DEBARCH=s390x BITS=64 ENDIAN=big ;;
        *) return 1 ;;
    esac
}

HOST_ARCH="$(uname -m)"
rust_info "$HOST_ARCH" >/dev/null 2>&1 && HOST_ARCH="$ARCH"

report_arch=()
report_bits=()
report_endian=()
report_tests=()
report_status=()
report_note=()

record() {
    report_arch+=("$1")
    report_bits+=("$2")
    report_endian+=("$3")
    report_tests+=("$4")
    report_status+=("$5")
    report_note+=("$6")
}

show_tail() {
    [ -s "$1" ] || return 0
    tail -n 20 "$1" | sed -e 's/^/    | /'
}

run_target() {
    local requested="$1"
    if ! rust_info "$requested"; then
        echo "error: unknown target '$requested'"
        record "$requested" "-" "-" "-" "error" "unknown target"
        FAILED=1
        return
    fi

    local native=0
    [ "$ARCH" = "$HOST_ARCH" ] && native=1
    local label="$ARCH $RUST_TARGET $BITS-bit $ENDIAN endian"
    local log="$LOG_ROOT/$ARCH.log"
    local target_dir="$PROJECT_ROOT/target/$RUST_TARGET"
    [ "$native" -eq 1 ] && target_dir="$PROJECT_ROOT/target"
    mkdir -p "$LOG_ROOT"

    if [ "$native" -eq 0 ]; then
        local missing=()
        [ -x "/usr/bin/$TRIPLE-gcc" ] || missing+=("gcc-$TRIPLE")
        [ -d "/usr/$TRIPLE" ] || missing+=("libc6-dev-$DEBARCH-cross")
        command -v "$QEMU" >/dev/null 2>&1 || missing+=("qemu-user")
        if [ "${#missing[@]}" -gt 0 ]; then
            local hint="missing ${missing[*]}"
            if [ "$ALLOW_MISSING" -eq 1 ]; then
                printf 'skip  %s\n      %s\n' "$label" "$hint"
                record "$ARCH" "$BITS" "$ENDIAN" "-" "skip" "$hint"
                SKIPPED=$((SKIPPED + 1))
            else
                printf 'error %s\n      %s (--allow-missing to skip)\n' "$label" "$hint"
                record "$ARCH" "$BITS" "$ENDIAN" "-" "error" "$hint"
                FAILED=1
            fi
            return
        fi
    fi

    printf 'test  %s\n' "$label"
    if [ "$CLEAN" -eq 1 ]; then
        rm -rf "$target_dir"
    fi
    if [ "$native" -eq 0 ] && ! rustup target list --installed 2>/dev/null | grep -qx "$RUST_TARGET"; then
        echo "  rustup target add $RUST_TARGET"
        if ! rustup target add "$RUST_TARGET" >"$log" 2>&1; then
            echo "  rustup target add failed"
            show_tail "$log"
            record "$ARCH" "$BITS" "$ENDIAN" "-" "error" "rustup target add failed"
            FAILED=1
            return
        fi
    fi

    local env_name
    env_name="$(printf '%s' "$RUST_TARGET" | tr 'a-z-' 'A-Z_')"
    local -a env_args=()
    local -a cargo_args=(test --all --jobs "$JOBS")
    if [ "$native" -eq 0 ]; then
        env_args+=(
            "CARGO_TARGET_${env_name}_LINKER=$TRIPLE-gcc"
            "CARGO_TARGET_${env_name}_RUNNER=$QEMU -L /usr/$TRIPLE"
        )
        cargo_args+=(--target "$RUST_TARGET")
    fi

    if ! env "${env_args[@]}" cargo "${cargo_args[@]}" "${EXTRA_ARGS[@]}" >"$log" 2>&1; then
        echo "  tests failed"
        grep -E "^(error|warning: unused|failures:)|FAILED" "$log" | head -n 10 | sed -e 's/^/    | /'
        show_tail "$log"
        record "$ARCH" "$BITS" "$ENDIAN" "-" "error" "tests failed"
        FAILED=1
        return
    fi

    local passed
    passed="$(sed -nE 's/^test result: ok\. ([0-9]+) passed.*/\1/p' "$log" | awk '{s += $1} END {print s + 0}')"

    local elf="" status="ok" note=""
    local elf_bin
    elf_bin="$(find "$target_dir/debug/deps" -maxdepth 1 -type f -executable -name '*-*' 2>/dev/null | head -n 1)"
    if [ -z "$elf_bin" ]; then
        status="error" note="no test binary found"
    elif ! command -v file >/dev/null 2>&1; then
        note="file not installed, ELF unchecked"
    else
        elf="$(file -b "$elf_bin")"
        case "$elf" in
            *"ELF $BITS-bit LSB"*) [ "$ENDIAN" = little ] || { status="bad-elf" note="ELF reports LSB"; } ;;
            *"ELF $BITS-bit MSB"*) [ "$ENDIAN" = big ] || { status="bad-elf" note="ELF reports MSB"; } ;;
            *) status="bad-elf" note="not $BITS-bit" ;;
        esac
    fi
    [ "$status" = "ok" ] || FAILED=1
    printf '  %s tests, elf: %s %s\n' "$passed" "$BITS-bit" "$ENDIAN"
    record "$ARCH" "$BITS" "$ENDIAN" "$passed" "$status" "$note"
}

TARGETS=()
EXTRA_ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        -c | --clean) CLEAN=1 ;;
        -a | --all)
            read -r -a TARGETS <<<"$ALL_TARGETS"
            ALLOW_MISSING=1
            ;;
        --allow-missing) ALLOW_MISSING=1 ;;
        -l | --list)
            echo "$ALL_TARGETS" | tr ' ' '\n'
            exit 0
            ;;
        -j | --jobs)
            shift
            JOBS="${1:-$JOBS}"
            ;;
        -j*) JOBS="${1#-j}" ;;
        -h | --help)
            usage
            exit 0
            ;;
        --)
            shift
            EXTRA_ARGS+=("$@")
            break
            ;;
        -*)
            echo "error: unknown option '$1'" >&2
            usage >&2
            exit 2
            ;;
        *) TARGETS+=("$1") ;;
    esac
    shift
done

[ "${#TARGETS[@]}" -gt 0 ] || TARGETS=("$HOST_ARCH")

for target in "${TARGETS[@]}"; do
    run_target "$target"
done

printf '\n%-8s %-4s %-8s %-8s %-8s %s\n' "target" "bits" "endian" "tests" "status" "note"
for i in "${!report_arch[@]}"; do
    printf '%-8s %-4s %-8s %-8s %-8s %s\n' \
        "${report_arch[$i]}" "${report_bits[$i]}" "${report_endian[$i]}" \
        "${report_tests[$i]}" "${report_status[$i]}" "${report_note[$i]}"
done

if [ "$FAILED" -ne 0 ]; then
    printf '\n%s\n' "one or more targets failed"
    exit 1
fi
if [ "$SKIPPED" -ne 0 ]; then
    printf '\n%s\n' "all targets passed, $SKIPPED skipped"
else
    printf '\n%s\n' "all targets passed"
fi
