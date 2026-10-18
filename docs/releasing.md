# Releasing

Every build published comes from one release binary per platform, built once for each commit on
`main`, and is published on GitHub in one of four rings (#390):

| Ring | When | Version and tag | Kept |
|---|---|---|---|
| canary | every merge to `main` (`canary.yml`) | `1.2.1-canary`, tag `v1.2.1-canary` | the newest |
| nightly | 03:17 UTC, when `main` has moved since the last (`nightly.yml`) | `1.2.2-nightly.20261016`, tag `v1.2.2-nightly.20261016` | the last 7 |
| rc | the maintainer pushes `v1.2-rcN` (`release.yml`) | `1.2.2-rc`, tag `v1.2.2-rc` | all |
| stable | the maintainer pushes `v1.2` (`release.yml`) | `1.2.2`, tag `v1.2.2` | all |

The maintainer chooses major and minor; the patch is counted ([Version strings](#version-strings)).
Only stable goes beyond GitHub, to crates.io and Chocolatey. Canary, nightly and rc are GitHub
prereleases, never its *Latest*.

## How a build is published

1. **Canary** (`.github/workflows/canary.yml`), on every push to `main`. `build.yml` builds the
   release binary of each target: Linux in an Ubuntu 22.04 container (so it runs on glibc 2.35
   and newer), Windows, and macOS for Apple silicon and Intel. It uploads the Linux and macOS
   symbols to PostHog ([Crash report symbols](#crash-report-symbols)) and keeps each binary,
   with its debug info, as the artifact `binary-<target>` for 14 days. `package.yml` then writes
   the canary version into the binaries (`parterre-stamp`, below) and packages them. Once CI has
   passed on the commit, `publish.yml` publishes the release and deletes the older canary
   builds with their tags. A newer merge cancels the run of an older one.
2. **Nightly** (`.github/workflows/nightly.yml`), at 03:17 UTC: if `main` has moved since the
   newest nightly build, its commit is published as one. It reuses that commit's canary
   binaries, or builds them when there are none (the canary run was cancelled, or its artifacts
   expired), writes the nightly version in, packages, runs the install checks below and
   publishes once CI has passed. The last 7 nightly builds are kept.
3. **Release candidate or release**: tag a commit on `main` with the minor and push the tag.

   ```sh
   git switch main && git pull
   git tag -a v1.2-rc1 -m "parterre 1.2 rc1"     # or v1.2 for the release
   git push origin v1.2-rc1
   ```

   `.github/workflows/release.yml` counts the commit's version (`1.2.2-rc`, or `1.2.2`), reuses
   its canary binaries (or builds them), writes the version in, packages, runs the install
   checks and, once CI has passed on the commit, makes the tag `v1.2.2-rc` (or `v1.2.2`) on the
   same commit and publishes the release on it. A release then goes to [crates.io](#cratesio)
   and [Chocolatey](#chocolatey); a release candidate's Chocolatey package is built but not
   pushed. The tag must name the minor the commit is in: `v1.2` on a commit that already has
   `v1.3-rc1` in its history fails. A commit can have one build of each ring, so a second rc tag
   on the same commit fails too; tag a later commit.

Every release has, for each target, the archive (`parterre-1.2.2-<target>.tar.gz`, or `.zip` on
Windows) holding the binary, the README, `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES.html`; the
Windows installer (`parterre-1.2.2-x86_64-pc-windows-msvc.msi`, see
[building.md](building.md#windows-installer)), the Windows PDB
(`parterre-1.2.2-x86_64-pc-windows-msvc-pdb.zip`), the `.deb` and `.rpm`
(`parterre_1.2.2_amd64.deb`, `parterre-1.2.2-1.x86_64.rpm`, see
[building.md](building.md#linux-packages)), a disk image per macOS target
(`parterre-1.2.2-aarch64-apple-darwin.dmg`, see [building.md](building.md#macos-app)), and
`SHA256SUMS`. The packages wrap the same binary; each but the archive adds its marker, a file
`channel`, which tells parterre how it was installed ([distribution.md](distribution.md#update-check)).
Release titles say the ring (and a nightly build's date), and GitHub's generated notes start
from the ring's previous build: a release candidate's from the previous release candidate or
release.

**Install checks.** Nightly builds, release candidates and releases are installed, run and
removed before they are published: the `.deb` and `.rpm` on Debian 12, Ubuntu 22.04 and 24.04,
Fedora and openSUSE Leap 15.6, the MSI on Windows per user and machine-wide, and the disk images
on Apple silicon and Intel Macs, each checking its marker (`install.yml`). If any of that fails,
nothing is published. Canary builds aren't install-checked.

**No tests again.** A published build doesn't run the tests: it is published only once the CI
run of its commit on `main` has passed, and waits for it if it's still running (#236). A commit
without one, not pushed to `main`, fails.

**`parterre-stamp`** (`crates/parterre-stamp`, not published) is what the workflows run:
`parterre-stamp version --ring rc` prints the version a build of `HEAD` is published under, and
`parterre-stamp write BINARY VERSION` writes it into a built binary, into its 64-byte version
slot and, on Windows, its version resource, in place: nothing moves, so the debug IDs stay
those the symbols were uploaded with (#400). Mac binaries are signed ad hoc again after. Each
package then takes its version from the binary.

To try a packaging change before merging, run the canary or release workflow by hand on its
branch:

```sh
gh workflow run canary.yml --ref my-branch     # builds and packages
gh workflow run release.yml --ref my-branch    # also runs the install checks
```

Either builds and packages as a published build does (`release.yml` as a release candidate of
the branch's commit), and uploads the symbols, but publishes nothing: the packages are left as
artifacts of the run.

## Crash report symbols

The shipped binaries are stripped, so the crash reports they send to PostHog (#263) hold bare
addresses. PostHog resolves them to functions, files and lines with the debug info of the
binary that crashed, which `build.yml` uploads (#265), following [PostHog's Rust
guide](https://posthog.com/docs/error-tracking/upload-source-maps/rust):

- Release builds have line tables (`debug = "line-tables-only"` in `Cargo.toml`). A local
  `cargo build --release` strips them with the symbols, as before. The workflow keeps them:
  - **Linux:** it builds with `CARGO_PROFILE_RELEASE_STRIP=none`, then
    `packaging/linux/split-debuginfo.sh` moves the debug info to `parterre.debug` (`objcopy
    --only-keep-debug`) and strips the binary. Both keep the GNU build ID that PostHog matches
    them by.
  - **macOS:** with `CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO=packed`, rustc writes
    `parterre.dSYM` before it strips the binary, which keeps its `LC_UUID`.
  - **Windows:** with `CARGO_PROFILE_RELEASE_STRIP=none`, the linker writes `parterre.pdb`.
- **One binary, one upload.** Every package and every ring of a commit reuse the same binary,
  so it has one set of debug info, uploaded once when it is built: three symbol sets per build
  (Linux, and each macOS target). They are filed under the release `parterre` and the commit's
  full hash, which holds for every copy: PostHog labels a crash with the release of the symbols
  it used, and refuses a binary's symbols again under another version (#394, #400). Which build
  crashed is the event's `$app_version` and `ring`. A failed upload fails the build, so nothing
  is published from it. A run by hand uploads too.
- PostHog deletes a symbol set nothing has used for 30 days (#394).
- **Windows:** PostHog doesn't take PDBs yet, so Windows reports show the panic message and its
  `file:line` only. The PDB is a release asset instead,
  `parterre-<version>-x86_64-pc-windows-msvc-pdb.zip`, listed in `SHA256SUMS` like the rest.

The upload uses two repository secrets, set up for PostHog (#272):

| Secret | What |
|---|---|
| `POSTHOG_CLI_API_KEY` | A PostHog personal API key with the *Source map upload* preset (error tracking write, organization read) |
| `POSTHOG_CLI_PROJECT_ID` | The ID of parterre's PostHog project |

Without them, in a fork, the upload is skipped with a warning and the build goes on.

To check that a build's symbols arrived:

1. The log of each `build.yml` job's *Upload symbols to PostHog* step lists the file with its
   debug ID and ends with an upload summary: `parterre.debug` on Linux, a dSYM on macOS.
2. In PostHog, *Error tracking › Configuration › Symbol sets* lists them, under the release
   `parterre` and the commit.
3. A panic from the build, with crash reports ticked, shows functions and `file:line` in its
   stack trace instead of addresses. A rebuild has another build ID, so only the published
   files count.

## crates.io

Cargo requires a package version in `Cargo.toml` and an equal version on each internal
dependency (`parterre-util`, `parterre-core`, `parterre-forge`, `parterre-highlight`,
`parterre-telemetry`). `scripts/prepare-crates-release.py` (Python 3.11 or newer) generates a
separate checkout of a tag with those versions:

```sh
scripts/prepare-crates-release.py v1.2.2
```

The script prints the checkout path and publish command. It updates the versions and the six
workspace entries in `Cargo.lock`, verifies them with `cargo metadata --locked`, and makes a
local commit so `cargo publish` sees clean sources. The commit exists only in that disposable
checkout; the pushed tag and `main` still point at the same original commit. The published
crate's recorded commit is the local packaging commit; the GitHub binary reports the tagged
source commit.

Once a GitHub release is published, the release workflow's `crates.io` job runs the script on
the release's tag (`v1.2.2`, which it fetches) and `cargo publish --workspace` from the checkout
it makes, leaving out `parterre-stamp` (`publish = false`). Release candidates aren't published. It
signs in with [Trusted Publishing](https://crates.io/docs/trusted-publishing): crates.io gives
`release.yml`, running in the `crates-io` environment, a token for that run only, so none is
stored. Crates already published at the tag's version are skipped, so a failed job is simply
rerun.

Trusted Publishing can't create a crate, so a new crate's first version is published by hand.
The job stops before publishing anything when a crate is new, and names it. Run the script,
then `cargo publish --workspace` from the checkout it prints, with an API token
(`cargo login`). Then add a trusted publisher to each new crate's settings on crates.io: owner
`aquamoth`, repository `parterre`, workflow file `release.yml`, environment `crates-io`.
Rerunning the job publishes any crates left.

A version on crates.io can be yanked but never replaced, so a broken one is fixed with a new
release.

## Chocolatey

The package `parterre` wraps the MSI of a GitHub Release: `packaging/chocolatey` holds the
nuspec and the install and uninstall scripts, which install it machine-wide (`ALLUSERS=1`) and
depend on the `git` package. `packaging/chocolatey/build.ps1` fills in the version, the MSI's
URL and its SHA-256, and runs `choco pack`.

`.github/workflows/chocolatey.yml` builds the package from a release's MSI and keeps the
`.nupkg` as a workflow artifact. Nothing installs the package itself; the release has already
installed the MSI it wraps, machine-wide as the package does. The release workflow runs it for
every release and release candidate, and it can be run by hand for an existing release:

```sh
gh workflow run chocolatey.yml -f tag=v1.2.2 -f push=true
```

Pushing uses the `CHOCOLATEY_API_KEY` secret of the `chocolatey` environment, the API key of
the Chocolatey account named in the nuspec's `owners`
(<https://community.chocolatey.org/account>). Set it once:

```sh
gh api -X PUT repos/aquamoth/parterre/environments/chocolatey
gh secret set CHOCOLATEY_API_KEY --env chocolatey
```

Without the secret, the push fails and the rest of the release is unaffected. Required reviewers
on the environment would make every push wait for an approval. Every version is checked by
Chocolatey's automated validator and verifier; the first ones also by a human moderator, which
can take weeks. A version on Chocolatey can't be replaced once approved, so a broken one is
fixed with a new release.

## Screenshots

The README, the crates.io README and the AppStream metadata
(`packaging/linux/se.trustfall.parterre.metainfo.xml`) show screenshots from
`docs/images/<major>.<minor>/`, the version of parterre they were taken of, linked through
`raw.githubusercontent.com/…/main/`. Packages and crates already published keep linking to
their folder, so never move or delete one: take new screenshots into a new folder, and point
the three at it.

`scripts/readme-images.sh OUT` takes them all, light and dark, of the same demo repository.
[docs/images/README.md](images/README.md) says why each version gets a folder of its own, and
how to make one.

## Version strings

`parterre --version` and *About parterre* show which build is running:

| Build | Version |
|---|---|
| Canary build | `parterre 1.2.1-canary+a1b2c3d` |
| Nightly build | `parterre 1.2.2-nightly.20261016+a1b2c3d` |
| Release candidate | `parterre 1.2.2-rc+a1b2c3d` |
| Release | `parterre 1.2.2+a1b2c3d` |
| The published crate (`cargo install parterre`) | `parterre 1.2.2+b4c5d6e` |
| Any other build of a checkout, a dev build | `parterre 1.2.3-dev+a1b2c3d` |
| … with uncommitted changes to the sources (`crates/`, Cargo files) | `parterre 1.2.3-dev+a1b2c3d.dirty` |
| A git checkout without version tags (e.g. a shallow clone) | `parterre 0.0.0-dev+a1b2c3d` |
| Without git (e.g. from GitHub's source archive) | `parterre 0.0.0-dev` |

The version comes from the tags, not from `Cargo.toml` (#397). The maintainer chooses major and
minor by pushing `vX.Y-rcN` or `vX.Y`; the patch is the number of commits on the first-parent
line since the oldest tag of the highest minor in the commit's history (`git rev-list --count
--first-parent`), so every commit on `main` has its own patch within a minor, and a new minor
starts at 0. The tags from before this scheme (`v1.0.0`, `v1.0.0-rc1`) count as the maintainer's;
the ones the pipeline makes for canary and nightly builds and release candidates
(`v1.2.1-canary`, `v1.2.2-nightly.20261016`, `v1.2.2-rc`) never do. The pre-release word names
the ring, and for one commit `canary` < `nightly.…` < `rc` < the release, in semver, dpkg and rpm;
the patch orders different commits. The commit is semver build metadata. The `.deb` and `.rpm`
write the `-` as `~`, and the MSI and the Mac app take `X.Y.Z` only. Known and accepted: patch
numbers jump with the merges (`1.2.0` then `1.2.7`).

Every build is a dev build: `crates/parterre/build.rs` runs `crates/parterre/src/version.rs` and
puts the result in the binary's version slot (`src/slot.rs`), which the pipeline later writes the
published version into. A build whose slot was never written is a dev build, or the published
crate, which has no `.git`: `cargo package` records its packaging commit in
`.cargo_vcs_info.json`, which `build.rs` reads, and its `Cargo.toml` has the release's version.
Git only counts when its top level is the workspace root, so sources unpacked inside some other
repository don't take that repository's commit or tags. Only a checkout without version tags (CI
checks out the whole history for this reason) or without git falls back to `Cargo.toml`'s
version, which is always `0.0.0` so that it can't pass for a real one.
