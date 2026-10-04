# How "Update now" installs a newer parterre, per channel

Research note for [#252](https://github.com/aquamoth/parterre/issues/252), part of the map
[#175](https://github.com/aquamoth/parterre/issues/175): *how does "Update now" install a newer
parterre on each channel, the way comparable desktop apps do it?* The menu entry itself was
decided in #226; Snap and Flathub update parterre themselves and are out of scope here.

All pages were read on 2026-10-04. Sources are vendor docs, package-manager source code at pinned
commits, and maintainers' statements in trackers. A statement that is my own conclusion is marked
**(derived)**. A statement I could not check against a primary source is marked
**(UNVERIFIED)**. A statement checked by running a command is marked **(tested 2026-10-04:
`command`)**.

## TL;DR

- **Current practice splits in two.** Apps that own their installer replace themselves in place:
  Zed, VS Code's installer builds, GitHub Desktop, Obsidian, TortoiseGit, every Tauri app. Where a
  package manager or distro owns the files, the same apps only notify, or point at the package
  manager: VS Code's zip and tarball, Zed's distro builds, WezTerm, Rerun. Alacritty doesn't even
  notify ("It's called a package manager"). Apart from winget and Chocolatey installs, which
  apps update with their own installer anyway, no app found replaces files a package manager owns
  (§2).
- **MSI (direct, winget, Chocolatey): run the newer MSI.** Download it from the GitHub Release,
  check its SHA-256 and start `msiexec /i <file> /passive`, adding `ALLUSERS=1` when parterre is
  installed machine-wide. Then quit, and have a small waiter restart parterre when msiexec exits
  (§3).
  - **Per-user installs need no elevation.** A machine-wide install brings a UAC prompt, in the
    yellow of unsigned programs while the MSI is unsigned. A standard user can't pass it without
    an administrator's password.
  - **Prior art:** Tauri's updater runs MSIs the same way. TortoiseGit downloads its MSI, checks a
    signature and runs it.
- **winget follows an in-app MSI upgrade.** It reads the installed version from the Apps &
  Features entry, which Windows Installer rewrites. The entry is tied to the package by the
  `UpgradeCode`, which parterre never changes. `winget list` then shows the new version and
  `winget upgrade` offers nothing older (§3.2).
  - **The exception is winget's *portable* (zip) install** in the pending manifest. If parterre
    replaced that `parterre.exe`, winget would keep the old version, and its later upgrade or
    uninstall would stop with "modified; use --force". So parterre must not touch it.
  - **`RequireExplicitUpgrade` is not needed.** It is winget's flag for self-updating packages.
    VS Code, Zed, GitHub Desktop and Obsidian all update themselves, and none sets it.
- **Chocolatey never learns of it.** Open-source Chocolatey keeps showing the version of the
  package it installed. Even licensed autosync keeps "the package version … the same". The next
  `choco upgrade` re-runs the MSI over an equal or older version. That is harmless with today's
  `AllowSameVersionUpgrades="yes"`; the one case that fails is sketched in §3.3. Chocolatey's own
  advice for self-updating software is that the *user* pins the package (§3.3).
- **Zip and tarballs: replace the binary in place.** Download the archive, check its SHA-256 and
  extract it next to the binary. Then use `self-replace` 1.5.0: on Unix an atomic rename, on
  Windows the running exe is renamed aside. Restart from the same path.
  - When the folder isn't writable, open the release page instead.
  - On macOS, write a new file and rename it into place, never modify it in place: Apple
    documents a code-signing crash otherwise (§4).
- **.deb / .rpm from GitHub: install with the system's package manager.** One way is
  `pkexec apt-get install -y /path/x.deb` (or `dnf install -y`, or `zypper install
  --allow-unsigned-rpm`). The other is PackageKit's `InstallFiles` over D-Bus. Either resolves
  dependencies and keeps dpkg's and rpm's records right, behind one admin password prompt.
  - **Prior art:** Tauri runs `pkexec dpkg -i` / `rpm -U`, which skip dependency resolution.
  - **What VS Code and Chrome do instead:** an apt or dnf repository, which
    `docs/distribution.md` defers (§5).
- **cargo install: show the command.** Offer `cargo install --locked parterre` (or
  `cargo binstall parterre`), as uv tells pip and brew users to use their tool. Replacing the
  binary would leave cargo's install record at the old version (§6).
- **Frameworks don't fit parterre's packaging.**
  - **Velopack 1.2.161** would *replace* the WiX MSI: it brings its own `Setup.exe`, an
    optional MSI with its own layout, and `Update.exe`. On Linux it supports only AppImage, so
    the `.deb` and `.rpm` stay as they are and get no updates from it.
  - **axoupdater 0.10.2** replaces nothing, but updates only installs made by `dist`'s shell
    and PowerShell installer scripts, a channel parterre doesn't have.
  - **`self_update` 1.3.0** fits the zip and tarballs only. Its README says to hand `.deb` and
    `.msi` files to `dpkg -i` and `msiexec /i` yourself (§2.2).
- **Verification:** each GitHub release asset already has a SHA-256 `digest` in the same
  releases-API response the update check reads (tested 2026-10-04). Checking it equals checking
  `SHA256SUMS`: integrity, not authenticity.
  - **Who checks more:** Tauri (mandatory minisign), TortoiseGit (OpenPGP) and Velopack (SHA-256
    from its own feed, plus Authenticode when signed). Zed relies on HTTPS alone (§7.1).
  - **Cheapest step up:** turn on GitHub's *immutable releases* (no code), which freezes assets
    and tags after publishing. After that, minisign signatures (`minisign-verify` 0.3.0, no
    dependencies) would also serve `cargo binstall`.
- **Mark of the Web:** a file downloaded by parterre's own HTTP client gets no zone mark. So
  there is no SmartScreen "Windows protected your PC" prompt, as with winget and Chocolatey today
  **(derived)**.
  - **Smart App Control** checks every executable, downloaded or not, and blocks unsigned ones.
    An in-app update neither helps nor hurts there (§7.2).

## 1. Per channel at a glance

| Channel | What comparable apps do | Recommended "Update now" | Elevation | Package manager's view afterwards | Cost to parterre |
|---|---|---|---|---|---|
| **MSI, per-user** (direct or winget's default) | Run the new installer silently, restart (Zed, VS Code user setup, Tauri MSI, TortoiseGit) | Download MSI, SHA-256, `msiexec /i … /passive`, quit, waiter restarts | None | winget: new version, from the `DisplayVersion` in Apps & Features | ~1 module; `sha2`; no packaging change |
| **MSI, machine-wide** (`ALLUSERS=1`; Chocolatey, or by hand) | Same, with an elevation prompt (VS Code system setup: "In-product updates also require elevation") | Same, plus `ALLUSERS=1` | UAC prompt (yellow while unsigned); credentials for standard users | Chocolatey: still the old package version (FOSS and licensed); winget: new version | Same |
| **winget portable** (zip in the pending manifest) | winget replaces the file and rewrites its own entry | Don't self-update; show `winget upgrade Trustfall.Parterre` | – | Self-replacing would break winget's later upgrade/uninstall | Path check only |
| **Windows zip** | VS Code: opens the download page; self_update-based CLIs: replace in place | Download zip, SHA-256, `self-replace`, restart; read-only folder → release page | None (if writable) | – | `self-replace`, `zip` |
| **Linux / macOS tarball** | Zed: rsync into `~/.local/zed.app`; VS Code: download page | Download tar.gz, SHA-256, `self-replace` (atomic rename), restart; read-only → release page | None (if writable) | – | `self-replace`, `tar` (`flate2` already in the tree) |
| **.deb / .rpm (GitHub)** | VS Code, Chrome: own apt/dnf repo; Tauri: `pkexec dpkg -i`/`rpm -U`; WezTerm, Rerun: notify | Download, SHA-256, `pkexec` apt-get/dnf/zypper install, re-exec `/usr/bin/parterre`; or PackageKit `InstallFiles` | Admin password (polkit) | dpkg/rpm database: new version | Distro-specific commands, or `zbus` for PackageKit |
| **cargo install** | uv: refuses, names the right tool; Alacritty: "use a package manager" | Show `cargo install --locked parterre` / `cargo binstall parterre` | – | cargo: correct, since cargo does the install | Text only |

## 2. Prior art

### 2.1 Desktop apps

- **Zed** (Rust, own GPU UI). Commit
  [a846890](https://github.com/zed-industries/zed/tree/a84689073d296dfd39987bc7dd478e43ef76d83a).
  - **Polling:** "By default, Zed checks for updates and installs them automatically the next
    time you restart the app"
    ([docs/src/update.md](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/docs/src/update.md)).
  - **Windows:** the Inno Setup installer is per-user (`PrivilegesRequired=lowest`,
    `{autopf}`) ([zed.iss](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/zed/resources/windows/zed.iss#L32-L39)).
    The update runs the downloaded installer with `/verysilent /update=true`. A separate
    `auto_update_helper` then swaps the files once Zed has quit and starts the new binary. It
    moves the old files to `old\` and rolls back on failure
    ([auto_update.rs L1313](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update/src/auto_update.rs#L1313),
    [updater.rs](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update_helper/src/updater.rs)).
  - **Linux:** extracts the tarball and `rsync -av --delete`s it over the running
    `~/.local/zed.app` ([L1129-L1195](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update/src/auto_update.rs#L1129-L1195)).
  - **macOS:** mounts the dmg and rsyncs the app bundle.
  - **Verification:** none beyond HTTPS. The download function writes the response straight to
    disk, and the file has no hash or signature check (`grep -i 'hash\|verify\|signature'` finds
    none; [L1076-L1127](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update/src/auto_update.rs#L1076-L1127)).
  - **Package managers:** a build-time or run-time `ZED_UPDATE_EXPLANATION` turns polling off. A
    manual check then shows "Zed was installed via a package manager." with that text
    ([L280-L320](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update/src/auto_update.rs#L280-L320)).
    The packaging docs tell distros to set it, e.g. "Please use flatpak to update zed."
    ([development/linux.md](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/docs/src/development/linux.md)).
- **VS Code** (Electron). Commit
  [24a4117](https://github.com/microsoft/vscode/tree/24a41178148f72f49e4ac0756ddb3b5347429a91).
  - **Windows installers:** user setup "does not require administrator permissions … provides
    the smoothest update experience". System setup "requires administrator permissions … In-product
    updates also require elevation" ([docs](https://code.visualstudio.com/docs/setup/windows)).
  - **Windows update steps** ([updateService.win32.ts](https://github.com/microsoft/vscode/blob/24a41178148f72f49e4ac0756ddb3b5347429a91/src/vs/platform/update/electron-main/updateService.win32.ts)):
    - downloads the setup and checks it against the `sha256hash` from its update server (L313);
    - runs it with `/verysilent` and `__COMPAT_LAYER=RunAsInvoker` (L433-L453);
    - turns updates off when a user setup runs as Administrator (L139-L142).
  - **Zip ("Archive"):** "update it manually". The code only opens the download URL (L211,
    L261, L370).
  - **Linux:** the `.deb` "prompts to install the apt repository and signing key, which enables
    auto-update through the system package manager". The `.rpm` docs install a yum repository
    ([docs](https://code.visualstudio.com/docs/setup/linux)). In-app, Linux only opens the
    website: "we don't currently detect the package type that was installed"
    ([updateService.linux.ts L77-L87](https://github.com/microsoft/vscode/blob/24a41178148f72f49e4ac0756ddb3b5347429a91/src/vs/platform/update/electron-main/updateService.linux.ts#L77-L87)).
- **GitHub Desktop** (Electron, Squirrel.Windows). Commit
  [3754e26](https://github.com/desktop/desktop/tree/3754e26d1f021ccb2de77f86061ea1dab2c95d39).
  - **Windows:** installs per user ([docs](https://docs.github.com/en/desktop/installing-and-authenticating-to-github-desktop/installing-github-desktop)).
    Its packaging builds both a Squirrel `setupExe` and a `setupMsi`
    ([script/package.ts L105-L106](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/script/package.ts#L105-L106)).
  - **The MSI is not an installer of the app.** "This MSI isn't a general-purpose installer …
    once you run the MSI, users from now on will get the app installed, on next Login"
    ([Squirrel docs](https://github.com/Squirrel/Squirrel.Windows/blob/51f5e2cb01add79280a53d51e8d0cfa20f8c9f9f/docs/using/machine-wide-installs.md)).
    Every copy lives in `%LocalAppData%`
    ([install-process.md](https://github.com/Squirrel/Squirrel.Windows/blob/51f5e2cb01add79280a53d51e8d0cfa20f8c9f9f/docs/using/install-process.md)),
    so updates never need elevation.
  - **Linux:** "not yet supported".
- **Obsidian** (Electron, closed source). The docs separate app updates ("If automatic updates are
  enabled, the application will update on restart") from "periodic installer updates, which
  require downloading and running the installer" ([help](https://obsidian.md/help/updates)). How
  that works per Linux package is **(UNVERIFIED)**.
- **WezTerm** (Rust). Commit
  [cab2516](https://github.com/wezterm/wezterm/tree/cab25161054c50fd6c705db4ceefef0f1e5a9575).
  It polls GitHub's `releases/latest` and shows a banner and a toast, "WezTerm Update Available
  … Click to see what's new", linking to the changelog. It installs nothing
  ([update.rs](https://github.com/wezterm/wezterm/blob/cab25161054c50fd6c705db4ceefef0f1e5a9575/wezterm-gui/src/update.rs#L93-L205)).
- **Alacritty** (Rust). No update check. A maintainer, on a request for one: "They're already out
  there. It's called a 'package manager'."
  ([#4778](https://github.com/alacritty/alacritty/issues/4778)).
- **Rerun** (Rust, egui, like parterre). Commit
  [107beb6](https://github.com/rerun-io/rerun/tree/107beb60f2a09c8145d554fe83e76900b0247ad4).
  It checks GitHub's `releases/latest` and logs "A newer version of Rerun is available … Download
  it at" the install page, or PyPI for the Python SDK. It installs nothing
  ([version_check.rs](https://github.com/rerun-io/rerun/blob/107beb60f2a09c8145d554fe83e76900b0247ad4/crates/top/re_viewer/src/version_check.rs)).
- **TortoiseGit** (parterre's behavioural reference; WiX MSI). Commit
  [7338078](https://github.com/TortoiseGit/TortoiseGit/tree/7338078f8ddd924b8cddee35f512f2286072136d).
  - **Steps:** *Check for updates* downloads the new MSI and a detached `.rsa.asc` OpenPGP
    signature into the Downloads folder, and checks it against a built-in key. *Install* then
    opens the MSI with `ShellExecute(…"open"…)`, so the MSI's own UI and elevation take over
    ([CheckForUpdatesDlg.cpp L477-L535, L538-L660](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/CheckForUpdatesDlg.cpp),
    [UpdateCrypto.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/UpdateCrypto.cpp)).
  - **winget:** its manifest is machine-scope WiX ([manifest](https://github.com/microsoft/winget-pkgs/blob/55dcc0718d6447883b3a9864614f0caf05b9e168/manifests/t/TortoiseGit/TortoiseGit/2.19.1.0/TortoiseGit.TortoiseGit.installer.yaml)).
- **Tauri apps** (Rust; `tauri-plugin-updater` 2.13.1, commit
  [d4835d0](https://github.com/tauri-apps/plugins-workspace/tree/d4835d0e947179bac24a383212792d74be3ebe4f)).
  This is the closest prior art for an MSI.
  - **Verification:** every download is checked against a minisign signature before installing
    ([updater.rs L837, L1634-L1658](https://github.com/tauri-apps/plugins-workspace/blob/d4835d0e947179bac24a383212792d74be3ebe4f/plugins/updater/src/updater.rs#L1634-L1658)).
  - **MSI:** `ShellExecuteW("open", "%SYSTEMROOT%\System32\msiexec.exe", "/i <msi> /passive
    /promptrestart AUTOLAUNCHAPP=True LAUNCHAPPARGS=…")`, then `std::process::exit(0)`
    ([L938-L1040](https://github.com/tauri-apps/plugins-workspace/blob/d4835d0e947179bac24a383212792d74be3ebe4f/plugins/updater/src/updater.rs#L938-L1040)).
    - Install modes: `/passive` (default), `/qb+` or `/quiet`. Quiet "Requires admin privileges
      if the installer does"
      ([config.rs](https://github.com/tauri-apps/plugins-workspace/blob/d4835d0e947179bac24a383212792d74be3ebe4f/plugins/updater/src/config.rs#L15-L38)).
    - The restart is a custom action in Tauri's WiX template:
      `<Custom Action="LaunchApplication" After="InstallFinalize">AUTOLAUNCHAPP AND NOT Installed</Custom>`
      ([main.wxs](https://github.com/tauri-apps/tauri/blob/61285f6fa5a531788e6912ea598a572bfcd7bf77/crates/tauri-bundler/src/bundle/windows/msi/main.wxs#L35-L38)).
  - **.deb / .rpm:** `pkexec dpkg -i` or `pkexec rpm -U`. If that fails, a password from zenity or
    kdialog is fed to `sudo`; failing that, `sudo` in the terminal
    ([L1223-L1340](https://github.com/tauri-apps/plugins-workspace/blob/d4835d0e947179bac24a383212792d74be3ebe4f/plugins/updater/src/updater.rs#L1223-L1340)).
  - **AppImage:** replaced by rename.
- **uv** (Rust CLI, axoupdater). Commit
  [46b84fd](https://github.com/astral-sh/uv/tree/46b84fd0bfec23b72f29e8e2185ba68a65052f48).
  Without an install receipt, `uv self update` refuses: "Self-update is only available for uv
  binaries installed via the standalone installation scripts. If you installed uv with pip, brew,
  or another package manager, update uv with `pip install --upgrade`, `brew upgrade`, or similar."
  ([self_update.rs L93-L112](https://github.com/astral-sh/uv/blob/46b84fd0bfec23b72f29e8e2185ba68a65052f48/crates/uv/src/commands/self_update.rs#L93-L112)).

### 2.2 Rust crates and frameworks

Versions from crates.io on 2026-10-04.

- **`self-replace` 1.5.0** (mitsuhiko, 2024-09). It replaces or deletes the running executable.
  "On UNIX systems … a new file is placed right next to the current executable and an atomic move
  with `rename` is performed." On Windows the running exe "can be renamed, but it cannot be
  unlinked". So the crate moves it aside, puts the new one in its place, and has a spawned copy
  delete the old one after exit
  ([lib.rs docs](https://github.com/mitsuhiko/self-replace/blob/d1356fdb346e191b90eec3a21b310c19ac24d2d9/src/lib.rs)).
  No dependencies beyond the OS.
- **`self_update` 1.3.0** (jaemk, 2026-09). Commit
  [fc84045](https://github.com/jaemk/self_update/tree/fc840459d2ba11dec2b04a8db053e919730b1d13).
  - **What it does:** finds the release on GitHub, GitLab, Gitea or S3; downloads and extracts
    the archive (features `archive-tar`/`archive-zip`, `compression-flate2`); replaces the
    binary through `self-replace`.
  - **HTTP client:** `reqwest` by default; `ureq` is a supported drop-in with
    `default-features = false`.
  - **Checksums:** the `checksums` feature (in 1.0.0) checks GitHub's per-asset `sha256:`
    digest automatically, and since 1.1.0 a `SHA256SUMS` asset via
    `checksum_from_asset("SHA256SUMS")`.
  - **Signatures:** `signatures` (zipsign) checks them
    ([README](https://github.com/jaemk/self_update/blob/fc840459d2ba11dec2b04a8db053e919730b1d13/README.md),
    [CHANGELOG](https://github.com/jaemk/self_update/blob/fc840459d2ba11dec2b04a8db053e919730b1d13/CHANGELOG.md)).
  - **Its own limit:** "`.deb` / `.msi` packages are a different shape entirely -- hand the
    downloaded file to `dpkg -i` / `msiexec /i` yourself".
- **axoupdater 0.10.2 / dist 0.33.0** (axo.dev). The library "uses the install receipts
  produced by cargo-dist", JSON files in `~/.config/APP` or `%LOCALAPPDATA%\APP`
  ([README](https://github.com/axodotdev/axoupdater/blob/73ba5a7eddf541b96e3db1c0b0243119a6115e15/README.md)).
  - **How it updates:** it downloads and runs `{app}-installer.sh` or `{app}-installer.ps1` from
    the new release
    ([lib.rs L454-L458](https://github.com/axodotdev/axoupdater/blob/73ba5a7eddf541b96e3db1c0b0243119a6115e15/axoupdater/src/lib.rs#L454-L458)).
  - **Scope:** dist's own docs scope it to "users of the shell and PowerShell installers". Users
    of other package managers "can use that package manager"
    ([updater.md](https://github.com/axodotdev/cargo-dist/blob/6d7c35a089d0fd2df2d1acf75b1c0ae2ba2abf66/book/src/installers/updater.md)).
  - **dist's MSI:** built with WiX v3, "WiX v4 isn't yet supported"
    ([msi.md](https://github.com/axodotdev/cargo-dist/blob/6d7c35a089d0fd2df2d1acf75b1c0ae2ba2abf66/book/src/installers/msi.md)).
  - **For parterre:** adopting axoupdater replaces none of parterre's packaging. It would add a
    shell/PowerShell installer channel, and only installs made through that channel would ever
    update. MSI, zip, tarball, `.deb`, `.rpm` and cargo installs have no receipt **(derived)**.
- **Velopack 1.2.161** (2026-09-29; Rust crate `velopack`, packaging tool `vpk` on .NET 8). Commit
  [92d6a1c](https://github.com/velopack/velopack/tree/92d6a1c91716729d449034df5c50307dcce39493),
  docs commit [1ca8eea](https://github.com/velopack/velopack.docs/tree/1ca8eea6017fb9c5743e070575a9f28da08c26c1).
  - **Windows layout:** `vpk pack` builds a one-click `Setup.exe` into `%LocalAppData%\{packId}`.
    The folder holds an execution stub, `current\` and `Update.exe`.
  - **Its MSI:** `--msi` builds one with Velopack's own WiX v7 fork, scoped `PerUser`,
    `PerMachine` or `Either`. "After installation, updates work identically via `Update.exe`"
    ([installer.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/packaging/installer.mdx),
    [windows.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/packaging/operating-systems/windows.mdx)).
    - Updating a Program Files install re-launches the updater as administrator
      ([apply_windows_impl.rs L63-L104](https://github.com/velopack/velopack/blob/92d6a1c91716729d449034df5c50307dcce39493/src/bins/src/commands/apply_windows_impl.rs#L63-L104)).
    - After an update it writes the new `DisplayVersion` straight into the MSI's uninstall
      registry key
      ([registry.rs L69-L106](https://github.com/velopack/velopack/blob/92d6a1c91716729d449034df5c50307dcce39493/src/bins/src/windows/registry.rs#L69-L106)).
      Windows Installer's own record keeps the old version.
  - **Linux:** "Velopack does not create an installer, it simply creates an `.AppImage` file"
    ([linux.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/packaging/operating-systems/linux.mdx)).
  - **Verification:** packages are checked against SHA-1/SHA-256 from its release feed
    ([manager.rs L537-L555](https://github.com/velopack/velopack/blob/92d6a1c91716729d449034df5c50307dcce39493/src/lib-rust/src/manager.rs#L537-L555)).
    Code signing is "very recommended (but not required)".
  - **Integration:** custom install steps go through `--veloapp-install` and other hook
    arguments that must exit within 15–30 s
    ([hooks.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/integrating/hooks.mdx)).
    `VelopackApp::build().run()` must be the first thing in `main`
    ([rust.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/getting-started/rust.mdx)).
    The Rust crate uses `ureq`, as parterre does.
- **`tauri-plugin-updater` 2.13.1 / `cargo-packager-updater` 0.2.3** (2025-07-21). The Tauri one
  needs a Tauri app. The standalone one is "Updater for apps that was packaged by
  `cargo-packager`", and its endpoints must answer in its own JSON format with a minisign
  signature
  ([README](https://github.com/crabnebula-dev/cargo-packager/blob/57488b02bc301609d24b9f5500d86745463d0de3/crates/updater/README.md)).
  Useful as code to read, not to adopt **(derived)**.
- **`minisign-verify` 0.3.0** (2026-09-25): verify-only, used by Tauri. `cargo binstall`'s
  signature support is minisign too: `[package.metadata.binstall.signing]`
  ([SIGNING.md](https://github.com/cargo-bins/cargo-binstall/blob/7bebc2e59eb8820162b7ac8f62a92bfdb2732447/SIGNING.md)).

### 2.3 What is current practice (derived)

1. **The installer that installed the app installs the update.** VS Code re-runs its Inno setup
   and Zed its Inno installer. Tauri re-runs the MSI or NSIS installer, TortoiseGit its MSI, and
   GitHub Desktop and Velopack their own updater. None of them patches files under an installer
   that didn't put them there.
2. **Where a package manager owns the files, the app defers.** VS Code adds its repository; Zed
   disables itself via `ZED_UPDATE_EXPLANATION`; uv names the right tool. On Windows, winget and
   Chocolatey are the exception: apps run their own installer anyway (VS Code, Zed, GitHub
   Desktop and Obsidian are all in winget without `RequireExplicitUpgrade`, §3.2).
3. **Archives are either replaced in place** (Zed's tarball, self_update-based CLIs) **or sent
   to the download page** (VS Code, WezTerm, Rerun).
4. **Restart is the norm** where the app installs (Zed, VS Code, Tauri, Velopack).
5. **Verification ranges from HTTPS only (Zed) to mandatory signatures (Tauri).** VS Code,
   Velopack and self_update check a SHA-256 from the same server; TortoiseGit and Tauri check a
   signature against a built-in key.

## 3. MSI, including winget and Chocolatey

### 3.1 Mechanics

- **Which file:** the release's `parterre-<v>-x86_64-pc-windows-msvc.msi`. It is the same file
  winget's `InstallerUrl` and Chocolatey's `url64bit` point at
  (`packaging/chocolatey/tools/chocolateyinstall.ps1`; winget PR
  [#442178](https://github.com/microsoft/winget-pkgs/pull/442178)). So "parterre can't tell them
  apart" doesn't matter for installing.
- **Which scope:** the MSI writes its component key paths under
  `HKMU\Software\Trustfall AB\parterre`: HKCU per-user, HKLM machine-wide
  (`packaging/windows/parterre.wxs`). parterre can read which hive has them, or compare its
  own path with `%LOCALAPPDATA%\Programs` **(derived)**.
  - **The scope must be repeated.** "Once Windows Installer 5.0 installs an application, it uses
    the same installation context for all subsequent updates"
    ([Single Package Authoring](https://learn.microsoft.com/en-us/windows/win32/msi/single-package-authoring)).
    `docs/building.md` records that a per-user install followed by a machine-wide one leaves two
    entries. So a machine-wide install must be updated with `ALLUSERS=1`, as Chocolatey passes it.
- **Command:** `msiexec /i <file> /passive /norestart`, plus `ALLUSERS=1` when machine-wide, as
  Tauri does with `/passive`.
  - **Per-user:** "does not display UAC prompts for credentials".
  - **Per-machine:** Windows Installer "prompts for UAC credentials to confirm that the user has
    sufficient privileges to install software for all users"
    ([Single Package Authoring](https://learn.microsoft.com/en-us/windows/win32/msi/single-package-authoring)).
    A standard user sees the credential prompt and needs an administrator
    ([Using Windows Installer with UAC](https://learn.microsoft.com/en-us/windows/win32/msi/using-windows-installer-with-uac)).
  - **What the prompt looks like:** "Yellow background: the application is unsigned or signed
    but isn't trusted"
    ([How UAC works](https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/how-it-works)).
    Which program name and publisher it shows for an unsigned MSI is **(UNVERIFIED)**.
  - **`/qn`:** whether it fails outright for a per-machine install started unelevated is
    **(UNVERIFIED)**. Tauri's docs only say quiet mode "Requires admin privileges if the
    installer does".
- **Downgrades and rc:** an older MSI is refused, and an equal version reinstalls
  (`AllowSameVersionUpgrades`). Since `0.5.0-rc.1` and `0.5.0` both build MSI version `0.5.0`, an
  rc user can update to the release (`docs/building.md`).
- **Mark of the Web:** none on a file parterre downloads itself (§7.2).
- **Restart:** two patterns.
  - **Tauri:** a WiX custom action after `InstallFinalize`, enabled by a property
    (`AUTOLAUNCHAPP`), starts the app; the app `exit(0)`s right after `ShellExecuteW`. Whether a
    custom action like that starts parterre elevated after a machine-wide update is
    **(UNVERIFIED)**.
  - **Zed and Velopack:** a helper outlives the app, waits, and launches the new binary.
  - **For parterre:** a copy of parterre in `%TEMP%`, started with the msiexec process handle,
    could wait for msiexec and read its exit code. It would restart parterre unelevated with
    the same arguments, or show what failed **(derived)**.
- **Failure:** "If however the installation is unsuccessful, the installer automatically
  performs a rollback installation that returns the system to its original state"
  ([Rollback](https://learn.microsoft.com/en-us/windows/win32/msi/rollback-installation)).
  - **Ordering:** parterre's `MajorUpgrade` keeps WiX's default schedule, which removes the old
    version inside the same transaction (`parterre.wxs` comment).
  - **Exit codes** ([error codes](https://learn.microsoft.com/en-us/windows/win32/msi/error-codes)):
    - 0, 3010 ("A restart is required … indicates success") and 1641 mean success. Chocolatey's
      script accepts the same three.
    - 1602 means the user cancelled, including at the UAC prompt **(UNVERIFIED)**.
    - 1603 is a fatal error, and 1618 means another installation is running.
  - **Files in use:** if a second parterre window is still open, Windows Installer meets
    `parterre.exe` in use. Whether it asks, or replaces the file at reboot (3010), under
    `/passive` is **(UNVERIFIED)**. Asking the other windows to close first avoids it
    **(derived)**. VS Code checks a mutex so as not to start its setup while another one runs
    ([L423-L428](https://github.com/microsoft/vscode/blob/24a41178148f72f49e4ac0756ddb3b5347429a91/src/vs/platform/update/electron-main/updateService.win32.ts#L423-L428)).

### 3.2 winget's view afterwards

winget-cli commit [3973956](https://github.com/microsoft/winget-cli/tree/39739564a4aaf1071b17d163ec332a08b9bcf05c),
release v1.29.380.

- **Which installer winget picks:** with no user preference, winget ranks MSIX, MSI, **WiX**,
  Burn, Nullsoft, Inno, EXE, and **Portable last**
  ([ManifestComparator.cpp L233-L246](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/src/AppInstallerCommonCore/Manifest/ManifestComparator.cpp#L233-L246)).
  The pending parterre manifest offers a zip/portable, a user WiX and a machine WiX
  ([PR #442178](https://github.com/microsoft/winget-pkgs/pull/442178)), so a plain
  `winget install` gets the MSI **(derived)**.
- **MSI installs:**
  - **Version:** winget reads the installed version from the Apps & Features key's
    `DisplayVersion` (`ARPHelper::DetermineVersion`,
    [ARPHelper.cpp L276-L331](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/src/AppInstallerRepositoryCore/Microsoft/ARPHelper.cpp#L276-L331)).
  - **Matching:** it pairs MSI entries with packages through `UpgradeCode`, read from
    `HKLM\…\Installer\UpgradeCodes` (same file, L62-L110). The manifest lists parterre's
    `UpgradeCode` `{B139A92E-…}` under `AppsAndFeaturesEntries`, which "are used to match
    installed packages with manifests"
    ([schema 1.12](https://github.com/microsoft/winget-pkgs/blob/55dcc0718d6447883b3a9864614f0caf05b9e168/doc/manifest/schema/1.12.0/installer.md)).
  - **Result:** an in-app `msiexec` upgrade rewrites `DisplayVersion` and keeps the
    `UpgradeCode`. `winget list` then shows the new version, and `winget upgrade` offers
    nothing until a newer manifest exists **(derived)**.
  - **When parterre is ahead of winget:** before its manifest is merged, winget compares a
    higher installed version with an older one and offers nothing, by the version rules in
    [spec #980](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/doc/specs/%23980%20-%20Apps%20and%20Features%20entries%20version%20mapping.md)
    **(derived)**.
- **Portable (zip) installs:**
  - **Where:** winget copies the exe to `%LOCALAPPDATA%/Microsoft/WinGet/Packages/<id>…` and
    links it from `…/WinGet/Links/`. It writes its *own* Apps & Features entry, and on upgrade
    "the entry in 'Apps & Features' will be updated accordingly"
    ([spec #182](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/doc/specs/%23182%20-%20Support%20for%20installation%20of%20portable%20standalone%20apps.md)).
    It also stores each file's SHA-256.
  - **If the file changes:** both upgrade and uninstall stop with "Unable to remove Portable
    package as it has been modified; to override this check use --force"
    ([PortableFlow.cpp L317-L327, L353-L367](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/src/AppInstallerCLICore/Workflows/PortableFlow.cpp#L303-L367),
    [PortableInstaller.cpp L67-L85](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/src/AppInstallerCLICore/PortableInstaller.cpp#L60-L90)).
  - **So:** parterre running from `…\WinGet\Packages\Trustfall.Parterre…` should show `winget
    upgrade Trustfall.Parterre` instead of replacing itself **(derived)**.
- **`RequireExplicitUpgrade`:** "This key identifies packages that upgrade themselves. By
  default, they are excluded from `winget upgrade --all`"
  ([schema 1.12](https://github.com/microsoft/winget-pkgs/blob/55dcc0718d6447883b3a9864614f0caf05b9e168/doc/manifest/schema/1.12.0/installer.md)).
  - **Who sets it:** none of the latest manifests of Microsoft.VisualStudioCode, ZedIndustries.Zed,
    GitHub.GitHubDesktop, Obsidian.Obsidian, wez.wezterm, TortoiseGit.TortoiseGit or Git.Git
    does. Most say `UpgradeBehavior: install` **(tested 2026-10-04: fetched each latest
    `*.installer.yaml` at winget-pkgs
    [55dcc07](https://github.com/microsoft/winget-pkgs/tree/55dcc0718d6447883b3a9864614f0caf05b9e168/manifests))**.
  - **Where the idea comes from:** winget's pinning spec gives "Packages may update themselves so
    that it will be duplicate effort for winget to try to update them" as one reason to pin
    ([spec #476](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/doc/specs/%23476%20-%20Package%20Pinning.md)).

### 3.3 Chocolatey's view afterwards

- **Open-source Chocolatey:** knows nothing of an upgrade made outside it: "open source
  Chocolatey will still have the package installed"
  ([Automatic Sync](https://docs.chocolatey.org/en-us/features/package-synchronization/automatic-sync/)).
  `choco list` keeps showing the package version it installed, from its `lib` folder
  **(derived)**.
- **Licensed autosync:** notes the upgrade, but "the package version will remain the same",
  because "There is not always a one to one line up between package version and software
  version". For self-updating software (Chrome is their example), "it is recommended typically
  that you pin the package to let the software automatically upgrade" (same page). The user does
  the pinning; no package-author guideline on built-in updaters was found in
  [Create Packages](https://docs.chocolatey.org/en-us/create/create-packages/).
- **What happens next (derived):**
  - **`choco upgrade parterre`** to a version parterre already installed: the MSI reinstalls
    the same version (`AllowSameVersionUpgrades="yes"`), which is harmless.
  - **`choco uninstall parterre`:** works whatever the version, since
    `chocolateyuninstall.ps1` finds the HKLM entry by name.
  - **The case that fails:** forcing Chocolatey to install an *older* package
    (`choco install parterre --version <old> --force`) runs an older MSI, which Windows Installer
    refuses. That gives exit 1603 with "A newer version of parterre is already installed."
- **Detecting Chocolatey (derived):** packages live under `$env:ChocolateyInstall\lib`, by default
  `C:\ProgramData\chocolatey\lib`
  ([setup](https://docs.chocolatey.org/en-us/choco/setup/)). parterre could check for
  `lib\parterre` if it ever wants to word the menu differently there. Nothing above requires it.

### 3.4 Would a framework replace the WiX MSI?

- **Velopack: yes.** Its installers (`Setup.exe`, or its own MSI) lay out `{root}\current\`
  plus an `Update.exe` and a stub, and only installs in that layout can be updated (§2.2).
  - **What has to move:** parterre's `parterre.wxs` would go, and with it the Explorer *Revision
    Graph* verbs, PATH entries, the dual-scope `HKMU` design and the ICE suppressions. Those
    would be rebuilt as `--veloapp-install` hook code, and the winget and Chocolatey manifests
    would wrap the new installer **(derived)**.
  - **Linux and macOS:** the `.deb` and `.rpm` stay, since Velopack only makes AppImages, and
    it would add an AppImage and a macOS `.pkg`.
  - **Build:** `vpk` needs the .NET 8 SDK.
- **axoupdater: no**, nothing is replaced, but it updates none of the existing channels (§2.2).
- **self_update: no**, nothing is replaced, but it covers the archives only.
- **Own code, as Tauri does for MSIs:** no packaging change. An optional `AUTOLAUNCHAPP`-style
  custom action in `parterre.wxs` is only needed if the restart should come from the MSI rather
  than from a waiter process.

### 3.5 Cost to parterre (derived)

- **Code:** about 150–250 lines on Windows: find the release asset and its digest in the update
  check's response, download, hash, start msiexec and wait, restart.
- **New crates:** `sha2` 0.11.0. `ureq` 3.4.2 and `windows-sys` are already in the tree; `ureq`
  sits behind `parterre-forge`'s `github` feature today.
- **Packaging:** none required. Optional: the relaunch custom action; signing (§7.1).

## 4. Windows zip, Linux and macOS tarballs

- **Steps:**
  1. Download `parterre-<v>-<target>.zip` or `.tar.gz` into the binary's own folder, so the
     final rename stays on one filesystem.
  2. Check its SHA-256.
  3. Extract `parterre` / `parterre.exe`.
  4. Call `self_replace::self_replace(new)`.
  5. Restart from `std::env::current_exe()` with the same arguments, then exit.
- **The other files in the archive** (README, LICENSE, NOTICE, third-party notices) are not
  locked and can be overwritten too **(derived)**.
- **Unix:** `self-replace` renames atomically: either the old file or the new one is at the path,
  never half of each.
- **Windows:** the running exe is renamed aside and deleted after exit (§2.2).
- **macOS:** Apple warns against writing over a signed binary. "macOS caches information about
  the code's signature in the kernel. It doesn't flush that cache when you modify the file's
  contents", which leads to "a hard-to-reproduce code-signing crash". The fix: "write the updated
  code to a temporary file and replace the existing file with that temporary one"
  ([Updating Mac software](https://developer.apple.com/documentation/security/updating-mac-software)).
  `self-replace`'s rename is exactly that **(derived)**.
  - Whether the in-app download gets a quarantine attribute, and so a Gatekeeper check, is
    **(UNVERIFIED)**; Gatekeeper checks software "When a user downloads and opens" it
    ([Gatekeeper](https://support.apple.com/guide/security/gatekeeper-and-runtime-protection-sec5599b66df/web)).
  - The release binaries' ad-hoc signature on Apple silicon is **(UNVERIFIED)**.
- **Where it can't write:** a binary in `/usr/local/bin` or `C:\Program Files`, placed there by
  hand, isn't writable without elevation.
  - **What others do:** Velopack prompts with `pkexec` for an AppImage "in a privileged folder"
    ([linux.mdx](https://github.com/velopack/velopack.docs/blob/1ca8eea6017fb9c5743e070575a9f28da08c26c1/docs/packaging/operating-systems/linux.mdx));
    VS Code's archive builds open the download page.
  - **For parterre:** opening the release page there fits VS Code, WezTerm and Rerun **(derived)**.
- **`packaging/linux/install.sh`** installs into `~/.local/bin` and writes the absolute path
  into the desktop entry. Replacing the binary in place keeps that path valid **(derived)**.
- **Failure:** download and extraction happen beside the old binary, which stays untouched until
  the final rename. Whether `self-replace` restores the original on Windows when the copy fails
  after the move aside is **(UNVERIFIED)**.
- **Alternative: `self_update` 1.3.0** with `default-features = false, features = ["ureq",
  "rustls", "github", "archive-tar", "archive-zip", "compression-flate2", "checksums"]`. It does
  all of the above. It also brings `regex`, `semver`, `serde_json`, `tempfile`, `zip` 8 and `tar`.
- **Cost (derived):** `self-replace` 1.5.0, `tar` 0.4.46 and `zip` 8.6.0, or `self_update`
  instead. `flate2` 1.1.10 is already in `Cargo.lock`. No packaging change.

## 5. .deb and .rpm from GitHub Releases

The files are root-owned in `/usr/bin` and `/usr/share`, and dpkg or rpm records them.

- **Options:**
  1. **`pkexec <package manager> install <file>`.** Commands:
     - Debian and Ubuntu: `apt-get install -y /abs/path.deb`;
     - Fedora: `dnf install -y /abs/path.rpm`;
     - openSUSE: `zypper --non-interactive install --allow-unsigned-rpm /abs/path.rpm`.

     These are the commands parterre's CI already uses to install its own packages on Debian
     12, Ubuntu 22.04/24.04, Fedora and openSUSE Leap 15.6
     (`packaging/linux/test-package.sh`, `.github/workflows/linux-packages.yml`). They resolve
     dependencies.
     - pkexec "allows an authorized user to execute PROGRAM as another user", root by default.
       It asks for admin authentication each time, and falls back to "its own textual
       authentication agent" when no agent runs
       ([pkexec(1)](https://manpages.debian.org/bookworm/pkexec/pkexec.1.en.html)).
     - On Debian 12 pkexec is its own package (same link). Whether it is installed by default
       on each desktop is **(UNVERIFIED)**.
     - Tauri does this with `dpkg -i` and `rpm -U`, which don't pull in new dependencies.
  2. **PackageKit `InstallFiles` over D-Bus.** "This method installs local package files onto the
     local system. The installer should always install extra dependant packages automatically"
     ([Transaction.xml](https://github.com/PackageKit/PackageKit/blob/d433465d4ad54f1964e152b7bfc44bb5042718f9/src/org.freedesktop.PackageKit.Transaction.xml#L705-L731)).
     - Its polkit action `package-install-untrusted` needs `auth_admin` every time ("This is not
       retained as each package should be authenticated")
       ([policy](https://github.com/PackageKit/PackageKit/blob/d433465d4ad54f1964e152b7bfc44bb5042718f9/data/policy/org.freedesktop.packagekit.policy.in#L49-L66)).
     - It works the same on apt, dnf and zypper systems, but needs `packagekitd` and a D-Bus
       client (`zbus`). Whether packagekitd is present by default per distro is
       **(UNVERIFIED)**.
  3. **Hand the file to the software centre** (`gio open` or `xdg-open` on the downloaded
     package). The user then clicks *Install*.
     - Ubuntu's App Center couldn't install local `.deb` files at first; Canonical said "We still
       plan to add back that feature"
       ([Discourse](https://discourse.ubuntu.com/t/supporting-gui-deb-package-installs-in-noble/43156)).
       It was reportedly added in a July 2024 App Center update **(UNVERIFIED; secondary
       sources only)**.
     - GNOME Software on Fedora and KDE Discover handling local packages is **(UNVERIFIED)**.
  4. **Open the release page**, as VS Code does for Linux and WezTerm and Rerun do everywhere.
  5. **A package repository of our own**, VS Code's and Chrome's way: updates then come with
     `apt upgrade` and no "Update now" is needed. `docs/distribution.md` defers this ("later, if
     at all").
- **After installing:** the package manager has replaced `/usr/bin/parterre` and updated its
  database, so `dpkg -l` and `rpm -q` agree with what runs. parterre re-executes
  `/usr/bin/parterre` with its arguments **(derived)**.
- **Failure:** a refused password or a dependency error leaves the old package installed, and
  the exit code can be shown **(derived)**.
- **Telling the formats apart:** the binary is `/usr/bin/parterre`, and `dpkg -S` or `rpm -qf`
  names the owning package. A cargo build feature or build-time variable per package, like Zed's
  `ZED_UPDATE_EXPLANATION`, says it without asking at run time **(derived)**.
- **Cost (derived):** option 1 is one command per distro family plus `pkexec`, with no new
  crates. Option 2 adds `zbus` (not checked further). Options 3 and 4 need only a download or a
  URL. No packaging change for any of them.

## 6. cargo install

- **What cargo records:** "By default, Cargo keeps track of the installed packages with a
  metadata file stored in the installation root directory". `cargo install` "will reinstall it
  if the installed version does not appear to be up-to-date"
  ([cargo install](https://doc.rust-lang.org/cargo/commands/cargo-install.html)).
- **If parterre replaced its own binary:** that record (and `cargo install-update` from
  `cargo-update`) would still name the old version **(derived)**.
- **What comparable tools do:**
  - uv refuses without its own receipt and names the user's tool (§2.1);
  - Alacritty defers to package managers;
  - `cargo binstall` recommends `cargo-update` for updating
    ([README](https://github.com/cargo-bins/cargo-binstall/blob/7bebc2e59eb8820162b7ac8f62a92bfdb2732447/README.md)).
- **Recommended:** "Update now" shows `cargo install --locked parterre` and
  `cargo binstall parterre`, with a copy button. A cargo-installed parterre sits in
  `$CARGO_HOME/bin` (default `~/.cargo/bin`) **(derived)**.
- **Cost:** none beyond the text.

## 7. Mechanics shared by all channels

### 7.1 Download and verify

- **What exists today:** a `SHA256SUMS` asset per release. GitHub also computes "SHA256
  checksums (digests) for all uploaded release assets … generated at upload time" and exposes
  them in the REST API
  ([changelog](https://github.blog/changelog/2025-06-03-releases-now-expose-digests-for-release-assets/)).
  - parterre's v0.6.0 assets each carry `digest: sha256:…` **(tested 2026-10-04: `gh api
    repos/aquamoth/parterre/releases/latest`)**.
  - The update check already reads that response (#225), so the download URL and its digest
    come with it and no second request is needed **(derived)**.
- **What a digest proves:** that the file arrived intact. It doesn't prove who made it, since
  "the forge recomputes the digest if an asset is replaced"
  ([self_update README](https://github.com/jaemk/self_update/blob/fc840459d2ba11dec2b04a8db053e919730b1d13/README.md)).
  The same holds for `SHA256SUMS` fetched from the same release.
- **Immutable releases:** "Once you publish a release as immutable, its assets can't be added,
  modified, or deleted", and tags "can't be deleted or moved". They come with signed Sigstore
  attestations, checked by `gh release verify` / `gh release verify-asset`
  ([changelog](https://github.blog/changelog/2025-10-28-immutable-releases-are-now-generally-available/),
  [docs](https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/verifying-the-integrity-of-a-release)).
  - parterre's repository has them off (`{"enabled":false}`) **(tested 2026-10-04: `gh api
    repos/aquamoth/parterre/immutable-releases`)**.
  - They protect a published release against later replacement, not against a compromised
    release workflow **(derived)**.
- **Signatures:**
  - **Tauri:** minisign, mandatory, public key in the app.
  - **TortoiseGit:** OpenPGP `.rsa.asc`.
  - **cargo-binstall:** minisign, opt-in.
  - **Velopack:** relies on Authenticode once you sign.
  - **For parterre:** a minisign key in a CI secret, one `.minisig` per asset, and
    `minisign-verify` 0.3.0 in the app. That gives authenticity on every channel, and
    `cargo binstall` uses the same files **(derived)**.
  - **Authenticode** (Artifact Signing, about $10/month, `docs/distribution.md`) would also
    change the UAC prompt from yellow to grey and give SmartScreen a publisher.

### 7.2 Mark of the Web, SmartScreen and Smart App Control

- **SmartScreen** checks "downloaded files", and "protects against malicious files from the
  internet"
  ([overview](https://learn.microsoft.com/en-us/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/)).
  An unsigned file gets "Windows protected your PC" and "Run anyway"
  ([reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)).
- **Why parterre's own download should escape it:** the mark is written by the program that
  saves the download (browsers, mail clients through
  [`IAttachmentExecute`](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-iattachmentexecute)).
  A file parterre saves with `ureq` carries none, so neither running it nor `msiexec` on it
  shows SmartScreen's prompt.
  - That matches `docs/distribution.md`: winget and Chocolatey downloads show no prompt. None of
    the updaters read (Zed, VS Code, Tauri, TortoiseGit) adds or strips a mark.
  - Taken together this is **(derived)**: no Microsoft page found states the "only with a mark"
    rule outright.
- **Smart App Control** "signature checks apply to all executable files, not just those
  downloaded from the Internet" (same reputation page). It blocks "unknown, unsigned code"
  ([Smart App Control](https://learn.microsoft.com/en-us/windows/apps/develop/smart-app-control/overview)).
  A machine with it on can't run today's parterre either, so in-app updating changes nothing
  there **(derived)**.
- **UAC** for a machine-wide update shows the yellow "unsigned" prompt (§3.1).

### 7.3 Quitting and restarting

- **Windows MSI:** start msiexec, then exit at once so `parterre.exe` is free. Tauri calls
  `exit(0)` right after `ShellExecuteW`. Restart via a waiter process or an MSI custom action
  (§3.1).
- **Zip and tarballs:** spawn the replaced binary, then exit. On Windows `self-replace` deletes
  the old file after exit.
- **.deb / .rpm:** the package manager runs while parterre still runs, since Unix keeps the old
  inode open. Then parterre re-executes itself **(derived)**.
- **Arguments:** VS Code writes its relaunch arguments to a file for the installer
  ([L614-L660](https://github.com/microsoft/vscode/blob/24a41178148f72f49e4ac0756ddb3b5347429a91/src/vs/platform/update/electron-main/updateService.win32.ts#L614-L660)).
  Tauri passes `LAUNCHAPPARGS`. parterre would pass its repository path the same way.
- **Unsaved state:** parterre keeps settings and layouts on disk, so restarting loses nothing
  that a normal quit wouldn't **(derived; not checked per dialog)**.

### 7.4 When the update fails part-way

| Step | What happens | Recovery |
|---|---|---|
| Download | Partial file in a temporary location | Delete it, keep running the old version, offer the release page |
| Digest or signature mismatch | Nothing installed | Same; say so |
| MSI | Windows Installer rolls back to the old version | Show the exit code (1602 cancelled, 1603 failed, 1618 busy) |
| Archive replace | Unix: atomic rename, old or new; Windows: see §4 | Keep the extracted file; offer the release page |
| `pkexec` install | Refused password or dependency error leaves the old package | Show the command so the user can run it in a terminal |

## 8. Recommendation per channel (derived)

- **MSI, per-user (direct and winget):**
  - Download the MSI from the release with the digest from the update check, verify it, then
    run `msiexec /i … /passive /norestart`.
  - Exit, and let a waiter restart parterre and report a failure.
  - No packaging change. Leave `RequireExplicitUpgrade` unset, as comparable apps do.
- **MSI, machine-wide (Chocolatey or `ALLUSERS=1`):** the same with `ALLUSERS=1`, accepting the
  UAC prompt. Chocolatey keeps listing the old version; the next `choco upgrade` reinstalls
  harmlessly. Optionally word the entry "requires administrator".
- **winget portable:** don't self-update; show `winget upgrade Trustfall.Parterre`. Or drop the
  zip from the winget manifest while the first PR is still open.
- **Windows zip:** replace in place with `self-replace` after a digest check, and restart. If
  the folder isn't writable, open the release page.
- **Linux and macOS tarballs:** the same; on macOS the rename is what keeps code signing intact.
- **.deb / .rpm from GitHub:** download, verify, `pkexec apt-get install -y` /
  `dnf install -y` / `zypper install --allow-unsigned-rpm`, then re-execute. Fall back to
  opening the release page when pkexec is missing. A repository of our own remains the
  long-term way.
- **cargo install:** show `cargo install --locked parterre` / `cargo binstall parterre`.
- **All channels:** first turn on immutable releases (free, no code). Then add minisign
  signatures if authenticity is wanted beyond HTTPS plus GitHub's digests. Velopack and
  axoupdater are not worth their packaging changes for parterre's channel mix.

## Not checked

- msiexec's exact behaviour with `/qn` for an unelevated per-machine update, and with files in
  use under `/passive`.
- Whether a WiX custom action after `InstallFinalize` starts parterre elevated after a
  machine-wide update.
- Whether pkexec, PackageKit and a local-package handler are installed by default on Ubuntu,
  Debian, Fedora, openSUSE and Mint desktops.
- Ubuntu App Center's local `.deb` support from a primary source.
- How Obsidian updates on each Linux package; GitHub Desktop's ARP entry after a Squirrel update.
- Quarantine and Gatekeeper behaviour for a binary parterre downloads itself on macOS.
- Whether `self-replace` rolls back on Windows when the copy fails after moving the old exe.
