# Building

## Linux (development machine)

```sh
cargo build --release
./target/release/parterre ~/some/repo
```

A Rust toolchain and a C compiler (`gcc` or `clang`, e.g. `build-essential` on Debian and
Ubuntu) are required: the TLS library and the syntax-colour grammars compile C in their build
scripts. No development packages of system libraries are needed. At runtime the window needs
the usual desktop libraries: Wayland or X11, libxkbcommon, and OpenGL (EGL/GLX), all of which
are present on any desktop.

## Features

All three are on by default. Leave one out with `--no-default-features --features …`.

| Feature | Brings |
|---|---|
| `github` | open pull requests from GitHub, and your GitHub repositories to clone (`parterre-forge`) |
| `send` | the update check, the usage statistics and crash reports sent to PostHog, and anything else parterre asks or sends over the network (`parterre-telemetry`) |
| `syntax` | syntax colour in the diff and blame windows (`parterre-highlight`) |

**Packagers:** `send` is your switch. Built without it, parterre makes no requests of its own:
it has no update check, so it never tells users of your package about releases you haven't
packaged yet, and sends no usage statistics or crash reports, so it doesn't ask about them at
first start either. Settings › Privacy shows those switches greyed out. Pull requests from
GitHub (`github`) are asked for only when the user shows them, and their repositories only
when they open *Clone repository…*. Without `send`, neither
PostHog's SDK nor its HTTP client (reqwest) is built.

Debug builds never send usage statistics or crash reports, `send` or not.

```sh
cargo build --release --locked --no-default-features --features github,syntax
```

