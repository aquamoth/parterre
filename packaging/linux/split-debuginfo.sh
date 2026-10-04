#!/bin/sh
# Moves the debug info of a release build of parterre into DIR/parterre.debug and strips the
# binary, as `strip = "symbols"` would have. The release workflow builds with
# CARGO_PROFILE_RELEASE_STRIP=none and uploads DIR to PostHog, which resolves the stack traces
# of crash reports with it (docs/releasing.md#crash-report-symbols). Needs binutils.
#
#   CARGO_PROFILE_RELEASE_STRIP=none cargo build --release [--target TRIPLE]
#   packaging/linux/split-debuginfo.sh target/[TRIPLE/]release/parterre DIR
set -eu

if [ $# -ne 2 ]; then
    echo "usage: $0 BINARY DIR" >&2
    exit 1
fi
bin=$1
dir=$2

# PostHog finds the symbols of a crash by the GNU build ID, which both files keep.
if ! readelf -n "$bin" | grep -q 'Build ID'; then
    echo "$bin has no GNU build ID" >&2
    exit 1
fi
if ! readelf -S "$bin" | grep -q '\.debug_line'; then
    echo "$bin has no line tables; build it with CARGO_PROFILE_RELEASE_STRIP=none" >&2
    exit 1
fi
mkdir -p "$dir"
objcopy --only-keep-debug "$bin" "$dir/parterre.debug"
strip --strip-all "$bin"
