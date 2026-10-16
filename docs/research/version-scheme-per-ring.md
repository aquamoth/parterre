# A version scheme for one binary published as canary, nightly, rc and stable

Research note for [#402](https://github.com/aquamoth/parterre/issues/402), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390). The decision is made in
[#397](https://github.com/aquamoth/parterre/issues/397). The question: *which version and tag
schemes meet all five requirements below at once, and how does each fare in the ring-switching
cases that broke earlier ideas?*

1. **One build per merge to `main`.** That binary is never rebuilt. Publishing it as nightly, rc
   or stable rewrites the version written into it ([#400](https://github.com/aquamoth/parterre/issues/400)).
2. **The canary version is a semver pre-release** that clearly reads as canary.
3. **A nightly reuses the canary binary under a new, semver-valid name.** Moving between the
   canary and nightly rings causes no ordering problems: not in parterre's update check, and not
   in dpkg, rpm, Windows Installer or winget when a package of one replaces the other.
4. **An rc tag on an existing commit reuses that commit's binary**, and the result still sorts
   and reads as a pre-release.
5. **A stable tag is the plain new version** (`X.Y.Z`), with nothing odd in it.

All of it in strict semver 2.0.0.

Sources: the semver 2.0.0 spec; Debian Policy §5.6.12; rpm's `rpmvercmp.cc`; Microsoft's
Windows Installer and resource-file docs; the WiX `MajorUpgrade` reference; winget-cli's
`Versions.cpp`; the release data, source and docs of the surveyed projects (fetched
16 October 2026); and parterre's own code at `a163d94`. All of them are listed at the end.
Orderings were **tested**, not reasoned out (§8): with an implementation of the spec, with
node-semver 7.8.5, with parterre's own `Version` (the crate built from this tree), with
`dpkg --compare-versions` (dpkg 1.22.22, Debian 13), with `rpm.vercmp` (RPM 6.0.2, Fedora 44),
and with a port of Windows Installer's and winget's comparisons. Package behaviour was tried
with dummy `.deb` and `.rpm` packages.

A statement that is my own conclusion is marked **(derived)**. One I could not check is marked
**(unverified)**.

## TL;DR

- **No scheme meets every scenario, and none can (§4.1).** It comes down to one pair of cases.
  Say ring builds are published after an rc of `X.Y.Z`. Later the stable `X.Y.Z` is tagged,
  either on the rc's own commit, which is *older* than those builds, or on a *later* commit.
  The ring builds' versions were fixed when they were published, and the stable is the same
  plain `X.Y.Z` in both cases (requirement 5). So the stable sorts either above them in both
  cases or below them in both. One of the two cases is always wrong. Each scheme only chooses
  which one. That is the requirement that has to give, in some form.
- **The usual shape, ring word first (`1.0.1-canary.142`, `1.0.1-nightly.142`), fails
  requirement 3.** It is what Next.js, Bun, Electron, TypeScript and Node.js use. In it, a
  nightly followed by the next canary is a downgrade in semver, in parterre's update check, in
  dpkg and in rpm (**tested**). Those projects never have to compare their rings: each ring is a
  separate npm dist-tag, a separate package (`electron-nightly`), a separate app installed side
  by side (VS Code Insiders, Chrome's channels) or a separate toolchain (rustup). None of the
  surveyed projects moves one installation between rings by comparing versions.
- **What works for canary ↔ nightly is the commit number before the ring word**:
  `X.Y.Z-main.<n>.canary` and `X.Y.Z-main.<n>.nightly`, where `n` counts commits on `main`.
  Alphabetically `canary` < `nightly` < `rc`, so relabelling the same commit up the rings is an
  upgrade everywhere (**tested**).
- **The leading word (`main`) is needed whenever rc tags keep their `rc` shape.** In semver a
  numeric identifier sorts *below* a word; in rpm a numeric segment sorts *above* one. So
  `1.0.1-143.nightly` → `1.0.1-rc.1` is an upgrade in semver and dpkg but a downgrade in rpm
  (**tested**, variant D).
- **Three workable candidates** (§3). Each passes requirements 1–3 and 5 as written, and differs
  in the rc and stable cases:
  - **A, bump past every tag:** ring builds after an rc of `X.Y.Z` become `X.Y.(Z+1)-main.n.…`.
    A stable on the rc's commit is handled right. But an rc.2 or stable tagged on a *later*
    commit (the way v0.6.0 and v1.0.0 were released) sorts below the ring builds. Windows
    Installer then refuses it ("A newer version … is already installed").
  - **B, extend the rc** (MinVer's shape): `1.1.0-rc.1.main.143.canary` between rc.1 and rc.2.
    Later rcs and stables sort right. A stable or rc tagged on an *older* commit is offered to
    later ring builds as an update, although it is older. The canary string carries `rc.1`.
  - **E, one shape for every pre-release:** canary, nightly *and rc* are all
    `X.Y.Z-main.n.<ring>`, so an rc becomes `1.1.0-main.142.rc`. It fails only the case that no
    scheme passes: a stable tagged on the rc's (older) commit is offered to later ring builds.
    It costs the `rcN` naming: rc tags become `v1.1.0-main.142.rc`.
- **Today's `rcN` tags aren't ordered by strict semver past 9**: `1.0.0-rc9` > `1.0.0-rc10` (the
  spec, node-semver). parterre's own `Version` departs from the spec on purpose and orders them
  rc9 < rc10. `rc.N` is ordered right everywhere (**tested**).
- **Shipped rc builds** (0.7.0-rc1 … rc3, 1.0.0-rc1) take the highest pre-release of any kind.
  With every candidate they will be offered ring builds whose tag starts `v<digit>`. The shipped
  `Version::parse` passes over a tag that begins any other way (`nightly-…`, `canary/…`):
  **tested** with the shipped code.
- **What packages do with a downgrade** (**tested**): `dpkg -i` installs the lower version;
  `apt-get install -y ./x.deb` refuses without `--allow-downgrades`; `rpm -U` refuses ("which is
  newer … is already installed"); `dnf install ./x.rpm` downgrades. Windows Installer reads only
  `X.Y.Z`. With `AllowSameVersionUpgrades` the same `X.Y.Z` replaces itself either way, and a
  lower one is refused. winget only ever sees that numeric MSI version.

## 1. The constraints, from the primary sources

### 1.1 Semver 2.0.0

- A pre-release is "a series of dot separated identifiers", `[0-9A-Za-z-]`, not empty, numeric
  ones without leading zeroes. "Pre-release versions have a lower precedence than the associated
  normal version" (§9).
- Build metadata (`+…`) "MUST be ignored when determining version precedence" (§10).
- Precedence (§11): identifiers are compared left to right. "Identifiers consisting of only
  digits are compared numerically." "Identifiers with letters or hyphens are compared lexically
  in ASCII sort order." "Numeric identifiers always have lower precedence than non-numeric
  identifiers." "A larger set of pre-release fields has a higher precedence than a smaller set,
  if all of the preceding identifiers are equal."
- So **the first identifier that differs decides**. If canary and nightly start with their ring
  word, the ring decides before the commit can. `rc10` is one alphanumeric identifier compared
  in ASCII, so it sorts below `rc9`; the spec's own example uses `rc.1`.
- "v1.2.3" is not a semantic version but is "a common way … to indicate it is a version number"
  as a tag name (FAQ). A tag is free to carry more than the version.

### 1.2 dpkg (Debian Policy §5.6.12)

- `upstream_version` may contain alphanumerics and `. + - ~`; without a `debian_revision`
  "hyphens are not allowed". This is why `packaging/linux/build-packages.sh` writes every `-`
  as `~`.
- Comparison alternates between non-digit and digit runs. In the non-digit run "all the letters
  sort earlier than all the non-letters and … a tilde sorts before anything, even the end of a
  part". Digit runs are compared numerically.
- Effects:
  - `1.0.1~x` < `1.0.1`, as a pre-release should be.
  - An empty run sorts below a letter, so `~143` < `~rc`, which agrees with semver.
  - `+` is compared like any other character, so build metadata *does* order in dpkg.

### 1.3 rpm (`rpmio/rpmvercmp.cc`)

- Versions split into alphabetic and numeric segments, and separators are skipped.
- `~` "sorts before everything else". `^` is the same "except that if one of the strings ends
  (base version), the other is considered as higher version". So `~` serves pre-releases and
  `^` post-release snapshots.
- When one segment is numeric and the other alphabetic: "numeric segments are always newer than
  alpha segments". **This is the opposite of semver.** `1.0.1~143.nightly` > `1.0.1~rc.1` in
  rpm, but `1.0.1-143.nightly` < `1.0.1-rc.1` in semver.
- rpm's `Version` can't contain `-`, so the mapping is the same `-` → `~` as for dpkg.

### 1.4 Windows Installer and the version resource

- `ProductVersion` is `major.minor.build`, at most 255.255.65535. "Windows Installer uses only
  the first three fields … If you include a fourth field … the installer ignores the fourth
  field." The Upgrade table's `VersionMin`/`VersionMax` say the same.
- WiX `MajorUpgrade`:
  - With `AllowSameVersionUpgrades="yes"`, an equal version is treated as a major upgrade. parterre
    sets it (`packaging/windows/parterre.wxs`).
  - With `AllowDowngrades` left at "no", "products with lower version numbers are blocked from
    installing when a product with a higher version is installed". parterre's message: "A newer
    version of parterre is already installed."
  - The default `Schedule`, `afterInstallValidate`, "removes the installed product entirely
    before installing the upgrade product". So file-version rules never decide which
    `parterre.exe` stays.
- **Effect: an MSI orders by `X.Y.Z` alone.** Any pre-release of `X.Y.Z` and `X.Y.Z` itself
  replace each other in either direction. Only a lower `X.Y.Z` is refused (#393 tried this).
- The `VERSIONINFO` resource holds `FILEVERSION` and `PRODUCTVERSION` as "four 16-bit integers"
  each (0–65535), plus free-text `FileVersion`/`ProductVersion` strings.
  - Today `build.rs` writes `X,Y,Z,0` and the whole version string.
  - The patcher of #400 rewrites both in place.
  - The fourth number could carry the commit number `n` (§5). That is cosmetic: Windows
    Installer ignores it.

### 1.5 winget

- `Versions.cpp` splits a version on `.`, reads each part as a leading integer plus an "Other"
  rest, and compares parts in turn. When the integers are equal, a part *with* an Other sorts
  *below* one without, and Others compare case-insensitively.
- winget sees parterre's version only through the installed MSI's ARP entry, which is the
  numeric `ProductVersion` (#393). Its manifests are stable only (#396, #398). So **winget
  orders exactly as the MSI does**. The tests confirm it: the winget port and the MSI order
  agreed on all 344 moves.

### 1.6 parterre today

- **`update.rs`, `Version`:** semver's order, except that "identifiers mixing letters and digits
  compare their digits as numbers" (`rc2` < `rc10`). `+…` and ` (…)` are dropped. In all 344
  moves tested here it agreed with the spec, because no candidate mixes letters and digits in
  one identifier.
- **`newer`:** a stable build asks `releases/latest` (never a pre-release). A pre-release build
  asks for 30 releases and takes the highest *of any kind* that is newer (`github.rs`). This is
  what shipped in 0.7.0-rc1 … 1.0.0. The current code is identical, apart from `notes`.
- **Packages:**
  - `build-packages.sh` takes `parterre --version`'s second word and writes `-` as `~`.
  - `build-msi.ps1` cuts the version at the first `-` or `+` to get the MSI version.
  - `build.rs` derives the numeric resource version the same way.
- **`build.rs`:**
  - The nearest tag comes from `git describe --tags --match 'v[0-9]*'`. A dev build is
    `X.Y.(Z+1)-dev.<since>+<commit>` after a stable tag, and `<tag>.dev.<since>+<commit>` after
    a pre-release tag.
  - **Any ring tag of the form `v1.0.1-…` would match that pattern**, so dev builds would then
    describe themselves from canary tags. The same applies to `metainfo.sh`, which lists `v*`
    tags as AppStream releases.

## 2. What real projects do

What each one publishes, read from its own release data or source on 16 October 2026:

| Project | Canary / nightly | Beta / rc | Stable | Rebuilt or relabelled | How one install changes ring |
|---|---|---|---|---|---|
| **Rust** | `1.101.0-nightly (4ebd25825 2026-10-15)` | `1.100.0-beta.5 (8ea38001b 2026-10-13)` | `1.99.1 (58cf8f9c3 2026-10-13)` | Rebuilt: the channel is a configure flag (`--release-channel=$RUST_RELEASE_CHANNEL`, `src/ci/run.sh`) | It doesn't: rustup installs toolchains side by side |
| **Node.js** | `v27.0.0-nightly202610161c90519c1a`, `v27.0.0-v8-canary20261014e0f9251822` | `v26.4.0-rc.0` | `v26.12.0` | Rebuilt: `DISTTYPE`/`DATESTRING`/`COMMIT` give `TAG=$(DISTTYPE)$(DATESTRING)$(COMMIT)` (Makefile) | Separate download directories |
| **Deno** | `X.Y.Z+<sha7>` (build metadata, not a pre-release); upgrades go by commit hash | `2.0.0-rc.1` | `v2.9.7` | **Relabelled**: `promote_to_release.ts` takes the canary binaries and runs `patchver` to stamp channel `rc`, then re-signs | `deno upgrade --canary` / `--rc`: chosen by the user |
| **Bun** | `1.0.0-canary.12+abcdefghi`, where the number is commits ahead of the latest release (`getCanaryRevision`, `.buildkite/ci.ts`) | – | `X.Y.Z` | Rebuilt: canary is a compile-time flag | `bun upgrade --canary`; one moving `canary` release |
| **Next.js** | `16.5.0-canary.15` (npm `canary`) | `15.0.0-rc.1`, `16.0.0-beta.0` | `16.4.2` | Rebuilt per publish (unverified) | npm dist-tags |
| **React** | `19.3.0-canary-098d2e87-20261015` | `19.0.0-rc.1` | `19.3.1` | **Relabelled**: built once with a placeholder version, then the version string is replaced per channel ("aside from the version, it's no different from the corresponding canary", `build-all-release-channels.js`); stable is "promoted" from a canary | npm dist-tags |
| **TypeScript** | `7.1.0-dev.20261016.1` (npm `next`) | `7.0.1-rc` | `7.0.2` | Rebuilt (unverified) | npm dist-tags |
| **Electron** | `46.0.0-nightly.20261015` in a separate npm package, `electron-nightly` | `45.0.0-alpha.16`, `45.0.0-beta.5` | `44.8.0` | Rebuilt (unverified) | Separate package |
| **Neovim** | `NVIM v0.13.0-dev-1868+gbe5167e21c` (`git describe --first-parent` count after `-dev`) | – | `v0.12.x` | Rebuilt | One moving `nightly` release |
| **VS Code** | `1.143.0-insider` | – | `1.142.0` | Rebuilt per quality (unverified) | Insiders "runs next to the Stable build" |
| **Chrome** | Canary `158.0.8108.0`, Dev `157.0.8092.0` | Beta `157.0.8103.6` | `156.0.8078.36` | The same version number is published on several channels: `152.0.7977.54` on beta and stable, `156.0.8078.36` on stable and extended (versionhistory API). The version doesn't name the channel | Separate apps per channel (unverified) |
| **Firefox** | Nightly `160.0a1` | `159.0b2` | `158.0` | Separate trains (unverified whether the final beta-channel RC binary is the one shipped as release) | Separate channels |

Version tools:

- **GitVersion:**
  - Continuous Delivery numbers every commit after a tag (`1.1.0-0`, `1.1.0-1`, …). Continuous
    Deployment keeps "the same semantic version for every commit until it has been tagged".
  - `main` increments `Patch` by default.
- **MinVer:**
  - An untagged commit after an RTM tag `1.0.0` becomes `1.0.1-alpha.0.<height>`. After a
    pre-release tag the height is appended: `1.0.0-beta.1.42`. This is candidate B's shape.
  - "Height is not for versioning your software … The presence of height in a version signals
    that the version should not be released."
- **Nerdbank.GitVersioning:**
  - The version lives in `version.json` in the commit, e.g. `1.2.3-beta.{height}`, with a
    `-g<commit>` suffix outside a "public release".
  - "**There is no way to remove the `-prerelease` tag from an existing commit**". The same
    commit can't later become stable, which rules it out for requirements 4 and 5.
- **semantic-release:**
  - A pre-release branch gives `2.0.0-beta.1`, `2.0.0-beta.2`, with the identifier taken from the
    branch.
  - Promoting adds an *existing* version to another channel without a new version: "a release
    version must be unique, independently of the distribution channel".
- **changesets:** snapshots are `0.0.0-{tag}-DATETIMESTAMP` and are published under a dist-tag
  "REALLY IMPORTANT[ly]", so that they don't become `latest`.

What the survey shows **(derived)**:

- **Ring word first is the norm**: Next.js, Bun, Electron, TypeScript (`dev`), Node.js, Rust and
  semantic-release. It works for them because nothing compares one ring's version with another
  ring's for the same installation: dist-tags, separate packages and side-by-side apps keep them
  apart. **No surveyed project does what requirement 3 asks.**
- **Several published shapes don't order under strict semver** (**tested**, node-semver):
  - Neovim's `0.13.0-dev-999+g…` > `0.13.0-dev-1868+g…`: `dev-1868` is one identifier, compared
    in ASCII.
  - React's canaries sort by commit hash: the October canary sorts below the May one.
  - parterre's own `1.0.0-rc9` > `1.0.0-rc10`.
  - Electron's `46.0.0-nightly.20261015` sorts above `46.0.0-alpha.1`.
- **Relabelling one build is established practice**:
  - React rewrites the version string in a single build for each channel.
  - Deno patches canary binaries into rc binaries.
  - Chrome publishes one version number on several channels.

  In Deno and Chrome the channel lives beside the version, not in it. parterre has instead put
  the ring in the version (#396).

## 3. Candidates side by side

Shared by all of them:

- **`n`**, the commit's number on `main`: `git rev-list --count --first-parent <commit>` (144
  at `a163d94`). It only grows along `main`, whether merges are merge commits, squashes or
  rebases (**derived**). It is known when the canary is built and can be recomputed for any
  commit later. CI already checks out the whole history.
  - Alternatives: the count of all commits (519 today), or the commit time as `YYYYMMDDhhmmss`.
    The time is readable, but it doesn't fit `FILEVERSION`'s 16 bits and is only monotonic by
    convention.
  - A force-push to `main` could repeat an `n`. The tag would then already exist, and publishing
    would fail rather than mislabel.
- **The commit hash** stays outside the semver string: `parterre 1.0.1-main.145.nightly
  (a1b2c3d)`, as release builds print `0.5.1 (a1b2c3d)` today. The `.deb`/`.rpm` scripts
  already drop it (they take the second word). It fits the 64-byte slot of #400 (32 bytes).
- **The platform** goes only in the asset file names (the target triple), as today. One release
  per build holds every platform, and the version is the same on all of them.
- **The base `X.Y.Z` before the next version is known:** the next patch after the newest stable
  tag. That is the smallest number above it, and no next release can sort below it: `1.1.0` >
  `1.0.1-…`. This is #393's "`X.Y.(Z+1)` after `vX.Y.Z`", and the default of MinVer and
  GitVersion's `main`. The candidates differ in what happens after an **rc** tag.
- **Mappings:** deb and rpm write every `-` as `~` (unchanged). The MSI `ProductVersion` is
  `X.Y.Z` (unchanged). Optionally `FILEVERSION X,Y,Z,n`.
- **The stable** is `X.Y.Z`, tag `vX.Y.Z` (requirement 5).

The examples below run from `v1.0.0` on commit 141, with an rc of 1.1.0 tagged on commit 142
and a stable on commit 145.

| | **A: bump past every tag** | **B: extend the rc** | **E: one shape, rc numbered by commit** | *C: ring word first (reference)* |
|---|---|---|---|---|
| canary after `v1.0.0` | `1.0.1-main.142.canary` | `1.0.1-main.142.canary` | `1.0.1-main.142.canary` | `1.0.1-canary.142` |
| nightly, same commit | `1.0.1-main.142.nightly` | `1.0.1-main.142.nightly` | `1.0.1-main.142.nightly` | `1.0.1-nightly.142` |
| rc | `1.1.0-rc.1` (or today's `rc1`) | `1.1.0-rc.1` | `1.1.0-main.142.rc` | `1.1.0-rc.1` |
| canary after that rc | `1.1.1-main.143.canary` | `1.1.0-rc.1.main.143.canary` | `1.1.0-main.143.canary` | `1.1.1-canary.143` |
| stable | `1.1.0` | `1.1.0` | `1.1.0` | `1.1.0` |
| base after an rc of `X.Y.Z` | `X.Y.(Z+1)` | the rc itself, extended | `X.Y.Z` | `X.Y.(Z+1)` |
| dev build (unchanged rule) | `1.0.2-dev.3+a1b2c3d` | same | same; after an rc: `1.1.0-main.142.rc.dev.3+a1b2c3d` | same |
| tags | `v1.0.1-main.142.canary`, `v1.1.0-rc.1` | same pattern | `v1.0.1-main.142.canary`, `v1.1.0-main.142.rc` | `v1.0.1-canary.142` |
| ring read from the version | the identifier after `n`; plain = stable; `rc…` = rc | the last identifier | the identifier after `n` (`canary`/`nightly`/`rc`); plain = stable | the first identifier |
| `.deb` / `.rpm` | `1.0.1~main.142.canary` | `1.1.0~rc.1.main.143.canary` | `1.1.0~main.142.rc` | `1.0.1~canary.142` |
| MSI | `1.0.1`, `1.1.1` after the rc | `1.1.0` | `1.1.0` | as A |
| Precedents | Commit number before the label: GitVersion's numbered commits, Neovim's `-dev-N`, Bun's `canary.N` (but label first); bumping past an rc: none found | **MinVer**; parterre's own dev builds today (`0.5.0-rc1.dev.3`) | None for the whole shape. Its parts: every commit numbered (GitVersion CD); rings relabelling one build (React, Deno) | Next.js, Bun, Electron, TypeScript, Node.js |

Variants that were tested and set aside:

- **D, A without the word** (`1.0.1-143.nightly`): rpm disagrees with semver and dpkg whenever a
  ring build meets an rc of the same base (§1.3). `nightly143 → rc.1` is a downgrade in rpm
  only.
  - In **E**, the word can go (`1.1.0-142.rc`, variant E0). Every published pre-release then
    starts with a number, and E0 passed every test E did. Dev builds (`1.0.2-dev.3`) would then
    sort above ring builds of the same base in semver and below them in rpm. That is harmless
    while dev builds aren't published.
- **A′, the ring in build metadata** (`1.0.1-main.142+canary`):
  - semver and parterre's update check rank canary and nightly of one commit as equal, but dpkg
    and rpm still order them, `+canary` < `+nightly` (**tested**).
  - The pre-release part no longer says canary.
  - GitHub "renames asset filenames that have special characters", so `+` in file names is a
    risk (unverified which characters).

## 4. The scenarios

Results of the tests (§8). "Offered" means that parterre's update check sees the target as
newer. dpkg and rpm are what a package does when the target replaces the source.

| Scenario | A | B | E | C |
|---|---|---|---|---|
| Successive canaries | ✓ | ✓ | ✓ | ✓ |
| canary → nightly of the same commit (relabel up) | ✓ upgrade everywhere | ✓ | ✓ | ✓ |
| canary → nightly of a later commit | ✓ | ✓ | ✓ | ✓ |
| **nightly → canary of a later commit** | ✓ | ✓ | ✓ | **✗ downgrade** in semver, update check, dpkg, rpm |
| nightly → canary of the *same* commit (by hand only) | downgrade everywhere except the MSI | same | same | same |
| canary/nightly → first rc, on the newest commit | ✓ | ✓ | ✓ | ✓ |
| rc → canary/nightly of a later commit | ✓ (MSI up) | ✓ (MSI equal) | ✓ (MSI equal) | ✓ |
| **rc.2 tagged on a later commit than rc.1** | **✗** sorts below the ring builds since rc.1: not offered; the MSI refuses it by hand | ✓ | ✓ | **✗** as A |
| **rc tagged on an older commit than the newest ring build** | **✗** offered to later ring builds although older | **✗** as A | ✓ (sorts by `n`) | **✗** as A |
| stable straight from the newest canary's commit | ✓ | ✓ | ✓ | ✓ |
| **stable on a later commit than its rc** (v0.6.0, v1.0.0) | **✗** sorts below the ring builds: same binary needs a downgrade; MSI refuses; a nightly install narrowed to stable waits for the next stable | ✓ | ✓ | **✗** as A |
| **stable on the rc's commit, after later ring builds** | ✓ not offered (correct: it's older) | **✗** offered as an update although older | **✗** as B | ✓ |
| canary/nightly after a stable | ✓ | ✓ | ✓ | ✓ |
| MSI / winget never asked to go down a version for newer code | ✗ (the ✗ rows above) | ✓ | ✓ | ✗ |

### 4.1 Why no scheme passes both stable rows (derived)

- Let `R` be a ring build of commit `c'`, published after an rc of `X.Y.Z` on commit `c` < `c'`.
  `R`'s version is fixed when `R` is published.
- If the stable `X.Y.Z` is later tagged on a commit ≥ `c'`, it is newer code, so it must sort
  above `R`.
- If it is tagged on `c` instead, it is older code, so it must not sort above `R`.
- Requirement 5 makes the stable the same string, `X.Y.Z`, in both cases. When `R` is published,
  nothing yet says which case will happen.
- So one of the two is misordered, whatever `R` looks like:
  - A, C and anything else that bumps `R` above `X.Y.Z` gets the later-commit case wrong.
  - B and E, which keep `R` below `X.Y.Z`, get the rc's-commit case wrong.
- The only fix uses information outside the version: for example, the update check reads the
  stable's commit number from somewhere it already fetches (the release's name, say) and
  doesn't offer a stable older than the running build. Packages need nothing for this in B or
  E: the move is an upgrade for dpkg, rpm and the MSI (**tested**). The cost is one step back
  that the update check offers, and the next ring build moves forward again.
- This repository has so far released the stable on a later commit than its last rc every
  time (v0.5.0, v0.6.0, v1.0.0): `v1.0.0-rc1` on first-parent commit 138, `v1.0.0` on 141.

## 5. Against the five requirements

| | A | B | E | C |
|---|---|---|---|---|
| 1. One build, version written at publishing | ✓ | ✓ | ✓ | ✓ |
| 2. Canary is a semver pre-release that reads as canary | ✓ (`…-main.142.canary`: ring last) | ◐ after an rc it reads `1.1.0-rc.1.main.143.canary` | ✓ (ring last) | ✓ (ring first: the clearest) |
| 3. canary ↔ nightly without ordering problems (update check, dpkg, rpm, MSI, winget) | ✓ | ✓ | ✓ | **✗** nightly → next canary is a downgrade |
| 4. rc reuses a commit's binary and sorts/reads as a pre-release | ◐ reads ✓; an rc.2 or rc on a later commit sorts below ring builds; an rc on an older commit is offered | ◐ ✓ except an rc on an older commit, which is offered | ✓, but rc changes shape to `X.Y.Z-main.n.rc` (no `rcN` sequence) | ◐ as A |
| 5. Stable is plain `X.Y.Z` | ✓ string; ✗ when tagged later than its rc | ✓ string; ✗ when tagged on the rc's commit after later ring builds (§4.1) | ✓ string; ✗ as B (§4.1) | ✓ string; ✗ as A |
| Strict semver 2.0.0 | ✓ with `rc.N` (`rcN` misorders past 9) | ✓ with `rc.N` | ✓ | ✓ with `rc.N` |

**Plainly: no scheme meets all five in every scenario.**

- **E** meets all five as they are written. It fails one scenario, and §4.1 shows that no scheme
  can pass that one by its version alone. Its price is that rc tags lose the `X.Y.Z-rcN` form
  that #397 assumed.
- **Keeping `X.Y.Z-rcN`** means choosing between A and B:
  - **A** gives up "stable/rc on a later commit than the first rc". That is how this repository
    has released so far, and the MSI then refuses moves by hand.
  - **B** gives up "rc on an older commit" and "stable on the rc's commit", and puts `rc.1` into
    canary strings.
- **C**, the common shape, is the only one that fails requirement 3 outright.

## 6. Costs and caveats

- **A**
  - Between an rc and its stable, ring builds claim a version, `X.Y.(Z+1)`, beyond the one about
    to be released.
  - A nightly install narrowed to stable in Settings is not offered `X.Y.Z` until `X.Y.(Z+1)` or
    higher ships.
  - Its winget (user-scope) entry reads `X.Y.(Z+1)`, so `winget upgrade` ignores `X.Y.Z`. That
    is harmless, since winget never downgrades.
  - Installing the stable MSI by hand over such a nightly fails with "A newer version of parterre
    is already installed."
- **B**
  - Canary and nightly strings get long and mention the rc (`1.1.0-rc.1.main.143.nightly`, 27
    characters).
  - Reading the ring needs "last identifier", while rc versions are recognised by `rc` coming
    first.
  - An rc tagged on an older commit is offered to every newer ring build once.
- **E**
  - rc tags become `v1.1.0-main.142.rc`. The maintainer needs `n` when tagging, from
    `git rev-list --count --first-parent <commit>`: a helper script, or a workflow that tags.
    That changes `docs/releasing.md`.
  - The rc sequence number disappears. Adding one (`…main.142.rc.1`) doesn't change the order,
    but then the ring is no longer the last identifier.
  - Old tags (`v1.0.0-rc1`) stay valid and older.
- **All of them**
  - **The release workflow must ignore ring tags**: `tags-ignore` (#392).
  - **`git describe --match 'v[0-9]*'` in `build.rs`, and the AppStream list in `metainfo.sh`,
    must exclude ring tags.** Otherwise dev builds describe themselves from canary tags. The
    alternative is to name ring tags so that they don't match.
  - **Shipped rc builds** (0.7.0-rcN, 1.0.0-rc1) will be offered any ring build whose tag parses,
    and every candidate's ring builds sort above `1.0.0`.
    - **Tested:** the shipped `Version::parse` returns `None` for `nightly/v1.0.1-main.145.nightly`,
      `nightly-1.0.1-main.145` and `canary/1.0.1-main.145.canary`. So a ring tag that doesn't start
      with `v<digit>` or a digit hides it from them.
    - Whether a `/` in a tag works in `releases/download/<tag>/…` URLs is unverified.
    - That is a tag-naming choice for #397. Every candidate's version string fits in such a tag.
  - **`nightly → canary` of the same commit is always a downgrade** (`canary` < `nightly`). It is
    never offered, and only arises by hand:
    - `rpm -U` refuses it;
    - `apt-get -y` needs `--allow-downgrades`;
    - `dpkg -i` and `dnf install` go ahead.
    - The MSI treats it as the same version.
  - **`rcN` to `rc.N`** (A, B, C): strict semver orders `rc10` below `rc9`; parterre's `Version`
    does not.

## 7. For #397

These follow from the tests whatever is chosen:

- The ring belongs *after* a shared commit number in the pre-release. `canary` < `nightly` <
  `rc` is already the nesting order, so relabelling one commit up the rings is an upgrade in
  every comparator.
- The leading word is needed only while rc tags keep a word first (A, B).
- Ring tags need excluding from `build.rs`'s and `metainfo.sh`'s tag patterns, and from
  `release.yml`.
- The shipped rc builds can only be kept away from ring builds by how the tags are named, not by
  the version.
- §4.1 can only be worked around outside the version, for example by the update check knowing
  each release's commit number.

## 8. Method and results

The scripts were run outside the repository (in the system temp folder). They work like this:

- **Candidates.** A, A′, D, B, E, E0, C and X (ring first, no bump past an rc) are generated from
  four timelines that start at `v1.0.0` on commit 141:
  - T1: rc on 144, ring builds 145–146, stable on 144 (the rc's commit), ring builds 147.
  - T2: stable straight from the canary of 143.
  - T3: rc.1 on 142, rc.2 on 144, stable on 145 (later commits, as v1.0.0 was).
  - T4: rc tagged on 142 while ring builds of 143–144 exist, stable on 145.
- **Moves.** 43 moves in all, each labelled with whether the target's commit is later, the
  same or older. Each move is compared by:
  - the spec's precedence, implemented from §11, with every string checked against the spec's
    regular expression;
  - **node-semver 7.8.5**, which agreed with the spec implementation on all 344 moves;
  - **parterre's `Version`**: a small binary that depends on `crates/parterre-telemetry` from
    this tree, built without `send`;
  - **`dpkg --compare-versions`** in `debian:stable-slim` (dpkg 1.22.22), after `-` → `~`;
  - **`rpm.vercmp`** via `rpm --eval '%{lua: …}'` in `fedora:latest` (RPM 6.0.2), after
    `-` → `~`;
  - the **MSI order** (the first three numbers) and a **port of winget's `Version::operator<`**,
    which agreed with the MSI on every move.
- **Where the comparators disagreed:**
  - rpm on D (`1.0.1~143.nightly` vs `1.0.1~rc.1`);
  - dpkg and rpm on A′ (`+canary` vs `+nightly` of the same commit);
  - nowhere else.
- **Package behaviour.** Dummy packages `demo` were built at `1.0.1~main.145.nightly` and
  `1.0.1~main.146.canary` (dpkg-deb, rpmbuild) and installed over each other:
  - **dpkg:** `dpkg -i` of the lower over the higher unpacked it ("Unpacking demo
    (1.0.1~main.145.nightly) over (1.0.1~main.146.canary)").
  - **apt:** `apt-get install -y ./lower.deb` ended with "E: Packages were downgraded and -y
    was used without --allow-downgrades."
  - **rpm:** `rpm -U` printed "package demo-1.0.1~main.146.canary-1.noarch (which is newer than
    demo-1.0.1~main.145.nightly-1.noarch) is already installed".
  - **dnf:** `dnf install ./lower.rpm` (dnf5) downgraded.
- **The shipped update check.** The same binary called `newer` with the running version
  `1.0.0-rc1` / `0.7.0-rc3`:
  - a `v1.0.1-main.145.nightly` tag is offered;
  - tags beginning `nightly/`, `nightly-` or `canary/` are passed over.

Candidate E's full results:

#### T1 rc, then stable on the rc's commit

| move | commit | from | to | semver | parterre | dpkg | rpm | MSI |
|---|---|---|---|---|---|---|---|---|
| canary142 → canary143 | later | `1.0.1-main.142.canary` | `1.0.1-main.143.canary` | up | up | up | up | eq |
| canary143 → nightly143 | same | `1.0.1-main.143.canary` | `1.0.1-main.143.nightly` | up | up | up | up | eq |
| nightly143 → canary144 | later | `1.0.1-main.143.nightly` | `1.0.1-main.144.canary` | up | up | up | up | eq |
| canary142 → nightly143 | later | `1.0.1-main.142.canary` | `1.0.1-main.143.nightly` | up | up | up | up | eq |
| nightly143 → canary143 | same | `1.0.1-main.143.nightly` | `1.0.1-main.143.canary` | DOWN | DOWN | DOWN | DOWN | eq |
| canary144 → rc@144 | same | `1.0.1-main.144.canary` | `1.0.1-main.144.rc` | up | up | up | up | eq |
| nightly143 → rc@144 | later | `1.0.1-main.143.nightly` | `1.0.1-main.144.rc` | up | up | up | up | eq |
| rc@144 → canary145 | later | `1.0.1-main.144.rc` | `1.0.1-main.145.canary` | up | up | up | up | eq |
| rc@144 → nightly145 | later | `1.0.1-main.144.rc` | `1.0.1-main.145.nightly` | up | up | up | up | eq |
| nightly145 → canary146 | later | `1.0.1-main.145.nightly` | `1.0.1-main.146.canary` | up | up | up | up | eq |
| rc@144 → stable@144 | same | `1.0.1-main.144.rc` | `1.0.1` | up | up | up | up | eq |
| nightly145 → stable@144 | older | `1.0.1-main.145.nightly` | `1.0.1` | up | up | up | up | eq |
| canary146 → stable@144 | older | `1.0.1-main.146.canary` | `1.0.1` | up | up | up | up | eq |
| canary146 → canary147 | later | `1.0.1-main.146.canary` | `1.0.2-main.147.canary` | up | up | up | up | up |
| stable@144 → canary147 | later | `1.0.1` | `1.0.2-main.147.canary` | up | up | up | up | up |
| stable@144 → nightly147 | later | `1.0.1` | `1.0.2-main.147.nightly` | up | up | up | up | up |

#### T2 stable straight from a canary's commit

| move | commit | from | to | semver | parterre | dpkg | rpm | MSI |
|---|---|---|---|---|---|---|---|---|
| canary142 → nightly142 | same | `1.0.1-main.142.canary` | `1.0.1-main.142.nightly` | up | up | up | up | eq |
| nightly142 → canary143 | later | `1.0.1-main.142.nightly` | `1.0.1-main.143.canary` | up | up | up | up | eq |
| canary143 → stable@143 | same | `1.0.1-main.143.canary` | `1.0.1` | up | up | up | up | eq |
| nightly142 → stable@143 | later | `1.0.1-main.142.nightly` | `1.0.1` | up | up | up | up | eq |
| stable@143 → canary144 | later | `1.0.1` | `1.0.2-main.144.canary` | up | up | up | up | up |
| canary143 → canary144 | later | `1.0.1-main.143.canary` | `1.0.2-main.144.canary` | up | up | up | up | up |
| nightly142 → nightly144 | later | `1.0.1-main.142.nightly` | `1.0.2-main.144.nightly` | up | up | up | up | up |

#### T3 rc.1, rc.2, stable on later commits

| move | commit | from | to | semver | parterre | dpkg | rpm | MSI |
|---|---|---|---|---|---|---|---|---|
| canary142 → rc@142 | same | `1.0.1-main.142.canary` | `1.1.0-main.142.rc` | up | up | up | up | up |
| rc@142 → canary143 | later | `1.1.0-main.142.rc` | `1.1.0-main.143.canary` | up | up | up | up | eq |
| canary143 → nightly143 | same | `1.1.0-main.143.canary` | `1.1.0-main.143.nightly` | up | up | up | up | eq |
| nightly143 → canary144 | later | `1.1.0-main.143.nightly` | `1.1.0-main.144.canary` | up | up | up | up | eq |
| canary144 → rc@144 | same | `1.1.0-main.144.canary` | `1.1.0-main.144.rc` | up | up | up | up | eq |
| nightly143 → rc@144 | later | `1.1.0-main.143.nightly` | `1.1.0-main.144.rc` | up | up | up | up | eq |
| rc@142 → rc@144 | later | `1.1.0-main.142.rc` | `1.1.0-main.144.rc` | up | up | up | up | eq |
| rc@144 → canary145 | later | `1.1.0-main.144.rc` | `1.1.0-main.145.canary` | up | up | up | up | eq |
| canary145 → stable@145 | same | `1.1.0-main.145.canary` | `1.1.0` | up | up | up | up | eq |
| nightly143 → stable@145 | later | `1.1.0-main.143.nightly` | `1.1.0` | up | up | up | up | eq |
| rc@144 → stable@145 | later | `1.1.0-main.144.rc` | `1.1.0` | up | up | up | up | eq |
| stable@145 → canary146 | later | `1.1.0` | `1.1.1-main.146.canary` | up | up | up | up | up |

#### T4 rc tagged on an older commit than the newest canary

| move | commit | from | to | semver | parterre | dpkg | rpm | MSI |
|---|---|---|---|---|---|---|---|---|
| nightly143 → rc@142 | older | `1.0.1-main.143.nightly` | `1.0.1-main.142.rc` | DOWN | DOWN | DOWN | DOWN | eq |
| canary144 → rc@142 | older | `1.0.1-main.144.canary` | `1.0.1-main.142.rc` | DOWN | DOWN | DOWN | DOWN | eq |
| rc@142 → canary145 | later | `1.0.1-main.142.rc` | `1.0.1-main.145.canary` | up | up | up | up | eq |
| rc@142 → nightly145 | later | `1.0.1-main.142.rc` | `1.0.1-main.145.nightly` | up | up | up | up | eq |
| nightly145 → stable@145 | same | `1.0.1-main.145.nightly` | `1.0.1` | up | up | up | up | eq |
| rc@142 → stable@145 | later | `1.0.1-main.142.rc` | `1.0.1` | up | up | up | up | eq |
| canary144 → stable@145 | later | `1.0.1-main.144.canary` | `1.0.1` | up | up | up | up | eq |
| stable@145 → canary146 | later | `1.0.1` | `1.0.2-main.146.canary` | up | up | up | up | up |

The problems the same run found in the other candidates, move by move:

- **A:**
  - T3 `1.1.1-main.143.nightly → 1.1.0-rc.2` and `→ 1.1.0`: later commits, but lower in every
    comparator including the MSI.
  - T3 and T4: the same binary needs a downgrade from the bumped canary to the stable.
  - T4: `1.0.1-main.144.canary → 1.0.1-rc.1` is an older commit offered as newer.
- **B:**
  - T1 `1.0.1-rc.1.main.145.nightly → 1.0.1`: an older commit offered as newer.
  - T4 `1.0.1-main.144.canary → 1.0.1-rc.1`: the same.
- **C:**
  - `nightly.143 → canary.144` (T1, T2, T3) and `nightly.145 → canary.146`: later commits, lower
    in semver, parterre, dpkg and rpm.
  - It has A's T3 and T4 problems too.
- **X** (ring first, no bump): additionally, `1.0.1-rc.1 → 1.0.1-canary.145` and
  `→ 1.0.1-nightly.145` sort lower.
- **D:**
  - rpm only: `1.0.1-143.nightly → 1.0.1-rc.1` and `1.0.1-144.canary → 1.0.1-rc.1`.
  - It has A's T3 and T4 problems too.
- **A′:** A's problems, and dpkg and rpm order `+canary`/`+nightly` while semver doesn't.
- **E0:** the same as E.

## Sources

Specifications and comparators:

- Semantic Versioning 2.0.0, §9–11 and FAQ: https://semver.org/spec/v2.0.0.html (source:
  https://github.com/semver/semver/blob/master/semver.md)
- Debian Policy Manual 4.7.4.1, §5.6.12 Version:
  https://www.debian.org/doc/debian-policy/ch-controlfields.html#version
- rpm, `rpmio/rpmvercmp.cc`:
  https://github.com/rpm-software-management/rpm/blob/master/rpmio/rpmvercmp.cc
- Windows Installer:
  - ProductVersion property: https://learn.microsoft.com/windows/win32/msi/productversion
  - Upgrade table: https://learn.microsoft.com/windows/win32/msi/upgrade-table
  - VERSIONINFO resource: https://learn.microsoft.com/windows/win32/menurc/versioninfo-resource
- WiX `MajorUpgrade` element: https://docs.firegiant.com/wix/schema/wxs/majorupgrade/
- winget-cli, `src/AppInstallerSharedLib/Versions.cpp`:
  https://github.com/microsoft/winget-cli/blob/master/src/AppInstallerSharedLib/Versions.cpp
- GitHub REST API, release assets ("GitHub renames asset filenames that have special
  characters"): https://docs.github.com/en/rest/releases/assets#upload-a-release-asset

Projects, with the data read on 16 October 2026:

- Rust:
  - Channel manifests: https://static.rust-lang.org/dist/channel-rust-nightly.toml (also `-beta`,
    `-stable`)
  - `src/ci/run.sh`: https://github.com/rust-lang/rust/blob/main/src/ci/run.sh
- Node.js:
  - Download indexes: https://nodejs.org/download/nightly/index.json, …/rc/, …/v8-canary/,
    …/release/
  - Makefile (`DISTTYPE`): https://github.com/nodejs/node/blob/main/Makefile
- Deno:
  - `cli/lib/version.rs`: https://github.com/denoland/deno/blob/main/cli/lib/version.rs
  - `cli/lib/shared.rs`: https://github.com/denoland/deno/blob/main/cli/lib/shared.rs
  - `tools/release/promote_to_release.ts`:
    https://github.com/denoland/deno/blob/main/tools/release/promote_to_release.ts
  - Latest canary and release: https://dl.deno.land/canary-latest.txt,
    https://dl.deno.land/release-latest.txt
- Bun:
  - `src/bun_core/Global.rs`: https://github.com/oven-sh/bun/blob/main/src/bun_core/Global.rs
  - `.buildkite/ci.ts`: https://github.com/oven-sh/bun/blob/main/.buildkite/ci.ts
  - The `canary` release: https://github.com/oven-sh/bun/releases/tag/canary
- Next.js:
  - `npm view next dist-tags`
  - `scripts/start-release.js`:
    https://github.com/vercel/next.js/blob/canary/scripts/start-release.js
- React:
  - `npm view react dist-tags`
  - `ReactVersions.js`: https://github.com/facebook/react/blob/main/ReactVersions.js
  - `scripts/release/README.md`:
    https://github.com/facebook/react/blob/main/scripts/release/README.md
  - `scripts/rollup/build-all-release-channels.js`:
    https://github.com/facebook/react/blob/main/scripts/rollup/build-all-release-channels.js
- TypeScript: `npm view typescript dist-tags`
- Electron: `npm view electron dist-tags`, `npm view electron-nightly dist-tags`
- Neovim:
  - The `nightly` release: https://github.com/neovim/neovim/releases/tag/nightly
  - `CMakeLists.txt` and `cmake/GenerateVersion.cmake`: https://github.com/neovim/neovim
- VS Code:
  - Update API: https://update.code.visualstudio.com/api/update/win32-x64-user/insider/latest
    (and `/stable/latest`)
  - https://code.visualstudio.com/insiders/
- Chrome, version history API:
  https://versionhistory.googleapis.com/v1/chrome/platforms/win64/channels/{canary,dev,beta,stable}/versions
  and `…/channels/all/versions/all/releases?filter=version=…`
- Firefox: https://product-details.mozilla.org/1.0/firefox_versions.json

Version tools:

- GitVersion:
  - Modes: https://github.com/GitTools/GitVersion/tree/main/docs/input/docs/reference/modes
  - Configuration: https://github.com/GitTools/GitVersion/blob/main/docs/input/docs/reference/configuration.md
- MinVer README: https://github.com/adamralph/minver
- Nerdbank.GitVersioning:
  - `docfx/docs/versionJson.md`:
    https://github.com/dotnet/Nerdbank.GitVersioning/blob/main/docfx/docs/versionJson.md
  - `docfx/docs/public-vs-stable.md`:
    https://github.com/dotnet/Nerdbank.GitVersioning/blob/main/docfx/docs/public-vs-stable.md
- semantic-release, workflow configuration:
  https://semantic-release.gitbook.io/semantic-release/usage/workflow-configuration
- changesets, snapshot releases:
  https://github.com/changesets/changesets/blob/main/docs/snapshot-releases.md

parterre:

- `crates/parterre/src/version.rs` and `crates/parterre/build.rs`
- `crates/parterre-telemetry/src/update.rs` and `github.rs`
- `packaging/linux/build-packages.sh` and `metainfo.sh`
- `packaging/windows/build-msi.ps1` and `parterre.wxs`
- `docs/releasing.md`
- The tags `v0.2.0` … `v1.0.0`
- Earlier research: #391–#396, #398 and #400
