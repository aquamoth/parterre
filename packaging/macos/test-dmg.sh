#!/bin/sh
# Installs parterre.app from a disk image as a user would, into /Applications, checks that it
# runs, also when Launch Services starts it as the Dock and Finder do, and removes it again.
# The release workflow runs it on Apple silicon and on Intel before it publishes.
#
#   packaging/macos/test-dmg.sh parterre-0.5.1-aarch64-apple-darwin.dmg REPOSITORY
#
# Launch Services starts the app with a screenshot of REPOSITORY, any git repository, so that
# it opens its window and exits by itself.
set -eu

dmg=$1
repo=$(cd "$2" && pwd)
app=/Applications/parterre.app
work=$(mktemp -d)
mount=$work/mount
mkdir "$mount"
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
trap 'hdiutil detach -quiet "$mount" || true' EXIT

fail() {
    echo "$*" >&2
    exit 1
}

[ "$(readlink "$mount/Applications")" = /Applications ] || fail "no link to /Applications"
[ ! -e "$app" ] || fail "$app is already there"
cp -R "$mount/parterre.app" /Applications/
codesign --verify --strict "$app" || fail "the signature of $app doesn't hold"
id=$(defaults read "$app/Contents/Info" CFBundleIdentifier)
[ "$id" = se.trustfall.parterre ] || fail "bundle identifier $id"
[ -f "$app/Contents/Resources/parterre.icns" ] || fail "no icon in $app"

version=$("$app/Contents/MacOS/parterre" --version)
echo "$version"
echo "$version" | grep -Eq '^parterre [0-9]+\.[0-9]+\.[0-9]+[^ ]*( \([0-9a-f]{7,}\))?$' ||
    fail "--version printed '$version'"

# open waits (-W) for a new instance (-n), whose output goes to files: Launch Services has no
# terminal to give it.
shot=$work/window.png
open -W -n "$app" --stdout "$work/stdout" --stderr "$work/stderr" \
    --args "$repo" --screenshot "$shot" || fail "open failed"
cat "$work/stdout" "$work/stderr"
[ -s "$shot" ] || fail "started by Launch Services, parterre saved no screenshot"

rm -rf "$app"
echo "Installed, ran and removed $(basename "$dmg")"
