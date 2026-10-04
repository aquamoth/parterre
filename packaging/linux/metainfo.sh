#!/bin/sh
# Writes the AppStream metadata, se.trustfall.parterre.metainfo.xml, to OUT with a <release>
# for each release tag, newest first, dated by its tag. Pre-releases are left out. A release
# being built without its tag in the checkout (a source archive) gets an entry dated today, as
# software centres show the newest entry as the version.
#
#   packaging/linux/metainfo.sh VERSION OUT   (VERSION as parterre --version prints it)
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
version=$1
out=$2

tags=
# Only the workspace's own repository, not one it was unpacked inside.
if command -v git >/dev/null &&
    [ "$(git -C "$root" rev-parse --show-toplevel 2>/dev/null)" = "$root" ]; then
    tags=$(git -C "$root" for-each-ref --sort=-v:refname \
        --format='%(refname:short) %(creatordate:short)' 'refs/tags/v[0-9]*' |
        grep -E '^v[0-9]+\.[0-9]+\.[0-9]+ ' || true)
fi
case $version in
*-*) ;;
*)
    if ! echo "$tags" | grep -q "^v$version "; then
        tags=$(printf 'v%s %s\n%s' "$version" "$(date -u +%F)" "$tags")
    fi
    ;;
esac

releases=$(echo "$tags" | sed -n 's|^v\([^ ]*\) \(.*\)|<release version="\1" date="\2"/>|p')
mkdir -p "$(dirname "$out")"
RELEASES=$releases awk '
    { print }
    /<releases>/ {
        indent = $0
        sub(/<.*/, "", indent)
        n = split(ENVIRON["RELEASES"], line, "\n")
        for (i = 1; i <= n; i++) print indent "  " line[i]
    }
' "$root/packaging/linux/se.trustfall.parterre.metainfo.xml" >"$out"
