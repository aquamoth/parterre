#!/bin/sh
# Builds the draft Flatpak (se.trustfall.parterre.yml) from this checkout and installs it for
# the current user. Needs flatpak-builder, curl, and uv or python3 with aiohttp and tomlkit
# (for flatpak-cargo-generator.py). The first run downloads the SDK and the Rust extension,
# about 1.5 GB.
#
#   packaging/flatpak/build.sh           build and install (then: flatpak run se.trustfall.parterre)
#   packaging/flatpak/build.sh --bundle  also write parterre.flatpak, a single-file bundle
#
# flatpak-builder's cache, the build directory, the repository and the bundle go in
# $FLATPAK_WORK, by default packaging/flatpak/build (ignored by git). cargo-sources.json, the
# vendored crates' sources, is written next to the manifest, and not committed either.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=${FLATPAK_WORK:-$here/build}
id=se.trustfall.parterre

# flatpak-builder-tools has no releases, so pin a commit and check what it gives.
tools_commit=41c20aa10819cdb2a4f3ca171758a96d1955c018
generator_sha256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

bundle=false
case "${1-}" in
    --bundle) bundle=true ;;
    "") ;;
    *) echo "usage: $0 [--bundle]" >&2; exit 2 ;;
esac

mkdir -p "$work"
generator=$work/flatpak-cargo-generator.py
if ! echo "$generator_sha256  $generator" | sha256sum -c --status 2>/dev/null; then
    curl -fsSL -o "$generator" \
        "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$tools_commit/cargo/flatpak-cargo-generator.py"
    echo "$generator_sha256  $generator" | sha256sum -c --quiet
fi
# The script declares its dependencies (PEP 723), which uv installs on the fly.
if command -v uv >/dev/null; then
    uv run --quiet --script "$generator" "$root/Cargo.lock" -o "$here/cargo-sources.json"
else
    python3 "$generator" "$root/Cargo.lock" -o "$here/cargo-sources.json"
fi

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
# The screenshot and icon URLs as Flathub's own build writes them (org.flatpak.Builder's
# flathub-build), so that its linter doesn't flag them. Not flathub-build itself: its --sandbox
# refuses the `type: dir` source outside the manifest's directory.
flatpak-builder --user --install-deps-from=flathub --force-clean --install \
    --state-dir="$work/state" --repo="$work/repo" \
    --mirror-screenshots-url=https://dl.flathub.org/media --compose-url-policy=full \
    "$work/app" "$here/$id.yml"

if $bundle; then
    # The runtime repo lets `flatpak install parterre.flatpak` fetch the runtime from Flathub.
    flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
        "$work/repo" "$work/parterre.flatpak" "$id"
    echo "Wrote $work/parterre.flatpak"
fi
