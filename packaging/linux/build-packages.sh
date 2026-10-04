#!/bin/sh
# Builds the .deb and .rpm of a release build, with the files the metadata in
# crates/parterre/Cargo.toml names. Needs cargo-deb, cargo-generate-rpm and cargo-about.
#
#   cargo build --release [--target TRIPLE]
#   packaging/linux/build-packages.sh [--target TRIPLE] [OUT]   (default OUT: target/packages)
#
# The packages take their version from the binary: the release tag's in the release workflow,
# X.Y.Z-dev.N+commit otherwise. A pre-release's - becomes ~, which sorts before the release in
# both dpkg and rpm, so 0.5.0-rc1 is packaged as 0.5.0~rc1 and 0.5.0 replaces it.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
target=
if [ "${1-}" = --target ]; then
    target=$2
    shift 2
fi
out=${1:-target/packages}
dir=target/${target:+$target/}release
bin=$dir/parterre
if [ ! -x "$bin" ]; then
    echo "no binary at $bin; run 'cargo build --release${target:+ --target $target}' first" >&2
    exit 1
fi

release=$("$bin" --version | cut -d' ' -f2)
version=$(echo "$release" | tr - '~')

# The generated files the metadata expects next to the binary.
mkdir -p "$dir/packaging" "$out"
cargo about generate --locked -c packaging/about.toml packaging/about.hbs \
    -o "$dir/packaging/THIRD-PARTY-NOTICES.html"
packaging/linux/metainfo.sh "$release" "$dir/packaging/se.trustfall.parterre.metainfo.xml"

cargo deb -p parterre --no-build --no-strip --no-dbgsym ${target:+--target "$target"} \
    --deb-version "$version" -o "$out/"
# Its own automatic requirements, but for weak glibc versions (rpm-find-requires.sh).
# cargo-generate-rpm ignores how the script exits, so a broken one would go unnoticed.
find_requires=$root/packaging/linux/rpm-find-requires.sh
if ! echo "$root/$bin" | "$find_requires" | grep -q '^libc\.so\.6()'; then
    echo "$find_requires found no libc requirement for $bin" >&2
    exit 1
fi
cargo generate-rpm -p crates/parterre ${target:+--target "$target"} \
    --auto-req "$find_requires" \
    -s "version = \"$version\"" -o "$out/"
# The ~ stays inside the packages but not in their file names, which GitHub may rewrite.
for f in "$out"/parterre*~*; do
    [ -e "$f" ] && mv "$f" "$(echo "$f" | tr '~' -)"
done
ls -l "$out"
