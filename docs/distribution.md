# Distribution

Where parterre is published, under which names, and why. Decided on 2026-09-25. The facts below
were checked on that date, and sources are linked; stores change their rules, so check again
before acting on an old fact.

## Channels

| Channel | Name | When | Issue |
|---|---|---|---|
| GitHub Releases | zip (Windows), tar.gz (Linux, macOS), `SHA256SUMS` | as today | |
| crates.io | `parterre`, `parterre-core` | 0.4.0 | #13 |
| Windows MSI on GitHub Releases | Manufacturer "Trustfall AB" | next | #15 |
| winget | `Trustfall.Parterre`, moniker `parterre` | next | #16 |
| Chocolatey | `parterre` | next | #17 |
| .deb and .rpm on GitHub Releases | `parterre`, app ID `se.trustfall.parterre` | next | #18 |
| Snap Store | `parterre` | next | #19 |
| Flathub | `se.trustfall.parterre` | later | #21 |

Everything except crates.io waited for the new icon (#14, now in), not for 1.0. After the
first, hand-made submission to each channel, a tag push publishes to all of them behind one
approval (#20). Not planned for now:

- **AUR**: not until someone asks. New AUR accounts can't be registered at the moment anyway
  (closed after the summer 2026 malware incidents).
- **A package repository of our own** (apt/dnf, so `apt upgrade` brings new versions): later,
  if at all. An Ubuntu PPA isn't possible yet: Launchpad builds with Ubuntu's Rust, and no
  Ubuntu release has 1.95, which egui needs.
- **Debian, Fedora and Arch official repositories**: left to distribution volunteers. Debian
  alone would need about 28 new Rust packages for the egui stack. Their builds can leave out
  the update check, usage statistics and crash reports with the
  [`send` feature](building.md#features).
- **Homebrew**: homebrew-core wants 225 stars for an author's own submission and doesn't take
  GUI apps as formulae.
- **AppImage**: no name registry, so it claims nothing.
- **ARM64** (Windows and Linux): a separate decision when someone asks. GitHub's ARM runners
  are free for public repositories.
- **macOS installers**: none until someone with a Mac can test them. The tarballs stay.
- **Code signing**: the MSI is unsigned for now (see [Windows](#windows)).

## Identity

- **Publisher: Trustfall AB.** Copyright stays with Mattias Åslund.
- **App ID `se.trustfall.parterre`** for the Linux desktop entry, AppStream metadata and
  Wayland app ID, and later Flathub. Flathub verifies a domain ID through a token on
  `https://trustfall.se/.well-known/`, and changing an ID after publishing is costly.
- **Only the namespaces are claimed; no trademark.** No trademark "parterre" for software
  (classes 9 or 42) was found in the EU, Sweden or the US: TMview and USPTO, 2026-09-25.

On 2026-09-25 the name was free on crates.io, winget, Chocolatey, Scoop, Flathub, the Snap
Store, AUR, Debian, Ubuntu, Fedora, nixpkgs and Homebrew. PyPI's `parterre` is a 2009
board-game package. The one clash is [Chillsbro/parterre](https://github.com/Chillsbro/parterre),
an MIT-licensed TypeScript terminal tool for watching AI agents browse, public since 2026-07-20.
Its command is also `parterre`, and it is published on no channel. We keep the name.

## git

parterre runs the `git` command-line tool, so every package needs git:

| Channel | How |
|---|---|
| winget | `PackageDependencies: Git.Git` with `MinimumVersion: 2.31.0`, installed automatically since winget 1.6. Must be in the first version: a dependency added later isn't installed on upgrade. TortoiseGit's manifest does the same. |
| Chocolatey | dependency on the `git` package, version 2.31.0 or newer |
| .deb / .rpm | `Depends: git (>= 1:2.31)` / `Requires: git-core >= 2.31` (git without Perl and the GUIs, on Fedora and openSUSE) |
| Snap | git bundled (the sandbox can't reach the host's git) |
| Flathub | git bundled; the freedesktop runtime has none |
| crates.io, zip, tarballs | documented in the README |

The minimum is git 2.31 (`parterre_core::git::MINIMUM_VERSION`,
[research](research/git-version-support.md)). Every channel declares it, so that raising it
later is gated everywhere; `crates/parterre/tests/packaging.rs` fails until they agree. A
channel that bundles git must bundle one at least that new. parterre also checks `git --version`
before it opens a repository, for the MSI, the zip, `cargo install` and a git changed after
installing (#233).

Bundling git on Windows (MinGit) was rejected. It unpacks to about 90 MB, it is GPLv2-only so
its source would have to ship alongside, and every git security fix would need a parterre
release. Switching to a git library (gitoxide) would be a large rewrite; see
`crates/parterre-core/src/git.rs` for why parterre uses the CLI.

## Update check

parterre asks GitHub's releases API at start and then once a day whether a newer release is out
(#226, #258). It sends nothing of its own. A newer one turns the ☰ icon bold blue and ends the
menu with *Download X.Y.Z*, which does what fits the channel parterre came through:

| Channel | *Download* |
|---|---|
| MSI (also winget and Chocolatey, which install it), zip, tarballs, .deb, .rpm | opens that channel's own file of the release in the browser |
| crates.io (`cargo install`), and any build without a channel | copies `cargo install --locked parterre` |
| Snap, Flathub | nothing: their stores update parterre, and there is no check |

- **The channel is stamped at build time.** Each packaging job in `release.yml` builds with
  `PARTERRE_CHANNEL` set to `msi`, `zip`, `tarball`, `deb` or `rpm`, so the installer and the
  packages each get a build of their own. Snap and Flatpak are told at run time by
  `SNAP_NAME=parterre` and `FLATPAK_ID=se.trustfall.parterre`.
- **Pre-releases:** a release build compares with the latest release, a release candidate (or
  a dev build) with the newest of any kind, so it hears of the next candidate and the final.
- **Off switches:** *Check for updates* in Settings › Privacy, for users. For packagers, the
  `send` feature: built without it, parterre makes no requests of its own at all
  ([building.md](building.md#features)).

## Windows

- **Per-user MSI** built with WiX v7. WiX's Open Source Maintenance Fee applies only to users
  with revenue ≥ US$10,000 from the activity, so a free project is exempt. `cargo-wix` and
  `dist` still use WiX v3, which is out of support.
- **Scope:** dual-purpose, per-user by default and machine-wide with `ALLUSERS=1`, which
  Chocolatey passes.
- **What it installs:** a Start menu shortcut and `parterre` on PATH, with no desktop
  shortcut, and *Revision Graph* in Explorer's context menu for folders (#11).
- **winget:** unsigned installers are accepted. Every new package is reviewed by a moderator,
  typically in 1–3 weeks. Updates can be automated with komac, which needs a classic
  `public_repo` token. Fine-grained tokens don't work, so the token belongs to a machine
  account that owns only a fork of winget-pkgs.
- **Unsigned:**
  - winget and Chocolatey downloads show no SmartScreen prompt.
  - A download straight from GitHub shows "Windows protected your PC" (→ *More info* → *Run
    anyway*), as the zip does today.
  - Windows 11 with Smart App Control turned on blocks unsigned programs outright.
- **Signing options if that changes:**
  - Azure Artifact Signing, about $10/month. It accepts EU organisations such as Trustfall AB,
    but individuals only in the US and Canada. The publisher shown is Trustfall AB.
  - SignPath Foundation: free for open source, but the publisher shown is "SignPath
    Foundation".
  - EV certificates no longer give instant SmartScreen reputation.

Sources:
[winget-pkgs docs](https://github.com/microsoft/winget-pkgs/tree/master/doc),
[winget install](https://learn.microsoft.com/en-us/windows/package-manager/winget/install),
[TortoiseGit manifest](https://github.com/microsoft/winget-pkgs/tree/master/manifests/t/TortoiseGit/TortoiseGit),
[komac](https://github.com/russellbanks/Komac),
[winget-releaser](https://github.com/vedantmgoyal9/winget-releaser),
[WiX OSMF](https://docs.firegiant.com/wix/osmf/),
[Artifact Signing](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart),
[SignPath Foundation](https://signpath.org/terms.html),
[SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

## Linux

- **Oldest supported systems.** Release builds for Linux run in an `ubuntu:22.04` container, so
  the binary needs glibc 2.35 or newer. That reaches Debian 12+, Ubuntu 22.04+, Zorin 17+,
  Mint 21+, Fedora and openSUSE Leap 15.6.
  - The Rust compiler still comes from rustup. The container only supplies the C linker and
    glibc's link stubs.
  - glibc and the display libraries are loaded from the user's system at run time.
  - `ubuntu-latest` would move the baseline to glibc 2.43 (Ubuntu 26.04) when GitHub switches
    it between 2026-10-19 and 2026-11-19.
  - Raise the baseline when Ubuntu 22.04's standard support ends in April 2027.
- **.deb and .rpm** declare their dependencies by hand: git and the libraries loaded at run
  time (EGL/GL, Wayland, X11, xkbcommon). `cargo-deb` and `cargo-generate-rpm` only detect
  libc. Details in [building.md](building.md#linux-packages).
- **App ID in the window.** The Wayland app ID and X11 window class are `se.trustfall.parterre`,
  like the desktop entry and icons, so desktops pair the window with its icon. The settings stay
  in `~/.local/share/parterre`.
- **Folders get *Revision Graph* in the file manager's context menu, not *Open With*.**
  (Decided 2026-09-27.) `MimeType=inode/directory` in the desktop entry would list parterre
  under *Open With* for folders, but where the desktop names no default folder app it makes
  parterre the default. GNOME, KDE and Cinnamon name one; Xfce, MATE, LXQt and plain window
  managers don't, and GIO then takes the first that sorts alphabetically. Tried on Ubuntu 24.04
  with Thunar: `xdg-open` and `exo-open` opened folders in parterre. `NoDisplay`, `OnlyShowIn`
  and `NotShowIn` don't change GIO's choice. So the packages add a menu item to Nautilus (via
  nautilus-python), Dolphin and Nemo, each file manager's own way; Thunar, Caja and PCManFM-Qt
  get none. See [building.md](building.md#revision-graph-in-file-managers).
- **Snap:**
  - Strict confinement. Classic is "reserved for mature, well-known applications", and new
    projects are refused.
  - parterre then sees repositories under the home folder, and `/media` and `/mnt` if the user
    connects `removable-media`.
  - `~/.gitconfig` isn't read.
  - A name registration is reviewed by hand. An unused name can be revoked, so register it
    just before the first upload.
- **Flathub, later:**
  - Flathub turns down apps that have "only existed for a very short period of time".
  - Its generative-AI policy (September 2026) requires disclosing AI-assisted code.
  - The manifest, the submission pull request and review replies must not be AI-written, so
    the maintainer writes them. In August 2026 Flathub closed three new git GUIs within a day
    under these rules.
  - Build from source only, with git bundled.
  - `--filesystem=home:ro` needs an exception, which git GUIs have been granted.
  - Directories chosen through the portal break linked worktrees, so a portal alone isn't
    enough.
  - `packaging/flatpak` has a draft manifest that builds from source and runs (written
    2026-09-27 by an AI assistant, so for reference only, not for submitting). What it found:
    git has to be built too (the freedesktop runtime has none); pull requests need a token
    without `gh`, which isn't in the sandbox; and git inside doesn't read
    `~/.config/git/config`.

Sources:
[Flathub requirements](https://docs.flathub.org/docs/for-app-authors/requirements),
[Flathub linter](https://docs.flathub.org/docs/for-app-authors/linter),
[Flathub ID renames](https://docs.flathub.org/docs/for-app-authors/maintenance#renaming-the-flatpak-id),
[Snap classic confinement](https://forum.snapcraft.io/t/process-for-reviewing-classic-confinement-snaps/1460),
[Snap name review](https://forum.snapcraft.io/t/manual-review-of-all-new-snap-name-registrations/39440),
[runner images: Ubuntu 26.04](https://github.blog/changelog/2026-09-17-ubuntu-26-generally-available-and-latest-migration/),
[runner images: 22.04 retirement](https://github.com/actions/runner-images/issues/14254),
[cargo-deb](https://github.com/kornelski/cargo-deb),
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm),
[AUR registration](https://lists.archlinux.org/archives/list/aur-general@lists.archlinux.org/thread/MZKFOZTW6HX7SU2YZIQEWFLF4EMWTF4O/).

## crates.io

- **Applications are welcome there.** Alacritty, Neovide and several egui apps are installed
  with `cargo install`. Squatting a name without a working crate isn't.
- **`parterre-core` is published too**, because crates.io refuses path-only dependencies. It is
  described as internal, with no stability promises.
- **Installing:** `cargo install --locked parterre` builds from source and installs only the
  binary, without a desktop entry. `cargo binstall parterre` downloads the release archive
  instead: their names already follow its default pattern.
- **Version:** a published crate has no `.git`, so `build.rs` reads the commit from
  `.cargo_vcs_info.json`. A build of the published sources shows the plain version, like a
  release.
- **Publishing:** the first version is published by hand with a personal token. Later ones use
  Trusted Publishing from the release workflow, with no stored token.

Sources:
[specifying dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html),
[crates.io policies](https://crates.io/policies),
[Trusted Publishing](https://crates.io/docs/trusted-publishing),
[cargo package](https://doc.rust-lang.org/cargo/commands/cargo-package.html),
[cargo-binstall](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md).
