# Flatpak (draft)

`se.trustfall.parterre.yml` is a draft Flatpak manifest for Flathub (#21), kept as a reference.
**It is not to be submitted as it is.** An AI assistant wrote it, and Flathub's generative-AI
policy doesn't accept AI-written manifests, submission pull requests or review replies; the
maintainer writes those (see `docs/distribution.md`).

What it does: builds parterre from source on the freedesktop runtime with the rust-stable SDK
extension, offline, with the crates vendored; builds git from kernel.org's tarball, as the
runtime has no git; and asks for `--filesystem=home:ro`, for which Flathub needs an exception.

## Building it locally

With `flatpak-builder` installed, and `uv` or `python3` with `aiohttp` and `tomlkit`:

    packaging/flatpak/build.sh           # build and install for the current user
    flatpak run se.trustfall.parterre
    packaging/flatpak/build.sh --bundle  # also write build/parterre.flatpak

The first build downloads the SDK and the Rust extension, about 1.5 GB. Built from a copy of
the checkout, `--version` says e.g. `0.4.0-dev` (Cargo.toml's version). Built from a tagged
`type: git` source with `PARTERRE_RELEASE_TAG` set, as Flathub's would be, it says
`0.5.1 (f674e16)`.

Flathub's linter (`flatpak install flathub org.flatpak.Builder`) should report only
`finish-args-home-ro-filesystem-access`, the exception to ask for:

    lint="flatpak run --command=flatpak-builder-lint org.flatpak.Builder"
    $lint manifest packaging/flatpak/se.trustfall.parterre.yml
    $lint builddir packaging/flatpak/build/app
    $lint repo packaging/flatpak/build/repo

## Known gaps

- Pull requests: parterre asks `gh auth token`, and there is no `gh` in the sandbox.
- git reads `~/.gitconfig`, but not `~/.config/git/config`: Flatpak points `XDG_CONFIG_HOME`
  at `~/.var/app/se.trustfall.parterre/config`.
- No *Revision Graph* in the file manager's context menu: a Flatpak can't add to the host's
  file managers, as the .deb and .rpm do (`packaging/linux/file-managers`).
- A folder outside home arrives through the document portal (`/run/user/*/doc/...`), where
  linked worktrees don't work.

## Generated, not committed

- `cargo-sources.json`: the crates in `Cargo.lock`, for building offline. `build.sh` makes it
  with `flatpak-cargo-generator.py` from flatpak-builder-tools (fetched at a pinned commit).
  Flathub's repository for the app would commit its own copy, regenerated with each release.
- `build/` (or `$FLATPAK_WORK`): flatpak-builder's cache, the build directory, the repository
  and the bundle.
