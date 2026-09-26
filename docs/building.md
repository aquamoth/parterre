# Building

## Linux (development machine)

```sh
cargo build --release
./target/release/parterre ~/some/repo
```

Only a Rust toolchain is required. At runtime the window needs the usual desktop libraries:
Wayland or X11, libxkbcommon, and OpenGL (EGL/GLX), all of which are present on any desktop.

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
(`crates/parterre/src/console.rs`, the workspace's only `unsafe`). It releases the console
before opening an interactive window, so closing the terminal doesn't close the window.
Shells don't wait for GUI programs, so the output may appear after the next prompt; press
Enter to get a fresh prompt. Debug builds are ordinary console programs.

Don't use the `x86_64-pc-windows-gnu` host toolchain as a shortcut around installing the MSVC
tools. Its bundled `dlltool` needs an assembler (`as.exe`) that the toolchain doesn't ship.
`windows-link` always uses `raw-dylib`, so the build fails with
`error calling dlltool` / `CreateProcess` unless a full MinGW-w64 is on `PATH`.

Cross-checking from Linux works without extra tools once the `github` feature is left out:
its TLS library, ring, compiles C for the target, which needs MinGW-w64's headers
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
`target\msi\stage`. With `-Stage DIR` it takes them from `DIR` instead, as CI and the release
workflow do; `-Out FILE` names the MSI. It passes `-acceptEula wix7` on every run, which
accepts WiX's Open Source Maintenance Fee EULA for that run only; don't run `wix eula accept`,
which leaves an acceptance file behind. The MSI version is the `Cargo.toml` version without its
pre-release part, so `0.5.0-rc.1` and `0.5.0` both build MSI version `0.5.0`, and the one
installed later replaces the other.

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

## macOS

Should build with `cargo build --release`. Not yet tried.

## Checking visuals without a human

```sh
cargo run --release -- ~/repo --screenshot out.png --window-size 1400x900 [--fit] [--theme dark]
```

This renders a few frames, saves the window to `out.png`, and exits. `--demo-drag DX,DY` drags
the centre node first, to show the physics. `--demo-menu node` (or `canvas`) right-clicks the
centre node (or `--demo-node NAME`, or empty canvas) and hovers the second item, to show the
context menu. `--demo-open menu` (or `filter`, `zoom`, `drag`, `settings`, `settings:advanced`
and the other pages) opens the ☰ menu, a toolbar popover or the settings. `--demo-log REF` (or
`FIRST..SECOND`) opens the log window, and `--log-layout a` (to `d`, or `stacked`,
`side-by-side`, `details-below`, `files-right`) picks its layout. Screenshot runs ignore the
saved settings and don't save any. Without `--theme` the theme follows the desktop.

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
only warns and the `.exe` has no icon or details. The `.icns` waits for a macOS `.app` bundle.

On Linux, `packaging/linux/install.sh` installs the release binary into `~/.local/bin`, and the
desktop entry and the icon where the desktop finds them; `--uninstall` removes them again. A
desktop shows a window's icon through the desktop entry, which it loads only if the entry's
`Exec` can be found, so the script writes the binary's absolute path into the entry rather than
relying on `~/.local/bin` being on the session's PATH. On Wayland the entry is the only source
of the icon, since GNOME never uses the icon a window sets on itself. A running parterre shows
it after a restart.
