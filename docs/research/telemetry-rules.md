# Channel and legal rules on phoning home

Research note for [#220](https://github.com/aquamoth/parterre/issues/220), part of the map
[#175](https://github.com/aquamoth/parterre/issues/175): *what may a default-on update check
that also counts usage send, under parterre's distribution channels and under GDPR and
ePrivacy as they apply to Trustfall AB in Sweden, and how should the update notice behave on
channels that update parterre themselves?*

All sources were read on **2026-10-04** unless a date says otherwise. Sources are official
documents, statutes, judgments, regulators' guidance and source code at pinned commits. A
statement that is my own conclusion is marked **(derived)**; one I could not check is marked
**(unverified)**. This is a reading of the sources, not legal advice.

## TL;DR

- **No channel forbids a default-on update check or usage counter.** None has a written rule on
  runtime telemetry. What exists:
  - Flathub requires an OARS content rating, and its `social-info` attribute rates "a
    user-counter" as *mild*.
  - winget asks for a `PrivacyUrl` when an app transmits personal information. Under GDPR an IP
    address makes the ping personal information.
  - Debian and Fedora packagers routinely compile update checks out. Debian enforces opt-in
    for phoning home as unwritten practice (visidata, closed 2026-09-26).
- **Self-updating channels:** apps detect Snap (`SNAP*`) and Flatpak (`FLATPAK_ID`,
  `/.flatpak-info`) at run time. Distro packages, MSI and winget give no runtime signal, so
  upstreams offer a build switch (a cargo feature) or a packager-supplied message. parterre's own
  `.deb`/`.rpm` from GitHub Releases are *not* updated by apt or dnf (there is no repository),
  so the notice is useful there.
- **GDPR:** a request carrying version, OS and architecture is personal data at the receiving
  server because of the IP address (Breyer, C-582/14). Identifiability is judged from the
  controller's side at collection (SRB, C-413/23 P). A random install ID is an online identifier
  (Recital 30). Legitimate interest is the basis the closest EU comparable uses (Audacity,
  Cyprus). JetBrains (Czechia) uses contract and sends a permanent install ID by default.
- **ePrivacy / LEK 9 kap. 28 §:** the EDPB reads software that "proactively call[s] an API
  endpoint" as *gaining access* to the terminal (Guidelines 2/2023 para 33). So the ping needs
  consent unless an exemption applies.
  - The update check can be argued to be "strictly necessary" for a service the user requested.
    No authority has ruled on update checks.
  - Usage counting has no exemption in today's law. WP29 says each purpose must qualify on its
    own, and first-party analytics does not. The EDPB and EDPS confirmed in February 2026: "No
    such exceptions are provided under Article 5(3)". The pending Digital Omnibus would add one
    for aggregated audience measurement.
  - Storing and sending a random install ID is storage *and* access, for a counting purpose:
    consent under the strict reading.
- **Plain summary (§5):** by default, an update check with version, OS, architecture and
  channel, no ID, the IP discarded or truncated, disclosed in a privacy notice, with a switch
  in Settings. Counting from those same requests is common practice and low-risk under GDPR. It
  is not covered by an ePrivacy exemption today. An install ID, crash reports and feature events
  need opt-in under the strict reading.

---

## 1. Channel rules

### 1.1 Summary

| Channel | Written rule on runtime telemetry or update checks | What it actually requires or does | Practice |
|---|---|---|---|
| Flathub | none | OARS rating is mandatory; `social-info` *mild* = "a user-counter"; `--share=network` shown as "Low Risk" | KeePassXC and GIMP builds turn update checks off; VS Code declares `social-info` *moderate* |
| Debian | none (build-time network only) | Lintian `privacy-breach-*` scans web files only, not binaries | update checks compiled out (syncthing, keepassxc, calibre, firefox-esr); visidata's phone-home patched out 2026-09-26 |
| Fedora | none (build-time network only) | – | same packages compiled out; Fedora's own `countme` is default-on, with no ID |
| Snap Store | none | network auto-connects with no review; ToS bans privacy violations and surreptitious collection | – |
| winget | none on telemetry | `PrivacyUrl` "should" be given if the app transmits Personal Information; opt-in for publishing it to "an outside service or third party" | `RequireExplicitUpgrade` exists for self-updaters |
| Chocolatey | none | moderation checks install scripts for malice only | community `*-disableautoupdate` companion packages |
| crates.io | none | bans "spyware" and obfuscated functionality | docs.rs builds have no network (build time only) |

### 1.2 Flathub

- **Requirements page** ([requirements](https://docs.flathub.org/docs/for-app-authors/requirements)):
  - It has no rule on telemetry, analytics, update checks or self-updaters.
  - At build time: "There is no network access during the build process."
  - At run time: "Static permissions must be kept to an absolute minimum."
  - Closest general clause: apps with "insecure or harmful design choices, such as … exposing or
    accessing sensitive information … will not be accepted".
- **Self-updating apps:**
  - Third-party PRs cite "Flathub's policy against in-app update checkers"
    ([fontra-pak#277](https://github.com/fontra/fontra-pak/pull/277),
    [fontra-flatpak#38](https://github.com/fontra/fontra-flatpak/issues/38)) but link no Flathub
    source, so that policy is **(unverified)**.
  - A Flathub moderator wrote: "Ideally shipping is done via flatpak, but there are apps like
    discord, that update parts" ([forum](https://discourse.flathub.org/t/application-update/10191)).
- **Content rating is mandatory:**
  - "Applications must be properly tagged by OARS data", `type="oars-1.1"`
    ([MetaInfo guidelines](https://docs.flathub.org/docs/for-app-authors/metainfo-guidelines)).
  - The linter makes `content-rating-missing` an error
    ([linter](https://docs.flathub.org/docs/for-app-authors/linter)).
- **OARS `social-info`** ([OARS generator](https://hughsie.github.io/oars/generate.html)) is
  "sharing information with a legal entity typically used for advertising or for sending back
  diagnostic data". Its levels:
  - *mild*: "Using any online API, e.g. a user-counter"
  - *moderate*: "Sharing diagnostic data not identifiable to the user, e.g. profiling data"
  - *intense*: "Sharing information identifiable to the user, e.g. crash dumps"

  **(derived)** An update check that counts is *mild*. Opt-in crash reports with stack traces
  would push the declared level to *intense* if they ship in the Flathub build.
- **Network permission on the website:** "Network access" / "Has network access" is rated Low
  Risk ([safety.ts](https://github.com/flathub-infra/website/blob/main/frontend/src/safety.ts)).
  parterre already needs the network for GitHub pull requests.
- **AppStream has no privacy-policy URL type**
  ([AppStream metadata](https://www.freedesktop.org/software/appstream/docs/chap-Metadata.html)).
- **Real Flathub manifests:**
  - KeePassXC builds with `-DWITH_XC_UPDATECHECK=OFF`
    ([manifest](https://github.com/flathub/org.keepassxc.KeePassXC)).
  - GIMP builds with `-Dcheck-update=no` ([manifest](https://github.com/flathub/org.gimp.GIMP)).
  - VS Code declares `social-info` *moderate*
    ([metainfo](https://github.com/flathub/com.visualstudio.code)).

#### Side finding: Flathub's Generative AI policy (affects #21)

The same [requirements page](https://docs.flathub.org/docs/for-app-authors/requirements) has a
"Generative AI policy". It is not about telemetry, but it governs the planned Flathub submission
([#21](https://github.com/aquamoth/parterre/issues/21)) and its automation
([#20](https://github.com/aquamoth/parterre/issues/20)). Quoted in full where it binds:

- "Submitters must disclose any AI-generated code, documentation, packaging, or other material
  they know or reasonably believe is included in the application or its Flathub packaging. The
  disclosure must identify the affected parts and approximate extent."
- "Flathub manifests must not contain AI-generated or AI-assisted content. Disclosure does not
  exempt manifests from this restriction."
- "Other disclosed AI-generated material is evaluated at reviewer discretion. Reviewers may
  reject a submission, including without further review, based on the extent or role of
  generated material … Disclosure does not create a presumption of acceptance."
- "AI tools or agents must not open or automate Flathub submission pull requests, or generate
  their commit messages, descriptions, review comments, or replies. Submitters must not request
  AI-agent reviews."
- "Undisclosed or materially misrepresented AI-generated material … may result in rejection.
  Repeated violations may result in a permanent ban from future submissions and activities."

**(derived)** For parterre:
- Many commits carry `Co-Authored-By: Claude`, so the submission must disclose AI-generated code
  and its extent, and acceptance is at the reviewer's discretion.
- The Flathub manifest (`se.trustfall.parterre.yml` and anything else in the flathub repo) must
  be written by a human without AI assistance.
- The submission PR, its commit messages and replies to reviewers must be written by a human,
  not an agent. The release automation in #20 must not use an AI agent to open Flathub PRs.

### 1.3 Debian

- **Policy 4.7.4.1 §4.9** covers build time only: "required targets must not attempt network
  access to other hosts" ([policy.txt](https://www.debian.org/doc/debian-policy/policy.txt)).
  The [Social Contract](https://www.debian.org/social_contract) and the
  [Developer's Reference](https://www.debian.org/doc/manuals/developers-reference/) say nothing
  about runtime phoning home.
- **Lintian** `privacy-breach-generic` is a warning for "fetching data from an external website
  at runtime"
  ([tag](https://salsa.debian.org/lintian/lintian/-/blob/master/tags/p/privacy-breach-generic.tag)).
  It scans only `\.(?:x?html?\d?|js|xht|xml|css)$` files
  ([PrivacyBreach.pm](https://salsa.debian.org/lintian/lintian/-/blob/master/lib/Lintian/Check/Files/PrivacyBreach.pm)),
  so a Rust binary is never flagged.
- **The visidata precedent, [#1001647](https://bugs.debian.org/1001647):** visidata fetched a
  daily startup message that its author used to count users. Christoph Berg (Debian Developer)
  wrote:
  - 2025-03-01: "There is no written policy for this yet, but every other package I know with
    such a feature has turned it off in the packaging."
  - 2025-03-03: "The GDPR mandates privacy by default, so opt-in unless you have good reasons
    otherwise," and "whatever the GDPR says, Debian wants no software to call home unless it's
    for really good reasons."

  The bug was closed in visidata 3.4-1 on 2026-09-26: "Add patch to disable default motd_url to
  avoid network access on startup."

  **(derived)** "Privacy by default" (GDPR Art. 25(2), §3.6) requires processing only the data
  necessary for each purpose by default. It does not itself require opt-in. That is Debian
  practice, not law.
- **popularity-contest** is opt-in: `Default: false`
  ([templates](https://sources.debian.org/src/popularity-contest/1.79/debian/templates)).
- **Packages that compile update checks out:**
  - syncthing: `-tags 'noupgrade purego'`
    ([rules](https://sources.debian.org/src/syncthing/1.29.5~ds1-5/debian/rules))
  - keepassxc: `-DWITH_XC_UPDATECHECK=OFF`
    ([rules](https://sources.debian.org/src/keepassxc/2.7.10+dfsg1-2.1/debian/rules))
  - calibre: "allow for plugin update check, but no calibre version check"
    ([patch](https://sources.debian.org/src/calibre/9.15.0+ds+~1.1.2-3/debian/patches/0001-only-plugin-update.patch))
  - firefox-esr: `--disable-updater`
    ([debian/](https://sources.debian.org/src/firefox-esr/153.4.0esr-1/debian/))

### 1.4 Fedora

- **Packaging Guidelines** cover only "Build Time Network Access"
  ([guidelines](https://docs.fedoraproject.org/en-US/packaging-guidelines/)). Nothing on runtime
  telemetry or update checks.
- **In practice**, the keepassxc, syncthing ("noupgrade: disable syncthing self-update
  functionality") and calibre ("Disable auto update from inside the app") specs all compile the
  check out (`https://src.fedoraproject.org/rpms/<name>`).
- **The withdrawn telemetry proposal** ([Changes/Telemetry](https://fedoraproject.org/wiki/Changes/Telemetry))
  records:
  - "Fedora Legal has determined that if we collect any personally-identifiable data, the
    entire metrics system must be opt-in."
  - "A very large number of users complained" about a default-on toggle.

  Its opt-in successor ([Changes/Metrics](https://fedoraproject.org/wiki/Changes/Metrics)) was
  dropped.
- **Fedora's own counter, `countme`, is default-on and carries no ID** (a design reference):
  - dnf adds `countme=N` to one metalink request per week, where N is an age bucket (1: first
    week, 2: up to a month, 3: up to six months, 4: older). The install epoch comes from
    `machine-id`'s modification time rather than an identifier
    ([dnf conf_ref](https://dnf.readthedocs.io/en/latest/conf_ref.html)). dnf's default is off.
  - Fedora's repo file turns it on: `countme=1`
    ([fedora.repo](https://src.fedoraproject.org/rpms/fedora-repos/raw/rawhide/f/fedora.repo)).
  - The design rule: "we don't want to use any identifier like /etc/machine-id … or in fact any
    UUID at all" ([DNF Better Counting](https://fedoraproject.org/wiki/Changes/DNF_Better_Counting)).

### 1.5 Snap Store

- The network interface is "Auto-connect: yes" and needs "no additional store review"
  ([network interface](https://snapcraft.io/docs/reference/interfaces/network-interface/)).
- The [Snap Store terms](https://canonical.com/legal/terms-and-policies/snap-store-terms) forbid
  content that "violates the privacy … rights of any third party" (5.6) and "routines intended to
  … surreptitiously intercept or expropriate any system, data or information" (5.7).
- No rule on telemetry or self-updaters, and no privacy-policy field in snap metadata
  **(unverified beyond these pages)**.

### 1.6 winget

- The [winget-pkgs policies](https://github.com/microsoft/winget-pkgs/blob/master/doc/Policies.md)
  say nothing on telemetry.
- The [Windows Package Manager Policies v1.0](https://learn.microsoft.com/en-us/windows/package-manager/package/windows-package-manager-policies)
  (2021-05-22) say:
  - 1.5: "Personal Information includes all information or data that identifies or could be used
    to identify a person, or that is associated with such information or data."
  - 1.5.1: "If the Product accesses, collects or transmits Personal Information, or if otherwise
    required by law, it should maintain a privacy policy. The submission, should include the
    PrivacyUrl".
  - 1.5.2: publishing customers' Personal Information "to an outside service or third party"
    requires opt-in consent given in the product's UI, with a way to rescind it there.
  - 1.2.3: "The Product may contain fully integrated middleware (such as … third-party analytics
    services)."

  **(derived)** An update check sends an IP address, which under GDPR is personal data (§3.2),
  so the winget manifest should carry a `PrivacyUrl`. Whether Trustfall's own server, or a
  hosted backend acting as its processor, is "an outside service or third party" under 1.5.2 is
  not defined.
- **Manifest fields** ([schema 1.28.0](https://github.com/microsoft/winget-pkgs/tree/master/doc/manifest/schema/1.28.0)):
  - `RequireExplicitUpgrade` "identifies packages that upgrade themselves".
  - `PrivacyUrl` is optional.

### 1.7 Chocolatey

- The [moderation docs](https://docs.chocolatey.org/en-us/community-repository/moderation/) and
  [validator rules](https://docs.chocolatey.org/en-us/community-repository/moderation/package-validator/rules/)
  have nothing on telemetry or updaters. Moderators check whether scripts "try to do anything
  malicious".
- The community maintains companion packages that switch an app's updater off, e.g.
  [visualstudiocode-disableautoupdate](https://community.chocolatey.org/packages/visualstudiocode-disableautoupdate).

### 1.8 crates.io

- The [usage policy](https://crates.io/policies) forbids "malicious code, such as … back doors,
  or spyware" and "obfuscation to hide or mask functionality". No telemetry rule.
- [docs.rs](https://docs.rs/about/builds) builds have "Network access blocked" (build time
  only).

### 1.9 Reference: Syncthing upstream

[Syncthing's security page](https://docs.syncthing.net/users/security.html):
- The upgrade check runs "at startup and then once every twelve hours" and is on by default. It
  "can be disabled only by compiling Syncthing with upgrades disabled", which Debian and Fedora
  do. Its requests "*do not* contain any identifiable information about the user or device."
- "Usage reporting defaults to off but the GUI will ask once about enabling it."

---

## 2. Self-updating channels and channel detection

### 2.1 How apps learn their channel

| Signal | Channel | Source |
|---|---|---|
| `SNAP`, `SNAP_NAME`, `SNAP_REVISION` env | Snap | [Ubuntu Core docs](https://documentation.ubuntu.com/core/explanation/security-and-sandboxing/) |
| `FLATPAK_ID` env; `/.flatpak-info` file | Flatpak | [flatpak-run(1)](https://man7.org/linux/man-pages/man1/flatpak-run.1.html), [flatpak-metadata(5)](https://man7.org/linux/man-pages/man5/flatpak-metadata.5.html) |
| `APPIMAGE` env | AppImage | [AppImage docs](https://docs.appimage.org/packaging-guide/environment-variables.html) |
| exe under `%ChocolateyInstall%\lib` | Chocolatey *portable* package | [Chocolatey FAQ](https://docs.chocolatey.org/en-us/faqs/) |
| ARP values `WinGetPackageIdentifier` | winget *portable* package only | [PortableARPEntry.cpp](https://github.com/microsoft/winget-cli/blob/39739564a4aaf1071b17d163ec332a08b9bcf05c/src/AppInstallerCommonCore/PortableARPEntry.cpp#L15-L30) |
| none | MSI (by hand, via winget or via Chocolatey), `.deb`, `.rpm`, tarball | – |

Where there is no runtime signal, upstreams use a **build-time switch or a marker written by the
packaging step**:
- Mozilla writes an `is-packaged-app` file in its own deb and rpm
  ([deb.py](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/python/mozbuild/mozbuild/repackaging/deb.py#L126-L127)).
- electron-builder writes `resources/package-type`
  ([FpmTarget.ts](https://github.com/electron-userland/electron-builder/blob/ec9135d0626879479ffa4235006f06b14375cc43/packages/app-builder-lib/src/targets/linux/FpmTarget.ts#L184-L185)).
- VS Code writes `target` into `product.json`
  ([gulpfile](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/build/gulpfile.vscode.win32.ts#L82)).
- Cargo features:
  - rustup `no-self-update`
    ([Cargo.toml](https://github.com/rust-lang/rustup/blob/b46db5ea94c1024c7cd2e7133a7f9b2ec1927478/Cargo.toml#L25-L26));
    Fedora and Arch build with it.
  - uv `self-update`
    ([lib.rs](https://github.com/astral-sh/uv/blob/46b84fd0bfec23b72f29e8e2185ba68a65052f48/crates/uv/src/lib.rs#L111-L131)).
  - topgrade builds its `.deb` without the feature because "we don't want the auto-update
    feature"
    ([workflow](https://github.com/topgrade-rs/topgrade/blob/710856320124b85228e42846d133b3fd1a3ab5ae/.github/workflows/create_release_assets.yml#L86-L111)).
- Syncthing's `noupgrade` build tag
  ([upgrade_unsupp.go](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/lib/upgrade/upgrade_unsupp.go#L7-L23)).
  Packagers are told "you almost certainly want to use `--no-upgrade`"
  ([building](https://docs.syncthing.net/dev/building.html)).
- KeePassXC's CMake option: "Include automatic update checks; disable for managed distributions"
  ([CMakeLists.txt](https://github.com/keepassxreboot/keepassxc/blob/9e0f57a4a4c6c629fa6d0a593acb7d089b1d95cd/CMakeLists.txt#L66)).

### 2.2 What they do with the notice

1. **Hide it.**
   - Firefox shows an empty `noUpdater` panel for packaged apps.
   - Its code explains the choice: "packaged apps may be getting updated by an administrator or
     they may not be … we err to the side of less confusion"
     ([AppUpdater.sys.mjs](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/mozapps/update/AppUpdater.sys.mjs#L417-L425)).
2. **Replace it with a message.**
   - IntelliJ shows "IDE updates are managed externally by {0}", where {0} is Snap, Flatpak,
     Homebrew or Toolbox
     ([ExternalUpdateManager.java](https://github.com/JetBrains/intellij-community/blob/51115ae660a4fd8d2a6209976d680c6b01c8580b/platform/platform-impl/src/com/intellij/openapi/updateSettings/impl/ExternalUpdateManager.java#L16-L50)).
   - Zed lets the packager supply the text through `ZED_UPDATE_EXPLANATION`, at compile time or
     at run time. Arch uses "Updates are handled by pacman"
     ([auto_update.rs](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/auto_update/src/auto_update.rs#L285-L322)).
   - uv says "To update uv, run `brew upgrade uv`".
3. **"Restart to finish" when the package manager has already updated the files.**
   - VS Code on Snap compares `SNAP_REVISION` with `$SNAP/../current`, with no network request
     ([updateService.snap.ts](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/update/electron-main/updateService.snap.ts#L161-L214)).
   - Signal on deb watches a file its `postinst` touches
     ([linux.main.ts](https://github.com/signalapp/Signal-Desktop/blob/832279c26138f2ea47c6bb7d6b9f7e0f80eb9e96/ts/updater/linux.main.ts#L69-L111)).
   - Flatpak's portal offers `CreateUpdateMonitor`, which emits `UpdateAvailable`
     ([portal XML](https://github.com/flatpak/flatpak/blob/acb9dc7959ad6eed865acb8fd1f2398095f54017/data/org.freedesktop.portal.Flatpak.xml#L418-L515)).
4. **Keep it, linking to the website.** VS Code on deb, rpm and tarball: "we don't currently
   detect the package type that was installed and the website download page is more useful"
   ([updateService.linux.ts](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/update/electron-main/updateService.linux.ts#L77-L87)).

The cost of not detecting the channel: the Sublime Merge snap shows notices for releases that
haven't reached the snap channel yet
([snapcrafters/sublime-merge#47](https://github.com/snapcrafters/sublime-merge/issues/47)).

### 2.3 What parterre would need per channel (derived)

| Channel | Updated by the channel? | Detect | Update notice |
|---|---|---|---|
| GitHub zip / tar.gz | no | default | show, link to the release |
| crates.io (`cargo install`) | no | exe under `$CARGO_HOME/bin` (path) | show, with `cargo install parterre` |
| MSI from GitHub | no | indistinguishable from winget or Chocolatey | show, link to the release (optionally also the winget and choco commands) |
| winget | yes, with `winget upgrade` | none for an MSI | as MSI |
| Chocolatey | yes, with `choco upgrade` | none if it wraps the MSI | as MSI |
| `.deb` / `.rpm` from GitHub | **no**: no apt or dnf repository (`docs/distribution.md`) | none at run time; a build-time cargo feature or marker | show, link to the new package |
| Debian or Fedora package by volunteers | yes | they will build without the check (practice in §1.3–1.4) | a cargo feature that compiles the check out, plus an optional packager message à la Zed |
| Snap | yes (snapd, 4×/day) | `SNAP_NAME` | hide, or "restart to update" when `SNAP_REVISION` ≠ `current` |
| Flathub | yes | `FLATPAK_ID` / `/.flatpak-info` | hide, or the portal's `UpdateMonitor` |

Two consequences for counting:
- If the usage count rides on the update check, installs whose packagers compile the check out
  are not counted.
- If the ping still runs on Snap and Flathub with the notice hidden, it counts those installs,
  and the channel can be one of its fields. On Flathub that is what `social-info` *mild*
  declares.

---

## 3. GDPR and ePrivacy

### 3.1 The texts

- **GDPR** ([Regulation (EU) 2016/679](https://eur-lex.europa.eu/eli/reg/2016/679/oj)):
  - Art. 4(1): an identifiable person is one who "can be identified, directly or indirectly, in
    particular by reference to an identifier such as … an online identifier".
  - Recital 26: "account should be taken of all the means reasonably likely to be used … either
    by the controller or by another person". Data protection does not apply to "anonymous
    information".
  - Recital 30: people "may be associated with online identifiers provided by their devices,
    applications, tools and protocols, such as internet protocol addresses, cookie identifiers or
    other identifiers … This may leave traces which, in particular when combined with unique
    identifiers and other information received by the servers, may be used to create profiles
    of the natural persons and identify them."
- **ePrivacy Directive Art. 5(3)**, as amended in 2009
  ([2002/58/EC consolidated](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:02002L0058-20091219)):
  - Storing information, or gaining access to information already stored, in a user's terminal
    equipment requires consent "having been provided with clear and comprehensive information".
  - "This shall not prevent any technical storage or access for the sole purpose of carrying out
    the transmission of a communication over an electronic communications network, or as
    strictly necessary in order for the provider of an information society service explicitly
    requested by the subscriber or user to provide the service."
- **Sweden, [LEK (2022:482)](https://lagen.nu/2022:482) 9 kap. 28 §**, which implements Art. 5(3):
  - "Uppgifter får lagras i eller hämtas från en abonnents eller användares terminalutrustning
    endast om abonnenten eller användaren får tillgång till information om ändamålet med
    behandlingen och samtycker till den. Trots att samtycke inte har lämnats är sådan lagring
    eller åtkomst tillåten som 1. behövs för överföring av ett elektroniskt meddelande via ett
    elektroniskt kommunikationsnät, eller 2. är nödvändig för tillhandahållande av en tjänst på
    uttrycklig begäran av användaren eller abonnenten."
  - LEK 1 kap. 8 § gives *samtycke* the GDPR's meaning.
- **Who enforces what in Sweden:**
  - **PTS** supervises LEK 9 kap. 28 §. See its cookie supervision of Tele2 and others, opened
    2022-10-07
    ([PTS decision, Tele2](https://www.pts.se/contentassets/7b02c828f0984bfba1d1614dc666ab1a/avslutsbeslut-dnr-22-11378-tele2.pdf)).
  - **IMY** supervises the GDPR.
  - The EDPB and EDPS note that the ePrivacy regulator is each Member State's choice
    ([Joint Opinion 2/2026](https://www.edpb.europa.eu/system/files/documents/2026-02/edpb_edps_jointopinion_202602_digitalomnibus_en.pdf),
    fn. 97).

### 3.2 Is version + OS + architecture personal data, given the IP?

- **Breyer, C-582/14, 19 October 2016**
  ([judgment](https://curia.europa.eu/juris/document/document.jsf?docid=184668&doclang=EN),
  [press release](https://curia.europa.eu/jcms/upload/docs/application/pdf/2016-10/cp160112en.pdf)):
  - "The dynamic internet protocol address of a visitor constitutes personal data, with respect
    to the operator of the website, if that operator has the legal means allowing it to
    identify the visitor concerned with additional information about him which is held by the
    internet access provider."
  - The Court excludes this only where identification "was prohibited by law or practically
    impossible" (para 46).
  - In the same judgment, a site operator "may have a legitimate interest" in storing visitors'
    data, there to protect itself against attacks.
- **EDPS v SRB, C-413/23 P, 4 September 2025**
  ([judgment](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:62023CJ0413),
  [press release 107/25](https://curia.europa.eu/site/upload/docs/application/pdf/2025-09/cp250107en.pdf)):
  - "pseudonymised data must not be regarded as constituting, in all cases and for every person,
    personal data". Pseudonymisation may stop "persons other than the controller" from
    identifying anyone.
  - But for the controller's own duty to inform, "the identifiable nature of the data subject
    must be assessed at the time of collection of the data and from the point of view of the
    controller."
- **IMY on analytics**, 2023-07-03
  ([news](https://www.imy.se/en/news/four-companies-must-stop-using-google-analytics/)): the data
  sent via Google Analytics "is personal data because the data can be linked with other unique
  data that is transferred". IMY fined Tele2 SEK 12 million and CDON SEK 300,000. The case was
  about transfers to the US.

**(derived)** Trustfall's server receives the IP address with every request, so at collection the
request is personal data for Trustfall, whatever fields it carries. Version, OS and architecture
alone identify no one. If the server drops or truncates the IP at once and keeps only aggregate
counts, the *stored* data can be anonymous (Recital 26). The receipt is still processing that
needs a legal basis and a privacy notice. SRB does not change that: it helps a *recipient* that
cannot re-identify, not the controller that collects.

### 3.3 Is a random install ID personal data?

**(derived)** For Trustfall, yes.
- A random ID stored on the device and sent with each ping is an "online identifier" in the sense
  of Art. 4(1) and Recital 30. Its purpose is to single out one installation over time.
- It arrives together with an IP address.
- Recital 26 names "singling out" as a means of identification.
- SRB's relative approach could make a *processor or recipient* without the IPs see it as
  non-personal, but not the controller at collection.

Practice differs:
- **IntelliJ** (JetBrains s.r.o., Czech Republic) puts `uid` (a permanent installation ID) and an
  anonymised machine ID (`mid`) into every update request by default, along with `build`, `os`
  and the update manager
  ([DefaultUpdateRequestParametersProvider.java](https://github.com/JetBrains/intellij-community/blob/51115ae660a4fd8d2a6209976d680c6b01c8580b/platform/platform-impl/src/com/intellij/openapi/updateSettings/impl/DefaultUpdateRequestParametersProvider.java#L27-L49)).
- **KDE's telemetry policy**: "we will not use any unique device, installation or user id", and
  telemetry "is always opt-in"
  ([policy](https://community.kde.org/Policies/Telemetry_Policy)).
- **Fedora's countme** avoids an ID by design (§1.4).

### 3.4 ePrivacy: is the ping "access to terminal equipment", and is it exempt?

[EDPB Guidelines 2/2023 on the technical scope of Art. 5(3)](https://www.edpb.europa.eu/our-work-tools/our-documents/guidelines/guidelines-22023-technical-scope-art-53-eprivacy-directive_en),
version 2.0, adopted 2024-10-07
([PDF](https://www.edpb.europa.eu/system/files/documents/2024-10/edpb_guidelines_202302_technical_scope_art_53_eprivacydirective_v2_en_0.pdf)):

- **Information, not personal data.**
  - Art. 5(3) covers "information", "regardless of whether or not it is personal data" (para 10,
    quoting *Planet49*, [C-673/17](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:62017CJ0673),
    para 70).
  - "Stored information" includes information from "processes and programs executed on the
    terminal equipment" (para 39), such as the OS version or the app's own version.
- **An app calling home is access.** Para 33: "That is equally the case when the accessing entity
  distributes software on the terminal equipment of the user that is stored and will then
  proactively call an Application Programming Interface ('API') endpoint over the network … Such
  access clearly falls within the scope of Article 5(3) ePD".
- **Local use is not access.** Para 44: an installed application using information "strictly
  inside the terminal" is not access "as long as the information does not leave the device".
- **Storing an ID is storage.** Paras 35–37: placing information on the device "by instructing
  software on the terminal equipment to generate specific information", with no minimum duration
  or size. Para 63: collecting a unique identifier is access.
- **Even the IP** can trigger Art. 5(3) where it originates from the terminal (para 55). That
  covers "IPV6 addresses since they are partly defined by the host".
- **Exemptions are outside these guidelines.** They are "analysed on a case-by-case basis
  accounting for the relevant member state transposition(s), and guidance issued by national
  Competent Authorities" (para 4). Applying Art. 5(3) "does not systematically mean that consent
  needs to be collected" (para 56).

The exemption guidance that exists is
[WP29 Opinion 04/2012 on Cookie Consent Exemption (WP194)](https://ec.europa.eu/justice/article-29/documentation/opinion-recommendation/files/2012/wp194_en.pdf),
which the EDPB still cites:

- **The "strictly necessary" test** (criterion B): "the user (or subscriber) did a positive action
  to request a service with a clearly defined perimeter", and "if cookies are disabled, the
  service will not work". It is applied per functionality, and "from the point of view of the
  user, not the service provider".
- **Each purpose is tested separately:** "If a cookie is used for several purposes, it can only
  benefit from an exemption to informed consent if each distinct purpose individually benefits
  from such an exemption."
- **First-party analytics is not exempt:** "they are not strictly necessary to provide a
  functionality explicitly requested by the user … these cookies do not fall under the exemption".
  WP29 adds that they are "not likely to create a privacy risk when they are strictly limited to
  first party aggregated statistical purposes", with clear information, "a user friendly
  mechanism to opt-out" and "comprehensive anonymization mechanisms … such as IP addresses".
  It suggested that legislators "might appropriately add a third exemption criterion".
- **Unique identifiers:** operational purposes such as "research and market analysis, product
  improvement and debugging … in principle … do not justify the use of unique identifiers"
  (said about third-party advertising).

**Swedish application:**
- PTS treats as needing consent cookies that are not "nödvändiga för att utföra en viss tjänst,
  till exempel en teknisk funktion, som användaren uttryckligen begär".
- In the Tele2 case it required consent for the "Statistik" category and for the `ADRUM` cookie
  ([PTS decision](https://www.pts.se/contentassets/7b02c828f0984bfba1d1614dc666ab1a/avslutsbeslut-dnr-22-11378-tele2.pdf)).
  **(derived)** `ADRUM` is AppDynamics' performance-monitoring cookie, so PTS treated
  performance monitoring as non-necessary too.

**The current state of the law, confirmed in 2026:**
- The EDPB and EDPS on the Commission's Digital Omnibus proposal
  ([Joint Opinion 2/2026](https://www.edpb.europa.eu/system/files/documents/2026-02/edpb_edps_jointopinion_202602_digitalomnibus_en.pdf),
  adopted 2026-02-10, fn. 103):
  - The proposal would add consent exceptions for "creating aggregated information about the
    usage of an online service to measure the audience of such a service, where it is carried
    out by the controller of that online service solely for its own use", and for security.
  - "No such exceptions are provided under Article 5(3) ePrivacy Directive."
  - Para 102: the measurement exception should cover only "anonymous aggregated information",
    not "combined with data from other services … or shared with third parties".
  - Para 103: "A provider of security patches should in general therefore be able to install the
    strictly necessary security updates without consent", under conditions.
- **Status:** the Omnibus was in trilogue in mid-2026 and is not law as of this note
  ([Taylor Wessing, 2026](https://www.taylorwessing.com/en/global-data-hub/2026/the-digital-omnibus-proposal/gdh---the-digital-omnibus---cookies);
  **(unverified)** against an official legislative record). The ePrivacy Regulation was
  withdrawn in February 2025.

**Applied to parterre (derived):**

| Element | Art. 5(3) / LEK 9:28 applies? | Exemption under today's law |
|---|---|---|
| Update check (asks "is there a newer version?"; sends version, OS, arch, channel) | yes (para 33) | **arguable** under "strictly necessary for a service explicitly requested". Strongest when the check is a documented feature with a visible switch, and sends only what choosing the right build needs. No regulator has ruled on update checks. |
| Counting usage from those same requests | the access is the same; the purpose is extra | **none today.** WP29: each purpose must qualify; first-party analytics does not. EDPB/EDPS 2026 confirm there is no audience-measurement exception. WP29 calls aggregate, opt-out-able, IP-anonymised first-party statistics low-risk. |
| Random install ID (stored, read, sent) | yes, storage and access | **none** for a counting purpose: consent under the strict reading |
| Crash report or feature events | yes | none: consent |
| A static "latest version" file fetched with nothing but the request | arguably only the IP and HTTP headers leave the device | the closest thing to "strictly necessary". Counting those downloads server-side is still a statistics purpose. |

### 3.5 Legal basis: what comparable EU-based projects use

| Project (controller, country) | Update check | Legal basis stated | Usage / crash data |
|---|---|---|---|
| Audacity ([MuseCY SM Ltd., Cyprus](https://www.audacityteam.org/desktop-privacy-notice/)) | "on by default", with "clear links to disable it when the app is first opened"; sends the User-Agent (version, OS) and the country from the IP; "The full IP address is never stored" | **legitimate interest**: "legitimate interest as a business to offer you our App"; kept up to 12 months | error reports opt-in per report, also legitimate interest |
| IntelliJ ([JetBrains s.r.o., Czech Republic](https://www.jetbrains.com/legal/docs/privacy/privacy/)) | default-on, with `uid`, `mid`, `build`, `os` (§3.3) | **contract**: "The legal basis for this data processing is the performance of a contract between you and us" (providing software and services, notice v3.2, 2026-06-12) | the user's usage-statistics and other consent states are sent as bits with each update request; their defaults not checked here |
| VLC ([VideoLAN, France](https://www.videolan.org/privacy.html)) | sends "Operating System version, CPU version, VLC version"; "accepted on the first run of VLC, and can be disabled"; "No information from the request is kept after the transaction" | not stated | none collected |
| Syncthing ([docs](https://docs.syncthing.net/users/security.html)) | default-on, "do not contain any identifiable information" | not stated on that page | usage reporting opt-in, asked once |
| KDE ([KDE e.V., Germany](https://community.kde.org/Policies/Telemetry_Policy)) | – | – | telemetry "always opt-in", no unique IDs |

**(derived)**
- Legitimate interest (Art. 6(1)(f)) is the basis that fits a free, default-on update check.
  Contract fits JetBrains' licensed products better than a free download.
- GDPR Recital 47 weighs "the reasonable expectations of data subjects based on their
  relationship with the controller".
- Legitimate interest answers only the GDPR question. Where Art. 5(3) / LEK 9:28 requires
  consent for the access, legitimate interest does not replace it. The EDPB/EDPS (Joint Opinion
  2/2026 para 99–100) read the access rule and the processing that follows as tied to the same
  purpose.
- The EDPB's three-step test for legitimate interest is in
  [Guidelines 1/2024](https://www.edpb.europa.eu/our-work-tools/documents/public-consultations/2024/guidelines-12024-processing-personal-data-based_en)
  (version for consultation, adopted 2024-10-08): a legitimate interest, necessity, and
  balancing.

### 3.6 What the privacy notice and the setup must contain

From the [GDPR](https://eur-lex.europa.eu/eli/reg/2016/679/oj); this is what the text requires:

- **Art. 13(1)** (at collection):
  - the controller's identity and contact details (Trustfall AB)
  - the purposes and the legal basis; for Art. 6(1)(f), "the legitimate interests pursued"
  - recipients or categories of recipients (e.g. the hosting backend)
  - any transfer outside the EEA and its safeguard
- **Art. 13(2):**
  - the retention period or the criteria for it
  - the rights of access, rectification, erasure, restriction, objection and portability
  - the right to withdraw consent, where consent is the basis
  - the right to complain to a supervisory authority (IMY)
- **Art. 21(4):** the right to object to legitimate-interest processing "shall be explicitly
  brought to the attention of the data subject and shall be presented clearly and separately
  from any other information", "at the latest at the time of the first communication".
  **(derived)** A first-run notice, or the Settings switch with a sentence beside it, is where
  comparable apps put this (Audacity, VLC).
- **Art. 12(1):** "concise, transparent, intelligible and easily accessible form, using clear
  and plain language".
- **Art. 11:** if the purposes do not require identifying a person, the controller need not keep
  extra data just to answer access requests. With no ID and no stored IP, Arts. 15–20 largely
  fall away.
- **Art. 25(2):** "by default, only personal data which are necessary for each specific purpose
  of the processing are processed".
- **Art. 28:** a data processing agreement with the backend host.
- **Art. 30(5):** the exemption from keeping a record of processing for organisations under 250
  employees does not apply when the processing "is not occasional". **(derived)** A ping on every
  launch is not occasional, so Trustfall AB keeps a record of this processing.
- **Chapter V, if the backend is in the US:**
  - IMY's 2023 Google Analytics decisions were about exactly this.
  - Since then, the EU–US Data Privacy Framework adequacy decision of 10 July 2023 covers
    certified recipients.
  - The General Court upheld that decision in *Latombe*,
    [T-553/23](https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:62023TJ0553),
    3 September 2025.

  Where the data goes is decided elsewhere in #175.
- **Under ePrivacy, where consent is used** (LEK 9:28 first sentence): information about the
  purpose, and consent with the GDPR's meaning: freely given, specific, informed, unambiguous,
  as easy to withdraw as to give. PTS required "lika lätt" refusal and withdrawal in the Tele2
  case.

### 3.7 Side note: the Cyber Resilience Act

- [Regulation (EU) 2024/2847](https://eur-lex.europa.eu/eli/reg/2024/2847/oj) Annex I Part I(2)(c)
  requires that vulnerabilities can be addressed through security updates, including "through
  the notification of available updates to users". Automatic security updates are to be
  "enabled as a default setting, with a clear and easy-to-use opt-out mechanism".
- The main obligations apply from 11 December 2027.
- Free and open-source software that is not monetised falls outside it (the regulation's
  recitals on free and open-source software; recital number not re-checked).

**(derived)** parterre, free and not monetised, is likely outside the CRA. The CRA still shows
EU law treating a default-on update notification as a security feature. That supports the
"legitimate interest" and "strictly necessary" arguments for the check itself, not for
counting.

---

## 4. Channel rules and the law together (derived)

- **Debian and Fedora volunteers** will compile the check out whatever the law allows (§1.3–1.4).
  Give them a cargo feature that does so, as rustup, uv and topgrade do. Also consider a packager
  message à la Zed.
- **Flathub** declares the ping as `social-info` *mild* in the MetaInfo. Opt-in crash reports in
  the Flathub build would make it *intense*.
- **winget:** add a `PrivacyUrl`, since the ping carries an IP (Personal Information under 1.5).
- **Snap, Chocolatey, crates.io:** nothing beyond the privacy notice and honest description.

---

## 5. Plain summary: default vs opt-in

What the sources support, for an app published by a Swedish company. "Strict reading" means
the EDPB's scope guidelines plus WP29's exemption opinion, applied as written.

**May be sent by default** (legitimate interest under GDPR; arguably "strictly necessary" under
LEK 9:28):
- An update check that sends what choosing and offering the right build needs: parterre version,
  OS, architecture, channel.
- No install ID, no hostname or username, no repository or path data.
- The IP not stored, or truncated at once (Audacity keeps three octets, VLC keeps nothing).
- Disclosed in a privacy notice (§3.6), with the right to object stated separately, and a
  switch in Settings.
- On Flathub, declared `social-info` *mild*; on winget, a `PrivacyUrl`.

**Common practice, low risk under GDPR, but without an ePrivacy exemption today:**
- Counting usage from those same update-check requests: daily or weekly totals by version, OS,
  architecture and channel, with no ID and no stored IP.
- WP29 says this is not exempt from consent but "not likely to create a privacy risk" with
  information, an easy opt-out and IP anonymisation.
- The pending Digital Omnibus would make it exempt ("aggregated … audience measurement … solely
  for its own use").
- Comparable precedents: Fedora `countme` (default-on, age bucket, no ID) and Audacity
  (update-check data with country, kept 12 months, legitimate interest).
- **(derived)** Fedora's age bucket, sent instead of an ID, gives new-vs-returning counts
  without storing an identifier beyond a timestamp.

**Needs opt-in under the strict reading:**
- A random install ID, which is storage and access for a counting purpose; personal data for
  Trustfall.
  - Practice is split. IntelliJ sends one by default under contract. KDE forbids them. Fedora
    designed around them.
- Crash reports with stack traces (OARS *intense*; Audacity asks per report).
- Feature-usage events (KDE and Syncthing: opt-in).
- Anything that can carry user content: paths, repository names, remotes, commit data.

## Sources not reachable or not checked

- EUR-Lex and InfoCuria pages did not render for the fetch tool. GDPR and ePrivacy wording is
  quoted from the official texts as I know them, not re-fetched. The Breyer and SRB holdings are
  quoted from the CJEU press releases. Breyer para 46 was checked against a search index of the
  judgment, not the page itself.
- PTS's cookie guidance page (`pts.se/internet-och-telefoni/kakor-cookies/`) was behind a bot
  check. The PTS position is taken from its Tele2 decision.
- The status of the Digital Omnibus comes from secondary sources. The EDPB/EDPS opinion is
  primary.
- The CRA wording is from secondary summaries of Annex I, not re-read on EUR-Lex.
- The seed findings for §1–2 (channel rules, self-updating channels) were gathered on
  2026-10-04 from the primary sources linked. I re-checked the Flathub requirements page
  (including the Generative AI policy), Debian bug #1001647 and the winget policies.
