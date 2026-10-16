# How desktop apps learn their install channel at run time

Research note for [#395](https://github.com/aquamoth/parterre/issues/395), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390): *how do other desktop apps tell, from
one binary, how they were installed? What does each of parterre's channels let a package add
cheaply, and how robust is each way against a user moving or copying the binary?*

**Channel** here means how this copy was installed (`CONTEXT.md` › *Builds and rings*), not
which builds it hears of (its **ring**). The ring is #396's question; §5 only notes where it
touches this one.

Sources are the apps' own source code (default branches, read 2026-10-16) and the vendors' own
documentation. A statement that is my own conclusion is marked **(derived)**. A statement I
could not check is marked **(unverified)**.

## TL;DR

- **The common pattern is a small file the package adds next to the program, and no file means
  the plain archive.** VS Code's Windows setup writes `"target": "user"|"system"` into its own
  copy of `product.json`; the zip's `product.json` has no `target`, and that makes it the archive
  (§1.1). electron-builder writes `resources/package-type` (`deb`, `rpm`, `pacman`, and now
  `nsis`/`nsis-web`) and electron-updater picks its updater from it; no file means AppImage
  (§1.2). Firefox's deb and rpm add an `is-packaged-app` file beside the binary, and Mozilla's deb,
  rpm and Flatpak and Canonical's snap each add their own `distribution/distribution.ini` to the
  same build (§1.3).
- **Sandboxes are told by the sandbox, not by a marker.** VS Code checks `SNAP` and
  `SNAP_REVISION`; Firefox checks `SNAP_NAME` (matched to its own name) and the existence of
  `/.flatpak-info` (§1.1, §1.3). parterre already does the same with `SNAP_NAME` and
  `FLATPAK_ID`.
- **Compile-time stamps are what parterre does today, and the apps that do it build per
  channel:** Zed's release channel and GitHub Desktop's `__RELEASE_CHANNEL__` (§1.4).
  Firefox compiles its update channel in only as the fallback for a file beside the binary
  (§1.3). Chrome reads its channel from its install path on Windows and
  from a variable its Linux launcher script exports (§1.5); both break when the binary is moved
  or started directly.
- **For parterre, every channel can get its marker without a rebuild (derived, §3):**

  | Channel | Marker | Robust when moved or copied |
  |---|---|---|
  | zip, tarball | none: the default for a release binary | n/a |
  | MSI (winget, Chocolatey) | a file beside `parterre.exe` (WiX `File`) | moves with the folder; a lone copied `parterre.exe` reads as zip |
  | dmg | a key in `parterre.app`'s `Info.plist` (or simply: running from inside an `.app`) | the bundle moves whole; a binary taken out reads as tarball |
  | deb, rpm | a data file the package installs, found relative to the binary (`../lib/parterre/…`) | a copied `/usr/bin/parterre` reads as tarball |
  | Snap, Flatpak | `SNAP_NAME`, `FLATPAK_ID` / `/.flatpak-info` (as today) | can't be copied out of the sandbox |
  | cargo | none needed: `cargo install` compiles anyway, and `build.rs` already knows a crates.io build (`Cargo.toml.orig`) | n/a |

- **Losing a marker degrades to the archive's behaviour, which is the right answer for a loose
  binary** (derived): its update is the archive download. A file next to the binary is
  therefore better than a registry value or a path check, which a copied or moved binary can
  mismatch in the wrong direction (claiming *MSI* for a zip unpacked on a machine that also has
  the MSI).
- **Avoid install-time properties** (winget `InstallerSwitches.Custom`, Chocolatey
  `silentArgs`) for anything that must last: Windows Installer ignores command-line properties
  on repair (§2.1). winget and Chocolatey install the same MSI, and the map keeps them stable
  only, so *MSI* is the right channel for them.

## 1. How other apps do it

### 1.1 VS Code: `product.json` per package, `SNAP` for the snap

- **One build, a different `product.json` per Windows setup.** The setup task copies the built
  app's `resources/app/product.json`, sets `productJson['target'] = target` (`user` or
  `system`) and hands the result to Inno Setup, which installs it over the build's copy
  ([`build/gulpfile.vscode.win32.ts`, `buildWin32Setup`, ~l. 61–114](https://github.com/microsoft/vscode/blob/main/build/gulpfile.vscode.win32.ts);
  [`build/win32/code.iss`, ~l. 102–110](https://github.com/microsoft/vscode/blob/main/build/win32/code.iss)
  leaves the original out and installs the copy). The zip is the same build without that field.
- **No `target` means the archive.** `isInnoSetupInstall(target)` is `target === 'user' ||
  target === 'system'`, and `getWin32UpdateType` returns `UpdateType.Setup` for it and
  `UpdateType.Archive` otherwise
  ([`src/vs/platform/update/electron-main/win32UpdateType.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/win32UpdateType.ts)).
  The archive asks a feed with `-archive` in its name and only offers a download
  (`State.AvailableForDownload`); the user setup gets `-user` and installs in the background
  ([`updateService.win32.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/updateService.win32.ts)).
  This is the closest match to what #390 wants.
- **Snap from the environment.** `isLinuxSnap = isLinux && !!env['SNAP'] && !!env['SNAP_REVISION']`
  ([`src/vs/base/common/platform.ts`, l. 79](https://github.com/microsoft/vscode/blob/main/src/vs/base/common/platform.ts)),
  and `app.ts` then picks `SnapUpdateService` with `SNAP` and `SNAP_REVISION`
  ([`src/vs/code/electron-main/app.ts`, ~l. 1231](https://github.com/microsoft/vscode/blob/main/src/vs/code/electron-main/app.ts)),
  which compares `SNAP_REVISION` with the revision `$SNAP/../current` points at.
- **deb and rpm are not told apart.** The Linux service always reports `UpdateType.Archive`
  and opens the download page, with the comment *"we don't currently detect the package type
  that was installed"*
  ([`updateService.linux.ts`, `doDownloadUpdate`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/updateService.linux.ts)).
  (Its deb and rpm add Microsoft's own apt/yum repository, so the system updates them.)
- **Portable mode** is another file-beside-the-binary marker: a `data` folder in the zip's or
  tarball's folder ([code.visualstudio.com/docs/editor/portable](https://code.visualstudio.com/docs/editor/portable)),
  which counts only when `product.json` has no `target`
  ([`src/bootstrap-node.ts`, ~l. 225–239](https://github.com/microsoft/vscode/blob/main/src/bootstrap-node.ts)).

### 1.2 electron-builder / electron-updater: `resources/package-type`

- **Written by the Linux package build.** For deb, rpm and pacman, `FpmTarget` writes
  `app-update.yml` and *"Extra file needed for auto-updater to detect installation method"*:
  `outputFile(path.join(resourceDir, "package-type"), target)`
  ([`packages/app-builder-lib/src/targets/linux/FpmTarget.ts`, ~l. 184–185](https://github.com/electron-userland/electron-builder/blob/master/packages/app-builder-lib/src/targets/linux/FpmTarget.ts)).
- **Written by the Windows installer at install time.** The NSIS install section writes
  `$INSTDIR\resources\package-type` as `nsis` or `nsis-web`
  ([`templates/nsis/installSection.nsh`, ~l. 83–91](https://github.com/electron-userland/electron-builder/blob/master/packages/app-builder-lib/templates/nsis/installSection.nsh)).
  The changeset says why: *"the only build artifact that differs between `nsis` and `nsis-web`
  (the app payload is byte-identical, since both targets share one app archive)"*
  ([`.changeset/nsis-web-package-type-marker.md`](https://github.com/electron-userland/electron-builder/blob/master/.changeset/nsis-web-package-type-marker.md)).
- **Read once at start, with a default.** On Linux, electron-updater starts from
  `AppImageUpdater`, reads `path.join(process.resourcesPath, "package-type")` if it exists, and
  switches to `DebUpdater`, `RpmUpdater` or `PacmanUpdater`; a missing file or an unknown value
  keeps the default, a read error only warns
  ([`packages/electron-updater/src/index.ts`, ~l. 36–61](https://github.com/electron-userland/electron-builder/blob/master/packages/electron-updater/src/index.ts)).
- This is the design §3 recommends, almost exactly: one payload, a one-word file the package
  adds, the bare archive as the default.

### 1.3 Firefox: `distribution.ini` beside the binary; sandbox checks

- **`distribution.ini`** is read from the directory service's `XREAppDist`
  ([`browser/components/distribution.sys.mjs`, `_iniFile`, ~l. 29–48](https://github.com/mozilla-firefox/firefox/blob/main/browser/components/distribution.sys.mjs)),
  which is `NS_GRE_DIR` plus `distribution`, i.e. a folder beside the installed program
  ([`toolkit/xre/nsXREDirProvider.cpp`, `XRE_APP_DISTRIBUTION_DIR`, ~l. 477–481](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/xre/nsXREDirProvider.cpp)).
  Its `[Global] id` becomes the default pref `distribution.id` (same `.mjs`, ~l. 410). Mozilla's
  own repackaging adds it to the unchanged build per package: `id=mozilla-deb` and
  `id=mozilla-rpm` from `python/mozbuild/mozbuild/repackaging/deb.py` (~l. 128) and `rpm.py`
  (~l. 103), with the template
  [`browser/installer/linux/app/debian/distribution.ini`](https://github.com/mozilla-firefox/firefox/blob/main/browser/installer/linux/app/debian/distribution.ini);
  `id=mozilla-flatpak` copied in by `repackaging/flatpak.py` from
  [mozilla-partners/flatpak](https://github.com/mozilla-partners/flatpak/blob/master/desktop/flatpak/distribution/distribution.ini);
  and Canonical's snap copies in `id=canonical-002`
  ([canonical/firefox-snap `snapcraft.yaml`, ~l. 479–484](https://github.com/canonical/firefox-snap/blob/stable/snapcraft.yaml),
  [mozilla-partners/canonical](https://github.com/mozilla-partners/canonical/blob/master/desktop/ubuntu/distribution/distribution.ini)).
- **`is-packaged-app`: Firefox's own "installed by a package" marker.** On Linux,
  `isPackagedApp` is `IsRunningUnderFlatpakOrSnap() || IsPackagedAppFileExists()`
  ([`xpcom/base/nsSystemInfo.cpp`, ~l. 1759](https://github.com/mozilla-firefox/firefox/blob/main/xpcom/base/nsSystemInfo.cpp)),
  the file being `is-packaged-app` in the GRE dir beside the binary
  ([`WidgetUtilsGtk.cpp`, ~l. 253](https://github.com/mozilla-firefox/firefox/blob/main/widget/gtk/WidgetUtilsGtk.cpp)),
  which the deb and rpm repackaging writes with the text *"This is a packaged app."*
  (`deb.py` ~l. 126, `rpm.py` ~l. 99). A packaged app gets no update UI
  ([`toolkit/mozapps/update/AppUpdater.sys.mjs`, ~l. 424](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/mozapps/update/AppUpdater.sys.mjs)).
  On Windows, `isPackagedApp` means MSIX package identity (`nsSystemInfo.cpp` ~l. 1604–1616),
  and the NSIS installer leaves `installation_telemetry.json` in the install folder
  ([`browser/installer/windows/nsis/installer.nsi`, ~l. 1129](https://github.com/mozilla-firefox/firefox/blob/main/browser/installer/windows/nsis/installer.nsi)).
  This is the same design as §1.2 and §3, one level more general.
- **Update channel:** `UpdateUtils.getUpdateChannel` reads the default pref
  `app.update.channel`, falling back to the compiled-in `AppConstants.MOZ_UPDATE_CHANNEL`
  ([`toolkit/modules/UpdateUtils.sys.mjs`, ~l. 36–41](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/modules/UpdateUtils.sys.mjs)).
  The pref sits alone in `defaults/pref/channel-prefs.js`, a file beside the binary
  ([`browser/app/profile/channel-prefs.js`](https://github.com/mozilla-firefox/firefox/blob/main/browser/app/profile/channel-prefs.js)),
  except on macOS: *"channel-prefs.js has been removed on macOS"*
  ([`browser/installer/package-manifest.in`, ~l. 277–283](https://github.com/mozilla-firefox/firefox/blob/main/browser/installer/package-manifest.in)),
  where it now comes from a separate `ChannelPrefs.framework`, falling back to the compiled-in
  value ([`modules/libpref/Preferences.cpp`, ~l. 6169–6187](https://github.com/mozilla-firefox/firefox/blob/main/modules/libpref/Preferences.cpp),
  bug 1799332): a signed bundle can't take a file added after signing (§2.3). The updater
  reads its accepted MAR channels from `update-settings.ini` beside the binary
  ([`toolkit/mozapps/update/updater/updater.cpp`, ~l. 3312, 3366](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/mozapps/update/updater/updater.cpp)).
- **Snap:** `GetSnapInstanceName()` reads `SNAP_NAME` and accepts it only if it is Firefox's
  own name (`MOZ_APP_NAME` or `…-devel`), then `SNAP_INSTANCE_NAME`; `IsRunningUnderSnap()` is
  that being set. **Flatpak:** `IsRunningUnderFlatpak()` is whether `/.flatpak-info` exists,
  following GTK
  ([`widget/gtk/WidgetUtilsGtk.cpp`, ~l. 175–183 and 278–290](https://github.com/mozilla-firefox/firefox/blob/main/widget/gtk/WidgetUtilsGtk.cpp)).
  parterre's `Channel::of` already matches `SNAP_NAME` to its own name for the same reason
  (a snap's variables leak into what is started from it).

### 1.4 Apps that stamp at compile time

- **Zed:** the release channel is compiled in from `ZED_RELEASE_CHANNEL` (overridable at run
  time by the same variable in dev builds), so each channel is its own build
  ([`crates/release_channel/src/lib.rs`, l. 13–32](https://github.com/zed-industries/zed/blob/main/crates/release_channel/src/lib.rs)).
  For packagers it reads `ZED_UPDATE_EXPLANATION` both at compile time (`option_env!`) and at
  run time (`env::var`): set, it stops polling and *Check for Updates* says *"Zed was installed
  via a package manager."* with that text
  ([`crates/auto_update/src/auto_update.rs`, ~l. 285–325](https://github.com/zed-industries/zed/blob/main/crates/auto_update/src/auto_update.rs)).
  Distro packages build Zed themselves, so a compile-time switch costs them nothing; a release
  binary can't be told this way except by the environment, which a launcher would have to set
  **(derived)**.
- **GitHub Desktop:** `__RELEASE_CHANNEL__` (`production`, `beta`, `test`, `development`) is a
  build-time constant
  ([`app/src/ui/lib/update-store.ts`, ~l. 307–368](https://github.com/desktop/desktop/blob/development/app/src/ui/lib/update-store.ts)).
  It ships one installer kind per platform (Squirrel on Windows), so it has no install channel to
  tell apart **(derived)**.

### 1.5 Chrome: the install path on Windows, a wrapper script on Linux

- **Windows:** the channel is worked out from the exe's own path. `GetInstallSuffix` looks for
  `\Chrome<suffix>\Application` in it, and the suffix picks the install mode: none is stable,
  ` Beta`, ` Dev` and ` SxS` are beta, dev and canary; an unknown suffix falls back to stable
  ([`chrome/install_static/product_install_details.cc`, l. 32–40, 83–121](https://github.com/chromium/chromium/blob/main/chrome/install_static/product_install_details.cc),
  [`google_chrome_install_modes.h`](https://github.com/chromium/chromium/blob/main/chrome/install_static/google_chrome_install_modes.h)).
  Being under Program Files makes it system-level (same file, l. 64–78). So moving a Beta out
  of its folder silently makes it stable.
- **Linux:** the launcher script the package installs does `export CHROME_VERSION_EXTRA="@@channel"`,
  filled in when the package is built
  ([`chrome/installer/linux/common/wrapper`, l. 18](https://github.com/chromium/chromium/blob/main/chrome/installer/linux/common/wrapper)),
  and branded builds read that variable
  ([`chrome/common/channel_info_posix.cc`, l. 39–50](https://github.com/chromium/chromium/blob/main/chrome/common/channel_info_posix.cc)).
  The binary run without the wrapper has no channel. A wrapper script is another marker a
  package can add without a rebuild, but parterre's packages install the binary itself at
  `/usr/bin/parterre`, and file managers' menus start it directly (derived).
- **Zed's CLI** checks `FLATPAK_ID` (starting `dev.zed.Zed`) and sets `ZED_UPDATE_EXPLANATION`
  for the editor it starts
  ([`crates/cli/src/main.rs`, ~l. 1091–1098](https://github.com/zed-industries/zed/blob/main/crates/cli/src/main.rs)).

## 2. The kinds of marker, and what each platform allows

### 2.1 Windows: a file beside the exe, or a registry value

- **A file the MSI installs.** WiX `File`'s `Source` is a path at build time, so its content is
  fixed per MSI ([WiX `File`](https://docs.firegiant.com/wix/schema/wxs/file/)). parterre's
  `parterre.wxs` already installs `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES.html` this way;
  one more file is one line and one staged file in `build-msi.ps1`. The MSI is built per
  release anyway, so a fixed content is enough.
- **A value from a property.** `IniFile` writes `Section`/`Key`/`Value`, which are *Formatted*,
  so `Value="[CHANNEL]"` writes a property into an `.ini` in `INSTALLFOLDER`
  ([WiX `IniFile`](https://docs.firegiant.com/wix/schema/wxs/inifile/),
  [IniFile table](https://learn.microsoft.com/en-us/windows/win32/msi/inifile-table),
  [Formatted](https://learn.microsoft.com/en-us/windows/win32/msi/formatted)). Needed only if
  one MSI must say different things depending on who runs it.
- **A registry value.** `RegistryValue`'s `Value` is formatted, and `HKMU` is HKCU for a
  per-user install, HKLM otherwise ([WiX `RegistryValue`](https://docs.firegiant.com/wix/schema/wxs/registryvalue/)).
  parterre already writes `HKMU\Software\Trustfall AB\parterre`. But a registry value says the
  MSI is installed somewhere, not that *this* exe is it: a zip unpacked elsewhere on the same
  machine would read it too, so it needs a path check besides (derived).
- **Asking Windows Installer.** `MsiEnumRelatedProducts` lists installed products by
  UpgradeCode ([learn.microsoft.com](https://learn.microsoft.com/en-us/windows/win32/api/msi/nf-msi-msienumrelatedproductsw)),
  and `MsiGetProductInfo(…, INSTALLPROPERTY_INSTALLLOCATION)` returns `ARPINSTALLLOCATION`
  ([learn.microsoft.com](https://learn.microsoft.com/en-us/windows/win32/api/msi/nf-msi-msigetproductinfow)),
  which `parterre.wxs` sets for winget. That works but loads `msi.dll` at start, and still needs
  the path comparison (derived).
- **Install-time properties don't last.** A public property can be set on the `msiexec` command
  line, but repair *"ignores any property values entered on the command line"*
  ([Command-line options](https://learn.microsoft.com/en-us/windows/win32/msi/command-line-options)).
  winget can pass one through `InstallerSwitches.Custom`
  ([installer manifest schema](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.12.0/installer.md)),
  Chocolatey through `silentArgs`
  ([Install-ChocolateyPackage](https://docs.chocolatey.org/en-us/create/functions/install-chocolateypackage/)).
  Neither winget nor Chocolatey leaves a documented mark of its own that an app could read:
  `winget list` matches apps to manifests through their Add/Remove Programs entries
  ([winget list](https://learn.microsoft.com/en-us/windows/package-manager/winget/list)), and
  Chocolatey keeps its package in `%ChocolateyInstall%\lib\<id>`, apart from where the MSI puts
  the program ([Chocolatey FAQ](https://docs.chocolatey.org/en-us/faqs/)).
- **Path heuristics** (exe under `FOLDERID_UserProgramFiles` or Program Files,
  [KNOWNFOLDERID](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid)) fail both
  ways: users unzip into those folders too. No Microsoft convention for "installed or portable"
  exists that I found **(unverified)**.

### 2.2 Linux packages: a data file; the package database is too costly

- A deb or rpm can install any extra file through `cargo-deb`'s and `cargo-generate-rpm`'s
  `assets` lists (`crates/parterre/Cargo.toml`), at no cost. The two packages are built from one
  binary by `packaging/linux/build-packages.sh`, which can write a one-word file per package
  before calling each tool (derived).
- Asking the package manager works but costs a process per start: `dpkg-query -S` and
  `rpm -qf` name the package owning a file
  ([dpkg-query(1)](https://man7.org/linux/man-pages/man1/dpkg-query.1.html),
  [rpm(8)](https://man7.org/linux/man-pages/man8/rpm.8.html)), and dpkg's database files are
  internal ([Debian dpkg FAQ](https://wiki.debian.org/Teams/Dpkg/FAQ)). No app above does it;
  Firefox ships a file instead (§1.3).
- Where: FHS puts read-only architecture-independent data in `/usr/share/<pkg>`
  ([FHS 3.0 §4.11](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s11.html)) and data used
  only by the program, architecture-dependent, in `/usr/lib/<pkg>`
  ([§4.6](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/ch04s06.html)); Firefox's markers
  are in `/usr/lib/firefox` because its whole tree, binary included, is there. Either way, read
  it relative to the binary (`current_exe()/../share/parterre/channel` or
  `…/../lib/parterre/channel`) rather than at an absolute path, so a binary copied elsewhere
  doesn't pick it up. `lib` is less likely than `share` to clash with anything parterre writes
  itself, while `…/share/parterre` could meet a tarball user's `~/.local/share/parterre`
  (derived).

### 2.3 macOS: inside the bundle, before signing

- `Info.plist` may carry custom keys, *"as long as your key names do not conflict with the ones
  Apple uses… prefix them with a unique prefix, such as your app's bundle ID"*
  ([About Info.plist files](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/AboutInformationPropertyListFiles.html)),
  read with `object(forInfoDictionaryKey:)`
  ([Information Property List](https://developer.apple.com/documentation/bundleresources/information-property-list)).
- A signature *"always seal[s] all files in a bundle… Bundles should be treated as read-only
  once they have been signed"*, and writing data after the first run *"still breaks the
  signature"*; `Contents/MacOS` is for code only, data goes in `Contents/Resources`
  ([TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html)). So the
  marker goes in before `build-dmg.sh` signs ad hoc, which it already does after filling in
  `Info.plist`'s `@VERSION@`.
- Only the dmg ships a `.app`, so *running from `….app/Contents/MacOS/parterre`* already says
  dmg without any key (derived). A key is still cheaper to reason about if anything else ever
  wraps the bundle.
- App Translocation runs a quarantined app from a random read-only path, and Apple has no
  supported way to detect it ([Apple Platform Security](https://support.apple.com/guide/security/gatekeeper-and-runtime-protection-sec5599b66df/web),
  [DTS on the forums](https://developer.apple.com/forums/thread/724969)); it moves the whole
  bundle, so a marker inside the bundle still reads right, unlike a path check against
  `/Applications` (derived).

### 2.4 Snap and Flatpak: the sandbox says so

- **Snap:** `snap run` sets `SNAP`, `SNAP_NAME`, `SNAP_INSTANCE_NAME`, `SNAP_REVISION` and
  `SNAP_REAL_HOME` for the app
  ([snapd `snap/snapenv/snapenv.go`, `basicEnv` ~l. 109–128, `userEnv` ~l. 250](https://github.com/canonical/snapd/blob/master/snap/snapenv/snapenv.go)).
  Documented under *Snap specific environment variables*
  ([snapcraft.io](https://snapcraft.io/docs/reference/development/environment-variables/)), which
  says nothing about inheritance; on the forum, what a classic snap starts *"will inherit the snap
  environment"* ([forum.snapcraft.io/t/12275](https://forum.snapcraft.io/t/launching-external-apps-from-inside-a-classic-snap/12275)).
  So match the name (as parterre and Firefox do); GLib also requires `$SNAP/meta/snap.yaml`
  ([`gio/gsandbox.c`, ~l. 87–113](https://gitlab.gnome.org/GNOME/glib/-/blob/main/gio/gsandbox.c)).
- **Flatpak:** `flatpak run` sets `FLATPAK_ID` to the app ID
  ([flatpak `common/flatpak-run.c`, ~l. 867](https://github.com/flatpak/flatpak/blob/main/common/flatpak-run.c))
  and creates `/.flatpak-info` *"that the app cannot modify"* (same file, ~l. 1665–1698).
  The docs: *"Flatpak sets the environment variable FLATPAK_ID to the application ID of the
  running app"*, and *"A metadata file describing the effective configuration is available inside
  the running sandbox at /.flatpak-info"*
  ([Flatpak command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html),
  flatpak-run and flatpak-metadata). `flatpak-spawn --host` passes on only the variables given
  with `--env`
  ([flatpak-xdg-utils `flatpak-spawn.c`, ~l. 1009–1012](https://github.com/flatpak/flatpak-xdg-utils/blob/main/src/flatpak-spawn.c)),
  so a host program doesn't inherit `FLATPAK_ID`. GLib, GTK and Firefox test `/.flatpak-info`;
  `FLATPAK_ID` can be set by hand (derived). Both are fine for an update-check label.

### 2.5 Reading a file beside the binary

`std::env::current_exe()`: through a symlink, *"some platforms will return the path of the
symbolic link and other platforms will return the path of the symbolic link's target"*; a
renamed binary may report its old path; and on some Unixes it is computed from `argv[0]` and
`$PATH`, so it is not to be trusted for security
([std docs](https://doc.rust-lang.org/std/env/fn.current_exe.html)). For a channel label that is
acceptable; canonicalize it so a `~/bin/parterre` symlink to `/usr/bin/parterre` still finds
`/usr/lib/parterre` (derived).

## 3. parterre's channels, one by one (derived)

The baseline: release.yml builds **one** binary per target; today's `PARTERRE_CHANNEL` builds
for `msi`, `dmg`, `deb` and `rpm` go. The release binary without a marker is the archive of its
platform (zip on Windows, tarball elsewhere), as VS Code's zip and electron-builder's AppImage
are the default of theirs.

| Channel | What the package adds | Cost | Moved / copied |
|---|---|---|---|
| **zip** | nothing | none | any copy is still a zip: right |
| **tarball** (Linux, macOS) | nothing | none | same |
| **MSI** | a one-word file in `INSTALLFOLDER` (WiX `File`), next to `parterre.exe` | one `File` line, one staged file | the folder moved whole: still MSI; `parterre.exe` copied alone: zip, whose update is the right one for it. A copied whole folder still says MSI: harmless, the user gets the MSI download |
| **winget, Chocolatey** | nothing of their own: they install the MSI | none | as MSI. Telling them apart would need an install-time property (`IniFile` + `[CHANNEL]`), lost on repair (§2.1); not worth it while they only carry stable |
| **dmg** | a prefixed key in `Info.plist` (e.g. `SETrustfallParterreChannel = dmg`), or nothing if "inside an `.app`" is the test | one `plutil`/`sed` before `codesign` | dragging the `.app` anywhere keeps it; translocation keeps it; the bare binary copied out reads as tarball |
| **deb / rpm** | `/usr/lib/parterre/channel` with `deb` or `rpm`, an asset in each package, found as `<exe>/../lib/parterre/channel` | two asset lines; `build-packages.sh` writes the file per package | a copied `/usr/bin/parterre` reads as tarball |
| **cargo** | nothing: `cargo install` compiles, and `build.rs` already sees a packaged crate (`Cargo.toml.orig`) | none | n/a |
| **Snap** | nothing: `SNAP_NAME=parterre` (as today) | none | can't leave the sandbox |
| **Flatpak** | nothing: `FLATPAK_ID=se.trustfall.parterre`, or `/.flatpak-info` (as today) | none | can't leave the sandbox |

Order of checks (derived): Snap, Flatpak, then a marker file or key, then "built from a crates.io
package or a checkout" (compile time, already known to `build.rs`), then the archive of the
platform. A dev build is not a release binary and stays as today (#390 leaves its ring open).

Also worth carrying into #396:

- The marker is **data, not code**, so the per-channel symbol uploads and PDBs in `release.yml`
  collapse to one per target: every package has the same build ID / `LC_UUID` / PDB (derived).
- The marker must be written only into the package it belongs to, never into the shared
  staging folder the zip is made from. electron-builder writes `package-type` into the shared
  `linux-unpacked/resources`, so a target built after it from that folder could carry a stale one
  (the research agent's reading of `FpmTarget.ts`, l. 170–186; **unverified**).
- `tests/packaging.rs` can check that each built package contains its marker, as it checks the
  package dependencies today (derived).
- A marker file names a format (`msi`, `deb`, …), so the same file could carry a second line
  later. But a marker is fixed when the package is built, so it can at most say which ring a
  build was **published for**; the installation's own, widening ring (#390) has to be kept
  beside the user's settings, which survive upgrades (derived).

## 4. Open questions for the human

- **Marker name and format:** one word in a file named `channel`, as electron-builder's
  `package-type`, or a small key=value file that can grow (Firefox's `distribution.ini`)?
- **dmg:** an `Info.plist` key, or just "inside a `.app`"?
- **Should winget and Chocolatey be told apart from the MSI** at all? With only stable on them,
  the MSI's update check already offers the right file; the difference would matter only if
  parterre ever says "run `winget upgrade`" instead.

## Sources

- VS Code: [`build/gulpfile.vscode.win32.ts`](https://github.com/microsoft/vscode/blob/main/build/gulpfile.vscode.win32.ts),
  [`win32UpdateType.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/win32UpdateType.ts),
  [`updateService.win32.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/updateService.win32.ts),
  [`updateService.linux.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/updateService.linux.ts),
  [`updateService.snap.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/platform/update/electron-main/updateService.snap.ts),
  [`platform.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/base/common/platform.ts),
  [`app.ts`](https://github.com/microsoft/vscode/blob/main/src/vs/code/electron-main/app.ts),
  [portable mode](https://code.visualstudio.com/docs/editor/portable).
- electron-builder: [`FpmTarget.ts`](https://github.com/electron-userland/electron-builder/blob/master/packages/app-builder-lib/src/targets/linux/FpmTarget.ts),
  [`installSection.nsh`](https://github.com/electron-userland/electron-builder/blob/master/packages/app-builder-lib/templates/nsis/installSection.nsh),
  [`electron-updater/src/index.ts`](https://github.com/electron-userland/electron-builder/blob/master/packages/electron-updater/src/index.ts),
  [changeset](https://github.com/electron-userland/electron-builder/blob/master/.changeset/nsis-web-package-type-marker.md).
- Firefox: [`distribution.sys.mjs`](https://github.com/mozilla-firefox/firefox/blob/main/browser/components/distribution.sys.mjs),
  [`nsXREDirProvider.cpp`](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/xre/nsXREDirProvider.cpp),
  [`UpdateUtils.sys.mjs`](https://github.com/mozilla-firefox/firefox/blob/main/toolkit/modules/UpdateUtils.sys.mjs),
  [`channel-prefs.js`](https://github.com/mozilla-firefox/firefox/blob/main/browser/app/profile/channel-prefs.js),
  [`package-manifest.in`](https://github.com/mozilla-firefox/firefox/blob/main/browser/installer/package-manifest.in),
  [`WidgetUtilsGtk.cpp`](https://github.com/mozilla-firefox/firefox/blob/main/widget/gtk/WidgetUtilsGtk.cpp).
- Zed: [`release_channel/src/lib.rs`](https://github.com/zed-industries/zed/blob/main/crates/release_channel/src/lib.rs),
  [`auto_update/src/auto_update.rs`](https://github.com/zed-industries/zed/blob/main/crates/auto_update/src/auto_update.rs).
- Chromium: [`product_install_details.cc`](https://github.com/chromium/chromium/blob/main/chrome/install_static/product_install_details.cc),
  [`google_chrome_install_modes.h`](https://github.com/chromium/chromium/blob/main/chrome/install_static/google_chrome_install_modes.h),
  [Linux `wrapper`](https://github.com/chromium/chromium/blob/main/chrome/installer/linux/common/wrapper),
  [`channel_info_posix.cc`](https://github.com/chromium/chromium/blob/main/chrome/common/channel_info_posix.cc).
- GitHub Desktop: [`update-store.ts`](https://github.com/desktop/desktop/blob/development/app/src/ui/lib/update-store.ts).
- Windows Installer and WiX: [File](https://docs.firegiant.com/wix/schema/wxs/file/),
  [IniFile](https://docs.firegiant.com/wix/schema/wxs/inifile/),
  [RegistryValue](https://docs.firegiant.com/wix/schema/wxs/registryvalue/),
  [IniFile table](https://learn.microsoft.com/en-us/windows/win32/msi/inifile-table),
  [Formatted](https://learn.microsoft.com/en-us/windows/win32/msi/formatted),
  [Command-line options](https://learn.microsoft.com/en-us/windows/win32/msi/command-line-options),
  [MsiEnumRelatedProducts](https://learn.microsoft.com/en-us/windows/win32/api/msi/nf-msi-msienumrelatedproductsw),
  [MsiGetProductInfo](https://learn.microsoft.com/en-us/windows/win32/api/msi/nf-msi-msigetproductinfow),
  [KNOWNFOLDERID](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid).
- winget and Chocolatey: [installer manifest schema 1.12](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.12.0/installer.md),
  [winget list](https://learn.microsoft.com/en-us/windows/package-manager/winget/list),
  [Install-ChocolateyPackage](https://docs.chocolatey.org/en-us/create/functions/install-chocolateypackage/),
  [Chocolatey FAQ](https://docs.chocolatey.org/en-us/faqs/).
- Apple: [About Info.plist files](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/AboutInformationPropertyListFiles.html),
  [Information Property List](https://developer.apple.com/documentation/bundleresources/information-property-list),
  [TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html),
  [Gatekeeper](https://support.apple.com/guide/security/gatekeeper-and-runtime-protection-sec5599b66df/web),
  [App Translocation on the forums](https://developer.apple.com/forums/thread/724969).
- Snap and Flatpak: [snapd `snapenv.go`](https://github.com/canonical/snapd/blob/master/snap/snapenv/snapenv.go),
  [flatpak `flatpak-run.c`](https://github.com/flatpak/flatpak/blob/main/common/flatpak-run.c).
- Rust and Cargo: [`std::env::current_exe`](https://doc.rust-lang.org/std/env/fn.current_exe.html),
  [`cargo install`](https://doc.rust-lang.org/cargo/commands/cargo-install.html).
