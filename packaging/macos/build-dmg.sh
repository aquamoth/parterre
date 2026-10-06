#!/bin/sh
# Builds parterre.app from a release build and puts it in a disk image beside a link to
# /Applications, to drag it onto. Needs macOS (codesign, hdiutil) and cargo-about.
#
#   cargo build --release [--target TRIPLE]
#   packaging/macos/build-dmg.sh [--target TRIPLE] [--version VERSION] [OUT]
#
# OUT defaults to target/packages, and the image is parterre-VERSION-TRIPLE.dmg. VERSION
# defaults to the binary's (parterre --version). A binary cross-compiled for Intel can't tell
# it on Apple silicon, so the release workflow passes the version it names its files with. It
# builds the image from a build stamped with its channel (PARTERRE_CHANNEL=dmg, #258).
#
# The app is signed ad hoc: Apple silicon runs nothing unsigned, and the signature seals
# Info.plist and the resources with the binary. It has no Developer ID, so Gatekeeper asks once
# before the first start (docs/building.md#macos-app).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
target=
version=
while [ $# -gt 1 ]; do
    case $1 in
    --target) target=$2 ;;
    --version) version=$2 ;;
    *) break ;;
    esac
    shift 2
done
out=${1:-target/packages}
dir=target/${target:+$target/}release
bin=$dir/parterre
if [ ! -x "$bin" ]; then
    echo "no binary at $bin; run 'cargo build --release${target:+ --target $target}' first" >&2
    exit 1
fi
[ -n "$target" ] || target=$(rustc -vV | sed -n 's/^host: //p')
[ -n "$version" ] || version=$("$bin" --version | cut -d' ' -f2)

# Bundle versions are numbers only, so a pre-release has its release's: 0.5.0-rc1 is 0.5.0.
numeric=$(echo "$version" | sed -n 's/^\([0-9]*\.[0-9]*\.[0-9]*\).*/\1/p')
if [ -z "$numeric" ]; then
    echo "$version is not a version" >&2
    exit 1
fi
# The oldest macOS the binary loads on, as the linker recorded it: rustc's default for the
# target (10.12 on Intel, 11.0 on Apple silicon), or MACOSX_DEPLOYMENT_TARGET. The Intel
# binary has it as the version of LC_VERSION_MIN_MACOSX, the other as minos of LC_BUILD_VERSION.
minimum=$(otool -l "$bin" | awk '
    $2 == "LC_VERSION_MIN_MACOSX" || $2 == "LC_BUILD_VERSION" { found = 1 }
    found && ($1 == "version" || $1 == "minos") { print $2; exit }')
if [ -z "$minimum" ]; then
    echo "found no minimum macOS version in $bin" >&2
    exit 1
fi
# NOTICE's, without the email address, as on the Windows executable.
copyright=$(grep -m1 '^Copyright' NOTICE | sed 's/ *<.*//')

stage=$dir/packaging/dmg
app=$stage/parterre.app
rm -rf "$stage"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$out"
cp "$bin" "$app/Contents/MacOS/"
cp packaging/icon/parterre.icns LICENSE NOTICE "$app/Contents/Resources/"
cargo about generate --locked -c packaging/about.toml packaging/about.hbs \
    -o "$app/Contents/Resources/THIRD-PARTY-NOTICES.html"
sed -e "s|@VERSION@|$numeric|" -e "s|@MINIMUM_SYSTEM_VERSION@|$minimum|" \
    -e "s|@COPYRIGHT@|$copyright|" packaging/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
ln -s /Applications "$stage/Applications"

# HFS+, which every macOS reads. hdiutil sometimes fails with "Resource busy" on GitHub's
# runners, so it gets three tries.
dmg=$out/parterre-$version-$target.dmg
for try in 1 2 3; do
    hdiutil create -volname parterre -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg" && break
    [ "$try" = 3 ] && exit 1
    sleep 5
done
ls -l "$dmg"
