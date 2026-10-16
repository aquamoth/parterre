# GitHub builds on top of winget, Chocolatey and MSI upgrades

Research note for [#393](https://github.com/aquamoth/parterre/issues/393), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390) (nightly and canary builds, and rings that
installations keep). The question: *if an installation from winget or Chocolatey installs a
nightly or rc MSI from GitHub, what happens, and what do a later `winget upgrade` and
`choco upgrade` do? How does Windows Installer order a nightly against a stable release of the
same numbers? The same for .deb, .rpm and the disk image. Which channels can safely take
non-stable builds from GitHub?*

Sources are official docs, source code at pinned commits, parterre's own packaging, and the
v1.0.0 MSI as published. A statement that is my own conclusion is marked **(derived)**; one I
could not check is marked **(unverified)**; one I checked on this machine is marked
**(tested 2026-10-16)** (Windows 11 Pro 26200, winget 1.29.380, Chocolatey 2.7.4, parterre 1.0.0
installed per user from the GitHub MSI).

The scheme that numbers nightly and canary builds is
[#397](https://github.com/aquamoth/parterre/issues/397)'s to decide. This note assumes the one
today's dev builds already follow: a build past tag `vX.Y.Z` is a pre-release of `X.Y.(Z+1)`
(`docs/releasing.md` › Version strings), so its MSI version is `X.Y.(Z+1)`. §6 says what changes
if #397 picks something else.

## TL;DR

| Channel | Take non-stable builds from GitHub? | Why |
|---|---|---|
| MSI from GitHub, per user | **Safe** | Same UpgradeCode and same context: every GitHub MSI major-upgrades the one before. Same-numbered builds replace each other in both directions; nothing reverts them behind the user's back. |
| winget, user scope | **Safe, with one condition** | The GitHub MSI upgrades it in place (same per-user context). winget keeps tracking it (name + publisher, UpgradeCode) and reads the nightly's numeric MSI version as installed. `winget upgrade` only replaces it with a stable release whose numbers are higher, which is a genuinely newer build. Condition: the winget manifest's install really is per user (see §3.3). |
| winget, machine scope | **Unsafe** | A GitHub MSI opened from the browser installs per user, and Windows Installer never upgrades across contexts: the result is a second, side-by-side install. |
| Chocolatey | **Unsafe: stay stable** | Chocolatey always installs machine-wide (`ALLUSERS=1`), so a GitHub MSI lands beside it, not over it. If the user forces a machine-wide install, open-source Chocolatey never notices; a later `choco upgrade` either silently swaps the nightly for the stable, or fails on every run with "A newer version of parterre is already installed" until a stable release overtakes the nightly's numbers. |
| .deb / .rpm from GitHub | **Safe** | No repository, so `apt upgrade` / `dnf upgrade` never touch it. `dpkg -i` installs any version over any other; `rpm -U` and `dnf` install newer ones and downgrade on request. `~` sorts a pre-release before its release in both. |
| macOS disk image | **Safe** | Dragging the app to Applications replaces the bundle. No installer database, no version check. |
| zip / tarball | **Safe** | Files the user unpacks; nothing tracks them. |

**Derived for #398:** the hazard on Windows is exactly the **machine-wide** MSI install
(Chocolatey always, winget with `--scope machine`, or an admin's `msiexec ALLUSERS=1`). parterre
can tell that cheaply at run time: its executable is under Program Files rather than
`%LOCALAPPDATA%\Programs`. Today, even a *stable* *Download* on a Chocolatey install leads to a
second, per-user install (§4.1).

## 1. parterre's MSI as built

From `packaging/windows/parterre.wxs` and `build-msi.ps1`, and the v1.0.0 MSI decompiled with
`wix msi decompile -data` **(tested 2026-10-16)**:

- **UpgradeCode** `{B139A92E-B931-4A5C-AFEF-2E75C53473C8}` on every build; a **new ProductCode per
  build** (WiX's default: "a new ProductCode property will be generated with every build enabling
  major upgrades", [WiX Package](https://docs.firegiant.com/wix/schema/wxs/package/)). v1.0.0's is
  `{BC1C8A5F-E588-406C-9BBE-E8A41C51DCD8}`.
- **ProductVersion** is `X.Y.Z` only: `build-msi.ps1` strips the pre-release part, so
  `1.0.0-rc1` and `1.0.0` are both `1.0.0`.
- **Scope** `perUserOrMachine`: per user by default, machine-wide with `ALLUSERS=1`, which the
  Chocolatey package passes (`packaging/chocolatey/tools/chocolateyinstall.ps1`).
- **`MajorUpgrade AllowSameVersionUpgrades="yes"`**, with the downgrade message "A newer version of
  parterre is already installed." In the built MSI that is:

  | Upgrade row | VersionMin | VersionMax | Attributes | ActionProperty |
  |---|---|---|---|---|
  | remove | (null) | 1.0.0 | 513 = VersionMaxInclusive + MigrateFeatures | `WIX_UPGRADE_DETECTED` |
  | detect | 1.0.0 | (null) | 2 = OnlyDetect (min *exclusive*) | `WIX_DOWNGRADE_DETECTED` |

  plus the launch condition `NOT WIX_DOWNGRADE_DETECTED`. So it removes every related product
  with a version **≤** its own and refuses to install over one **>** its own.
  `RemoveExistingProducts` runs right after `InstallValidate` (WiX's default schedule: the old
  product is removed entirely before the new one is installed,
  [WiX MajorUpgrade](https://docs.firegiant.com/wix/schema/wxs/majorupgrade/)).

## 2. How Windows Installer orders builds

- **Three fields, numbers only.** `major.minor.build`, at most 255.255.65535; "Windows Installer
  uses only the first three fields of the product version. If you include a fourth field in your
  product version, the installer ignores the fourth field."
  ([ProductVersion](https://learn.microsoft.com/en-us/windows/win32/msi/productversion),
  [Major Upgrades](https://learn.microsoft.com/en-us/windows/win32/msi/major-upgrades))
- **A nightly and a stable release with the same three numbers are the same version** to Windows
  Installer. With `AllowSameVersionUpgrades`, each replaces the other, in either direction: WiX
  documents that the attribute "also allows downgrades when the first three product version fields
  are identical" ([WiX MajorUpgrade](https://docs.firegiant.com/wix/schema/wxs/majorupgrade/)).
  Without it, two builds of one version would be "two products" installed side by side. That is
  why parterre sets it, and ICE61's warning against removing "a newer or equal version" is
  suppressed ([ICE61](https://learn.microsoft.com/en-us/windows/win32/msi/ice61)).
- **So nightly builds of the same `X.Y.Z` upgrade each other whatever their order**, including an
  older nightly over a newer one. Installing a lower `X.Y.Z` over a higher one stops at the launch
  condition: "A newer version of parterre is already installed." **(derived from §1)**
- **A fourth field changes nothing for Windows Installer**, but it does show in Apps and in
  winget's comparison (§3.2), so it could carry a nightly's number for display. **(derived)**
- **Never across contexts.** "If an application is installed in the per-user installation context,
  any major upgrade to the application must also be performed using the per-user context. […] The
  Windows Installer will not install major upgrades across installation context."
  ([Major Upgrades](https://learn.microsoft.com/en-us/windows/win32/msi/major-upgrades)) A
  per-user GitHub MSI therefore neither removes nor detects a machine-wide parterre: it installs a
  second one. The same holds the other way round. **(derived)**

## 3. winget

### 3.1 It keeps track of a parterre it didn't install

winget's view of installed software is the Apps list (ARP), correlated with packages by
ProductCode, UpgradeCode, package family name, and normalized name + publisher
(`CompositeSource.cpp`, `AddSystemReferenceStringsFromManifest`, winget-cli at
[`3d9083c`](https://github.com/microsoft/winget-cli/tree/3d9083c062c1e7a7086e05becb414d4e005e4ab6)).
winget reads MSI UpgradeCodes from `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UpgradeCodes`
(`ARPHelper.cpp`, `GetUpgradeCodes`), and a per-user install is registered there too **(tested
2026-10-16: the per-user parterre 1.0.0's ProductCode is under that key and under
`HKCU\Software\Microsoft\Installer\UpgradeCodes`)**. The manifest schema's `AppsAndFeaturesEntries`
`UpgradeCode` exists "to help correlate installed packages with manifests"
([installer manifest 1.10](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.10.0/installer.md)).

So after a GitHub nightly replaces a winget install, its new ProductCode is in no manifest, but the
UpgradeCode (if the manifest lists it) and the name "parterre" with publisher "Trustfall AB"
still tie it to `Trustfall.Parterre`. **(derived)** The plan in #16 already matches the publisher to
the MSI's ARP entry.

### 3.2 What `winget upgrade` does with it

- **Installed version** is the ARP `DisplayVersion` (`ARPHelper::DetermineVersion`), which for an
  MSI is its ProductVersion: numbers only. v1.0.0 shows `DisplayVersion 1.0.0` **(tested)**.
- **Comparison** splits on `.` and compares part by part, numerically, padding the shorter with
  zeros; a part with trailing text (`1-rc1`) sorts before the bare number (`Versions.cpp`,
  `Version::operator<`, `Part::operator<`). A four-part `1.0.1.7` sorts after `1.0.1`.
- **`winget upgrade`** offers a package only when an available version is higher than the installed
  one ([upgrade](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)); it has
  no downgrade switch. Pins (`winget pin`) can hold a package back from `--all`.

Hence, with nightly MSI version `X.Y.(Z+1)` after tag `vX.Y.Z` **(derived)**:

| Installed (from GitHub) | Newest stable in winget | `winget upgrade` |
|---|---|---|
| nightly after v1.0.0 → MSI 1.0.1 | 1.0.0 | nothing (installed is newer) |
| nightly after v1.0.0 → MSI 1.0.1 | 1.0.1 | nothing (equal); parterre's own check, in semver, still offers 1.0.1 > 1.0.1-nightly |
| nightly after v1.0.0 → MSI 1.0.1 | 1.1.0 | replaces it with 1.1.0, which is newer than that nightly |

Every case is safe: winget never downgrades a nightly, never loses it, and only replaces it with a
stable release built later. The ring is stored with the installation, not in the MSI, so it stays
nightly and the next nightly is offered as usual (per #390's decisions).

### 3.3 Scope is the catch

- winget's default "scope of package installs is for the current user", falling back to a machine
  installer if there is no user one
  ([Settings.md](https://github.com/microsoft/winget-cli/blob/master/doc/Settings.md)); upgrades
  carry the installed scope into installer selection (`PackageVersionSelection.cpp`,
  `CurrentlyInstalledScope`).
- **Machine scope** (`--scope machine`, or a manifest offering only a machine installer) gives the
  Chocolatey case of §4.1: a GitHub MSI installs beside it.
- **Observed, worth a test in #16:** this machine's per-user parterre (Windows Installer context
  `UserUnmanaged`) has its Uninstall key under **HKLM**, not HKCU, and `winget list` shows it as
  `ARP\Machine\X64\{BC1C8A5F-…}` **(tested 2026-10-16)**. If winget takes that as the installed
  scope, a manifest with both a user and a machine MSI installer might upgrade a per-user install
  with the machine one, which would install a second copy. **(unverified)** A manifest with a single
  MSI installer and no `Scope` avoids the question.

## 4. Chocolatey

### 4.1 Machine-wide, so a GitHub MSI lands beside it

The package always installs machine-wide (`ALLUSERS=1`). A GitHub MSI opened from the browser has
no `ALLUSERS`, so it installs per user (`release.yml` tests it that way, "its default, as winget
does"). Across contexts there is no major upgrade (§2): the user ends up with two parterre
installs **(derived)**:

- two *parterre* entries in Settings › Apps and two in the Start menu;
- `parterre` in a terminal runs the **machine-wide** one, because the system PATH comes before the
  user's **(tested: this machine's PATH lists system entries first)**;
- *Revision Graph* in Explorer runs the **per-user** one, because `HKCU\Software\Classes` wins over
  `HKLM\Software\Classes` in the merged view
  ([Merged View of HKEY_CLASSES_ROOT](https://learn.microsoft.com/en-us/windows/win32/sysinfo/merged-view-of-hkey-classes-root));
- `choco upgrade` keeps upgrading only the machine-wide one.

This is already true today for a **stable** *Download* on a Chocolatey install: the update check
opens the MSI for every MSI channel (`docs/distribution.md` › Update check).

### 4.2 If the GitHub MSI is installed machine-wide

(An admin's `msiexec /i … ALLUSERS=1`.) The MSI major-upgrades the Chocolatey-installed product,
and Chocolatey doesn't notice:

- Open-source Chocolatey keeps its own record in `lib\parterre` and has no sync: software changed
  outside it leaves "the package installed"; even the licensed editions' Automatic Sync records an
  outside upgrade but "doesn't change the version of the package"
  ([Automatic Sync](https://docs.chocolatey.org/en-us/features/package-synchronization/automatic-sync/)).
  `choco upgrade` compares that record with the feed and has `--allow-downgrade` off by default
  ([upgrade](https://docs.chocolatey.org/en-us/choco/commands/upgrade/)).
- When a newer stable package is pushed, `choco upgrade` runs its MSI machine-wide **(derived from
  §1–§2)**:
  - stable numbers **higher** than the nightly's: the nightly is replaced. Fine.
  - **equal**: the stable MSI replaces the nightly (same-version upgrade), which may be an older
    build than the nightly. Silent.
  - **lower** (only if a nightly ever carries higher numbers than the next stable): the launch
    condition fails, `msiexec` returns an error outside the package's `validExitCodes` (0, 3010,
    1641), and every `choco upgrade` fails until a stable release overtakes the nightly.
- `choco uninstall parterre` removes whichever machine-wide parterre is there: its script looks up
  the HKLM entry named *parterre* (`chocolateyuninstall.ps1`).

So Chocolatey can never take a GitHub build cleanly: per user it duplicates, machine-wide it
drifts from Chocolatey's record. It must stay stable, and its update check should point at
`choco upgrade parterre` rather than at the MSI. **(derived)**

## 5. Linux packages and the disk image

### 5.1 .deb and .rpm (GitHub only)

- **Nothing reverts them.** They come from GitHub, not a repository, so `apt upgrade` and
  `dnf upgrade` have no candidate for them (DNF upgrades to "the latest version that is both
  available and resolvable", available meaning in a repository,
  [DNF command reference](https://dnf.readthedocs.io/en/latest/command_ref.html)). This changes
  only if parterre gets an apt/dnf repository of its own (`docs/distribution.md`, "later").
- **dpkg installs any version over any other.** Downgrade is among the `--force` things "forced by
  default": "Install a package, even if newer version of it is already installed", with a warning
  ([dpkg(1)](https://manpages.debian.org/bookworm/dpkg/dpkg.1.en.html)).
- **rpm upgrades, and downgrades on request.** `rpm -U` installs "to a newer version";
  `--oldpackage` allows "an upgrade to replace a newer package with an older one"
  ([rpm(8)](https://rpm-software-management.github.io/rpm/man/rpm.8)). DNF given a specific
  version "will install the desired version, no matter which version of the package is already
  installed" ([DNF](https://dnf.readthedocs.io/en/latest/command_ref.html)).
- **Ordering.** `build-packages.sh` writes a pre-release's `-` as `~`. In dpkg "a tilde sorts
  before anything, even the end of a part"
  ([deb-version(7)](https://manpages.debian.org/bookworm/dpkg-dev/deb-version.7.en.html)); in rpm
  "the tilde operator causes a segment to sort older, and is used for pre-release versions"
  ([rpm-version(7)](https://rpm-software-management.github.io/rpm/man/rpm-version.7)). So
  `1.0.1~nightly.20261016 < 1.0.1~rc1 < 1.0.1`: both put every pre-release before its release,
  and between pre-releases the words decide (`nightly` before `rc`, as `n` < `r`), which is
  #397's to choose. Digit runs compare numerically in both, so a date or counter after
  `nightly.` orders nightlies.
  rpm also has `^` for post-release snapshots (`2.0^150825` sorts after `2.0`, before `2.0.1`),
  added in rpm 4.15 and backported to 4.14.3 ([RPM 4.15 change](https://fedoraproject.org/wiki/Changes/RPM-4.15));
  whether openSUSE Leap 15.6's rpm has it is **(unverified)**, and dpkg has no `^` (its `+` sorts
  after the end of a part).

### 5.2 Disk image

The disk image holds `parterre.app`, which the user drags to Applications, replacing the bundle.
No package database records it and nothing checks its version; `CFBundleVersion` is numbers only,
like the MSI (`build-dmg.sh`), but nothing compares it on install. **(derived)** Safe.

## 6. If #397 numbers nightly builds differently

The Windows results depend on the nightly's three MSI numbers against the next stable's **(derived)**:

- **Next patch of the nearest tag** (`X.Y.(Z+1)`, today's dev-build rule): never above the next
  stable, never below the last one. Everything in §3–§4 holds.
- **The same numbers as the last tag** (`X.Y.Z`): equal to the stable it follows, so a same-version
  upgrade either way. winget and Chocolatey would offer nothing until the next stable; harmless.
- **Anything that can exceed the next stable** (e.g. next minor `X.(Y+1).0` while a patch
  `X.Y.(Z+1)` ships): winget silently offers nothing; Chocolatey machine-wide (§4.2) fails on every
  upgrade. Avoid.
- **A date or run number** can't go in the third field (max 65,535). In a fourth field it is
  ignored by Windows Installer and shown by Apps and winget.

## Sources

- Windows Installer: [ProductVersion](https://learn.microsoft.com/en-us/windows/win32/msi/productversion),
  [Major Upgrades](https://learn.microsoft.com/en-us/windows/win32/msi/major-upgrades),
  [Upgrade table](https://learn.microsoft.com/en-us/windows/win32/msi/upgrade-table),
  [FindRelatedProducts](https://learn.microsoft.com/en-us/windows/win32/msi/findrelatedproducts-action),
  [ICE61](https://learn.microsoft.com/en-us/windows/win32/msi/ice61),
  [Merged View of HKEY_CLASSES_ROOT](https://learn.microsoft.com/en-us/windows/win32/sysinfo/merged-view-of-hkey-classes-root).
- WiX: [MajorUpgrade](https://docs.firegiant.com/wix/schema/wxs/majorupgrade/),
  [Package](https://docs.firegiant.com/wix/schema/wxs/package/).
- winget: [upgrade](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade),
  [Settings.md](https://github.com/microsoft/winget-cli/blob/master/doc/Settings.md),
  [installer manifest 1.10](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.10.0/installer.md),
  winget-cli source at [`3d9083c`](https://github.com/microsoft/winget-cli/tree/3d9083c062c1e7a7086e05becb414d4e005e4ab6):
  `src/AppInstallerSharedLib/Versions.cpp`, `src/AppInstallerRepositoryCore/CompositeSource.cpp`,
  `src/AppInstallerRepositoryCore/Microsoft/ARPHelper.cpp`,
  `src/AppInstallerRepositoryCore/PackageVersionSelection.cpp`.
- Chocolatey: [upgrade](https://docs.chocolatey.org/en-us/choco/commands/upgrade/),
  [Automatic Sync](https://docs.chocolatey.org/en-us/features/package-synchronization/automatic-sync/).
- Debian: [deb-version(7)](https://manpages.debian.org/bookworm/dpkg-dev/deb-version.7.en.html),
  [dpkg(1)](https://manpages.debian.org/bookworm/dpkg/dpkg.1.en.html).
- RPM: [rpm(8)](https://rpm-software-management.github.io/rpm/man/rpm.8),
  [rpm-version(7)](https://rpm-software-management.github.io/rpm/man/rpm-version.7),
  [Fedora: RPM 4.15](https://fedoraproject.org/wiki/Changes/RPM-4.15),
  [DNF command reference](https://dnf.readthedocs.io/en/latest/command_ref.html).
- parterre: `packaging/windows/parterre.wxs`, `packaging/windows/build-msi.ps1`,
  `packaging/chocolatey/`, `packaging/linux/build-packages.sh`, `packaging/macos/build-dmg.sh`,
  `docs/releasing.md`, `docs/distribution.md`, and the v1.0.0 MSI from its GitHub Release.
