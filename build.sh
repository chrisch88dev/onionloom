#!/usr/bin/env bash
# Build a local, host-optimized development binary.

set -euo pipefail

project_root="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$project_root"

gpu_mode="auto"
if (( $# > 1 )); then
    printf '%s\n' "usage: ./build.sh [--gpu|--cpu-only]" >&2
    exit 2
fi
case "${1:-}" in
    "")
        ;;
    --gpu)
        gpu_mode="required"
        ;;
    --cpu-only)
        gpu_mode="disabled"
        ;;
    *)
        printf '%s\n' "usage: ./build.sh [--gpu|--cpu-only]" >&2
        exit 2
        ;;
esac

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' "error: cargo is required; install Rust 1.85 or newer" >&2
    exit 1
fi

build_flags=(
    "-Ctarget-cpu=native"
    "-Cforce-frame-pointers=no"
    "--remap-path-prefix=${project_root}=."
)
rust_sysroot="$(rustc --print sysroot)"
build_flags+=("--remap-path-prefix=${rust_sysroot}=/rustc")

cargo_binary="$(command -v cargo)"
cargo_home_source="${CARGO_HOME:-$(dirname -- "$cargo_binary")/..}"
cargo_home_candidate="$(CDPATH='' cd -- "$cargo_home_source" && pwd -P)"
if [[ -d "$cargo_home_candidate/registry" ]]; then
    build_flags+=("--remap-path-prefix=${cargo_home_candidate}=/cargo")
fi

cargo_features=()
build_description="host-native CPU and automatic GPU"
if [[ "$gpu_mode" == "disabled" ]]; then
    cargo_features=(--no-default-features)
    build_description="host-native CPU"
elif [[ "$gpu_mode" == "required" ]]; then
    cargo_features=(--features gpu)
fi

case "$(uname -s)" in
    Linux)
        build_flags+=("-Clink-arg=-Wl,-z,relro,-z,now,-z,noexecstack")
        binary_name="onionloom"
        ;;
    MINGW*|MSYS*|CYGWIN*)
        binary_name="onionloom.exe"
        ;;
    *)
        binary_name="onionloom"
        ;;
esac

encoded_flags=$(IFS=$'\x1f'; printf '%s' "${build_flags[*]}")
export CARGO_ENCODED_RUSTFLAGS="$encoded_flags"
export ONIONLOOM_BUILD_TUNING="host-native"
unset RUSTFLAGS
export CARGO_TARGET_DIR="$project_root/target"

cargo build --locked --release "${cargo_features[@]}"

built_binary="$CARGO_TARGET_DIR/release/$binary_name"
local_binary="$project_root/$binary_name"
if [[ ! -f "$built_binary" ]]; then
    printf '%s\n' "error: expected build output at $built_binary" >&2
    exit 1
fi

staged_binary="$(mktemp "$project_root/.${binary_name}.build.XXXXXX")"
cleanup() {
    if [[ -n "${staged_binary:-}" && -f "$staged_binary" ]]; then
        rm -f -- "$staged_binary"
    fi
}
trap cleanup EXIT
install -m 0755 "$built_binary" "$staged_binary"

if [[ "$(uname -s)" == "Linux" ]] && command -v strip >/dev/null 2>&1; then
    strip --strip-unneeded "$staged_binary"
fi

if [[ "$(uname -s)" == "Linux" ]] && command -v readelf >/dev/null 2>&1; then
    if readelf --wide --sections "$staged_binary" | grep -F ".debug_" >/dev/null; then
        printf '%s\n' "error: local binary still contains debug sections" >&2
        exit 1
    fi
fi

if command -v strings >/dev/null 2>&1 \
    && strings -a "$staged_binary" | grep -F -- "$project_root" >/dev/null; then
    printf '%s\n' "error: local binary contains the absolute project path" >&2
    exit 1
fi

if command -v strings >/dev/null 2>&1 \
    && strings -a "$staged_binary" \
        | grep -E '(/home/[^/]+/|/Users/[^/]+/|[A-Za-z]:\\Users\\[^\\]+\\)' >/dev/null; then
    printf '%s\n' "error: local binary contains an absolute user-home path" >&2
    exit 1
fi

mv -f -- "$staged_binary" "$local_binary"
staged_binary=""

printf '%s\n' "Built host-optimized local binary: $local_binary"
printf '%s\n' "Included engines: $build_description"