A release build stamps where it is published in `PARTERRE_CHANNEL` (`msi`, `zip`, `tarball`,
`dmg`, `deb` or `rpm`), so that *Download* offers the same kind of file
([distribution.md](distribution.md#update-check)). Leave it unset in any other build: unset is
`cargo install`.

## Windows

Native build on Windows with the MSVC toolchain. Prerequisites:

1. The MSVC C++ build tools and a Windows SDK. Either install *Build Tools for Visual Studio*
   with the "Desktop development with C++" workload, or add the components to an existing
   Visual Studio (from an elevated prompt):

   ```powershell
   & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe" modify `
     --installPath "C:\Program Files\Microsoft Visual Studio\18\Professional" `
     --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
     --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --passive --norestart
   ```

2. Rust from [rustup](https://rustup.rs) with the default `x86_64-pc-windows-msvc` host.
   `rust-toolchain.toml` pulls in rustfmt and clippy.

Then:

```powershell
cargo build --release
.\target\release\parterre.exe C:\path\to\repo
```

The result is a single self-contained `target\release\parterre.exe`. `.cargo/config.toml` links
the C runtime statically, so it runs without the Visual C++ Redistributable. It needs only
`git`: the one on `PATH`, or else Git for Windows' `cmd\git.exe`, found through the
`InstallPath` its installer records in the registry or in its default folders
(`crates/parterre-core/src/git/program.rs`). That covers a terminal opened before Git for
Windows was installed.

Release builds use the GUI subsystem (no console window), and git is started with
`CREATE_NO_WINDOW` so no console flashes. To still show `--help`, errors and `--export` output
in a terminal, the program attaches to its parent's console at startup
(`crates/parterre/src/console.rs`, one of the few places with `unsafe`). It releases the console
before opening an interactive window, so closing the terminal doesn't close the window.
Shells don't wait for GUI programs, so the output may appear after the next prompt; press
Enter to get a fresh prompt. Debug builds are ordinary console programs.

Don't use the `x86_64-pc-windows-gnu` host toolchain as a shortcut around installing the MSVC
tools. Its bundled `dlltool` needs an assembler (`as.exe`) that the toolchain doesn't ship.
`windows-link` always uses `raw-dylib`, so the build fails with
`error calling dlltool` / `CreateProcess` unless a full MinGW-w64 is on `PATH`.

Cross-checking from Linux works without extra tools once the `github` and `send` features are
left out: their TLS library, ring, compiles C for the target, which needs MinGW-w64's headers
(`sudo apt install mingw-w64` for a check with it).

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --workspace --target x86_64-pc-windows-gnu --no-default-features
```

Producing a Windows `.exe` from Linux needs a linker. Options: `sudo apt install mingw-w64`
and then `cargo build --target x86_64-pc-windows-gnu`, or `cargo-zigbuild`, or `cargo-xwin`
(which downloads the MSVC CRT; that requires accepting Microsoft's license).

## Windows installer

The MSI comes from `packaging/windows/parterre.wxs`, built with WiX v7 (why WiX, and why
unsigned: [distribution.md](distribution.md#windows)). WiX only runs on Windows. Setup, once:

```powershell
dotnet tool install --global wix --version 7.0.0
cargo install cargo-about --locked --features cli
```

Then:

```powershell
cargo build --release
packaging\windows\build-msi.ps1   # → target\msi\parterre-<version>-x86_64-pc-windows-msvc.msi
```

The script gathers `parterre.exe`, `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES.html` in
`target\msi\stage`. With `-Stage DIR` it takes them from `DIR` instead, as the release
workflow does, with a `parterre.exe` built with `PARTERRE_CHANNEL=msi` (see
[Features](#features)); `-Out FILE` names the MSI. It passes `-acceptEula wix7` on every run, which
accepts WiX's Open Source Maintenance Fee EULA for that run only; don't run `wix eula accept`,
which leaves an acceptance file behind. The MSI version is the release tag's version in the
release workflow (`PARTERRE_RELEASE_TAG`), otherwise the version `parterre.exe` carries (from
the git tags, see [releasing.md](releasing.md#version-strings)), in both cases without its
pre-release part: `0.5.0-rc.1` and `0.5.0` both build MSI version `0.5.0`, and the one installed
later replaces the other. The script checks the built MSI's version.

The script ends with the ICE checks (`wix msi validate`), minus two it suppresses for reasons
given in `parterre.wxs`: ICE57 (the Start menu shortcut in a dual-purpose package) and ICE61
(same-version upgrades).

One MSI installs either way:

| | Per-user (default) | Machine-wide |
|---|---|---|
| Command | `msiexec /i parterre.msi` | `msiexec /i parterre.msi ALLUSERS=1` (elevated; Chocolatey does this) |
| Admin prompt | none | yes |
| Folder | `%LOCALAPPDATA%\Programs\parterre` | `C:\Program Files\parterre` |
| PATH | the user's | the system's |
| Start menu | the user's | all users' |
| Explorer entry | `HKCU\Software\Classes\Directory\…` | `HKLM\Software\Classes\Directory\…` |
| Registry (component key paths only) | `HKCU\Software\Trustfall AB\parterre` | `HKLM\Software\Trustfall AB\parterre` |

The Explorer entry, *Revision Graph*, is a shell verb under `Directory\shell` (a
folder) and `Directory\Background\shell` (the background of an open one), running
`parterre.exe "%V"`; Windows 11 lists it under *Show more options*. Started that way parterre
has no terminal, so a folder outside any repository opens the window with the error rather
than exiting.

Both show in *Settings → Apps* as parterre by Trustfall AB, without a Modify button, and
uninstall from there or with `msiexec /x`. Add `/qn` for a silent install and
`/l*v install.log` for a log. A newer MSI replaces the installed one, as does one with the same
version; an older one is refused. That only works within one scope: Windows Installer looks
for the installed version in the scope being installed, so a per-user install followed by a
machine-wide one leaves two entries.

`packaging\windows\test-msi.ps1 -Msi FILE -Scope user|machine` installs the MSI in one scope,
checks that `parterre --version` runs and that the folder is on that scope's `PATH`, and
uninstalls it again. The release workflow runs it in both scopes before publishing; the machine
scope needs an elevated prompt.

## Linux packages

The `.deb` and `.rpm` are built with `cargo-deb` and `cargo-generate-rpm` from a release build,
with the metadata in `crates/parterre/Cargo.toml` (why these two: [distribution.md](distribution.md#linux)).
Setup, once:

```sh
cargo install --locked cargo-deb cargo-generate-rpm cargo-about
```

Then:

```sh
cargo build --release
packaging/linux/build-packages.sh              # → target/packages/parterre_<version>_amd64.deb, .rpm
```

The release builds each package from a build stamped with its channel (see
[Features](#features)), and the same again with `rpm`:

```sh
PARTERRE_CHANNEL=deb cargo build --release --target x86_64-unknown-linux-gnu
packaging/linux/build-packages.sh --target x86_64-unknown-linux-gnu --only deb dist
```

The packages take their version from `parterre --version`, with a pre-release's `-` turned into
`~` so that `0.5.0~rc1` sorts before `0.5.0` in both dpkg and rpm. The file names keep the `-`,
since GitHub may rewrite a `~` in a release asset's name. The script also writes
`THIRD-PARTY-NOTICES.html` and the AppStream metadata next to the binary. The metadata's
releases come from the release tags, newest first and dated by the tag, without pre-releases
(`packaging/linux/metainfo.sh`); a release built without its tag gets an entry dated the day
it was built, since software centres show the newest entry as the version.

What they install:

| File | Where |
|---|---|
| `parterre` | `/usr/bin` |
| Desktop entry `se.trustfall.parterre.desktop` | `/usr/share/applications` |
| AppStream metadata | `/usr/share/metainfo` |
| Icons, named `se.trustfall.parterre` | `/usr/share/icons/hicolor` |
| *Revision Graph* for Dolphin | `/usr/share/kio/servicemenus` |
| *Revision Graph* for Nemo | `/usr/share/nemo/actions` |
| *Revision Graph* for Nautilus | `/usr/share/nautilus-python/extensions` |
| README, NOTICE, third-party notices (and the licence) | `/usr/share/doc/parterre` (`/usr/share/licenses/parterre`) |

Besides git (`git-core` in the `.rpm`, which is git without Perl and the GUIs), both depend on
the display libraries parterre loads at run time and neither tool can see: EGL and GLX, Wayland
client and EGL, X11, X11-xcb, Xcursor, Xi, Xrender, xkbcommon and xkbcommon-x11. That list came
from running parterre with `LD_DEBUG=libs` on Wayland and on X11. The `.deb` names Debian's
packages; the `.rpm` asks for the libraries by soname, so it installs on Fedora and openSUSE
alike.

`packaging/linux/test-package.sh PACKAGE` installs a package with the system's package manager,
checks that `parterre --version` runs and the desktop files are in place, and removes it again.
The release workflow runs it on Debian 12, Ubuntu 22.04 and 24.04, Fedora and openSUSE Leap
15.6 (`.github/workflows/linux-packages.yml`), as root in their containers:

```sh
docker run --rm -v "$PWD:/src" -w /src debian:12 \
  sh packaging/linux/test-package.sh target/packages/parterre_0.5.1_amd64.deb
```

### Revision Graph in file managers

Right-clicking a folder, or the background of an open one, gives *Revision Graph*, as on
Windows. Menus belong to the file manager, not to Wayland or X11, and each has its own way in,
all in `packaging/linux/file-managers`:

- **Nautilus** (GNOME): `nautilus.py`, loaded by nautilus-python, which the packages recommend
  (`python3-nautilus`, `nautilus-python` on Fedora). It starts parterre through its desktop
  entry. It handles both the extension API of Nautilus 43 and later and the older one of
  Nautilus 42 (Ubuntu 22.04).
- **Dolphin** (KDE): `dolphin.desktop`, a service menu.
- **Nemo** (Cinnamon): two actions, one for a folder and one for the background.

All three offer it for one folder on this machine at a time. Clicked through, from the
packages, under Xvfb in Nautilus 42 (Ubuntu 22.04) and 46 (Ubuntu 24.04), Dolphin 23.08 (KDE
Frameworks 5, Ubuntu 24.04) and 26.08 (Frameworks 6, Fedora 44) and Nemo 6.0: the item is there
on a folder and on the background, not on files, and starts parterre with the folder's path. Xfce (Thunar), MATE (Caja) and LXQt
(PCManFM-Qt) get nothing: Thunar's custom actions live only in each user's own settings.

The desktop entry deliberately has no `MimeType=inode/directory`, which would have listed
parterre under *Open With* for folders. Where the desktop names no default folder app, as on
Xfce, MATE, LXQt and plain window managers, GIO then picks the first app that can open folders,
alphabetically, and `se.trustfall.parterre` comes before `thunar`: tried on Ubuntu 24.04 with
Thunar, `xdg-open` and `exo-open` then opened folders in parterre. `NoDisplay`, `OnlyShowIn` and
`NotShowIn` don't keep GIO from choosing it. A test in `crates/parterre/src/settings.rs`
keeps it out.

## macOS

```sh
xcode-select --install    # once: the C compiler and git
cargo build --release
./target/release/parterre ~/some/repo
```

## macOS app

The release's disk images hold `parterre.app`, to drag into Applications. Setup, once:

```sh
cargo install --locked cargo-about --features cli
```

Then:

```sh
cargo build --release
packaging/macos/build-dmg.sh    # → target/packages/parterre-<version>-<target>.dmg
```

The release builds it for each target from a build stamped with its channel (see
[Features](#features)), and passes the version, since the Intel binary built on Apple silicon
can't be run to tell it:

```sh
PARTERRE_CHANNEL=dmg cargo build --release --target aarch64-apple-darwin
packaging/macos/build-dmg.sh --target aarch64-apple-darwin --version 0.5.1 dist
```

| File in `parterre.app/Contents` | What |
|---|---|
| `MacOS/parterre` | the binary |
| `Info.plist` | from `packaging/macos/Info.plist`: bundle ID `se.trustfall.parterre`, the version, and the oldest macOS the binary loads on (10.12 on Intel, 11.0 on Apple silicon) |
| `Resources/parterre.icns` | the icon, in Finder, the Dock and the app switcher |
| `Resources/LICENSE`, `NOTICE`, `THIRD-PARTY-NOTICES.html` | the licences, which go wherever the app goes |

The bundle version is the version without its pre-release part, as the MSI's is: bundle
versions are numbers only, so `0.5.0-rc1` and `0.5.0` are both `0.5.0`.

The app is signed ad hoc (`codesign --sign -`), not with a Developer ID, and not notarized
(why: [distribution.md](distribution.md#macos)). Apple silicon runs nothing unsigned, and the
signature seals `Info.plist` and the resources with the binary. A copy downloaded with a
browser is quarantined, so the first start says Apple could not verify it; *System Settings ›
Privacy & Security › Open Anyway* opens it, once. A copy from `curl` or built locally isn't
quarantined and opens at once.

From a terminal, run `/Applications/parterre.app/Contents/MacOS/parterre`, or link it onto the
`PATH`. Started from the Dock or Finder, git and `gh` still get the login shell's `PATH`, as
from a terminal (#326).

`packaging/macos/test-dmg.sh DMG REPO` installs the app from a disk image into
`/Applications`, checks its signature and `parterre --version`, starts it through Launch
Services as Finder does, with a screenshot of `REPO`, and removes it again. The release
workflow runs it on Apple silicon and Intel runners before publishing.

## Checking visuals without a human

```sh
cargo run --release -- ~/repo --screenshot out.png --window-size 1400x900 [--fit] [--theme dark]
```

This saves the window to `out.png` once the graph is in, and exits. `--script` drives the
window first: open any window, dialog or menu, click and type, and take more screenshots.
`--record` makes a GIF or a video. `scripts/screenshots.sh` takes every window, dialog and menu
at once. All of it is described in [automation](automation.md).

## Icon

`crates/parterre-core/src/icon.rs` draws the app icon in code. The window icon is rasterised
from it at startup, and

```sh
cargo run --release -p parterre-core --example icon_assets
```

writes `packaging/icon/`: the SVG, PNGs from 16 to 512 px, `parterre.ico` and `parterre.icns`.
Rerun it after changing the drawing and commit the results.

`crates/parterre/build.rs` embeds `parterre.ico` in the Windows executable through a resource
script it writes (`crates/parterre/src/win_resource.rs`), together with the version information
Explorer shows under *Properties → Details*: product name, file and product version, the
copyright line of `NOTICE` and Trustfall AB as the company. That needs `rc.exe` from the Windows SDK (installed with the
build tools above), or `x86_64-w64-mingw32-windres` for the GNU target; without one the build
only warns and the `.exe` has no icon or details. The `.icns` is the icon of the
[macOS app](#macos-app).

On Linux, `packaging/linux/install.sh` installs the release binary into `~/.local/bin`, and the
desktop entry, the icon and the file managers' *Revision Graph* where the desktop finds them;
`--uninstall` removes them again. A desktop shows a window's icon through the desktop entry of
the window's app ID, `se.trustfall.parterre`, which it loads only if the entry's `Exec` can be
found, so the script writes the binary's absolute path into the entry rather than relying on
`~/.local/bin` being on the session's PATH. On Wayland the entry is the only source of the icon,
since GNOME never uses the icon a window sets on itself. A running parterre shows it after a
restart. The entry and icon were called `parterre` up to 0.5; the script removes those.
