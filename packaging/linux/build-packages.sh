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
metainfo=$dir/packaging/se.trustfall.parterre.metainfo.xml
cp packaging/linux/se.trustfall.parterre.metainfo.xml "$metainfo"
# Software centres show the newest release entry as the version, so a release gets one even
# before it is added to the file in the repository.
case $release in
*-*) ;;
*)
    if ! grep -q "<release version=\"$release\"" "$metainfo"; then
        sed -i "s|^\( *\)<releases>|&\n\1  <release version=\"$release\" date=\"$(date -u +%F)\"/>|" \
            "$metainfo"
    fi
    ;;
esac

cargo deb -p parterre --no-build --no-strip --no-dbgsym ${target:+--target "$target"} \
    --deb-version "$version" -o "$out/"
cargo generate-rpm -p crates/parterre ${target:+--target "$target"} \
    -s "version = \"$version\"" -o "$out/"
# The ~ stays inside the packages but not in their file names, which GitHub may rewrite.
for f in "$out"/parterre*~*; do
    [ -e "$f" ] && mv "$f" "$(echo "$f" | tr '~' -)"
done
ls -l "$out"
