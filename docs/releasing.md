# Releasing

GitHub releases are tag-driven. The tag supplies the version shown by the binary and in the
release filenames. Tag a clean commit on `main`; the version in the root `Cargo.toml` stays
`0.0.0` and is never bumped in the repository.

1. Tag the chosen commit on `main` and push the tag:

   ```sh
   git switch main && git pull
   git tag -a v0.5.0-rc1 -m "parterre 0.5.0-rc1"
   git push origin v0.5.0-rc1
   ```

2. `.github/workflows/release.yml` builds on Linux (in an Ubuntu 22.04 container, so
   the binary runs on glibc 2.35 and newer), Windows and macOS (Apple silicon and Intel), then
   publishes a GitHub Release. The release has one archive per target
   (`parterre-0.5.0-rc1-<target>.tar.gz`, or `.zip` for Windows) and a `SHA256SUMS` file. Each
   archive holds the binary, the README, `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES.html`.
   Windows also gets an installer built from the same files,
   `parterre-0.5.0-rc1-x86_64-pc-windows-msvc.msi` (see
   [building.md](building.md#windows-installer)), and Linux a `.deb` and an `.rpm` from the same
   binary, `parterre_0.5.0-rc1_amd64.deb` and `parterre-0.5.0-rc1-1.x86_64.rpm` (see
   [building.md](building.md#linux-packages)). Before the release is published they are
   installed, run and removed: the `.deb` and `.rpm` on Debian 12, Ubuntu 22.04 and 24.04,
   Fedora and openSUSE Leap 15.6, and the MSI on Windows per user and machine-wide. If any of
   that fails, nothing is published. A tag with a pre-release part goes through all of it and
   publishes a pre-release; its MSI has version `0.5.0`, since MSI versions are numbers only.
   CI builds no packages, so a pre-release (or a run by hand, below) is where packaging is
   first tested.

   The release doesn't run the tests again: it publishes only once the CI run of the tagged
   commit on `main` has passed, and waits for it if it's still running (#236). A commit
   without one, not pushed to `main`, fails the release.

3. The workflow then builds the [Chocolatey](#chocolatey) package from that MSI and pushes it
   unless the tag is a pre-release.

To try a packaging change without tagging, run the release workflow by hand on its branch:

```sh
gh workflow run release.yml --ref my-branch
```

It builds, packages and installs as a release does, but publishes nothing: the packages are
left as artifacts of the run. Their version follows the nearest tag, as in any build
([Version strings](#version-strings)).

The build fails if the tag is not `vX.Y.Z` with an optional pre-release suffix, does not point
at the commit being built, or the sources have local changes. In that case delete the tag
(`git push origin :refs/tags/v0.5.0-rc1`), fix things and tag again.

## crates.io

Cargo requires a package version in `Cargo.toml` and an equal version on each internal
dependency (`parterre-util`, `parterre-core`, `parterre-forge`, `parterre-highlight`). After
the GitHub release workflow passes, use Python 3.11 or newer to generate a separate checkout
from the tag:

```sh
scripts/prepare-crates-release.py v0.5.0-rc1
```

The script prints the checkout path and publish command. It updates the versions and the five
workspace entries in `Cargo.lock`, verifies them with `cargo metadata --locked`, and makes a
local commit so `cargo publish` sees clean sources. The commit exists only in that disposable
checkout; the pushed tag and `main` still point at the same original commit. From the generated
checkout, run `cargo publish --workspace` with a crates.io API token (`cargo login`). Until the
publish job of #20 exists this is done by hand. The first publish of each crate always is.
A version on crates.io can be yanked but never replaced, so check the generated versions before
publishing. The published crate's recorded commit is the local packaging commit; the GitHub
binary reports the tagged source commit.

## Chocolatey

The package `parterre` wraps the MSI of a GitHub Release: `packaging/chocolatey` holds the
nuspec and the install and uninstall scripts, which install it machine-wide (`ALLUSERS=1`) and
depend on the `git` package. `packaging/chocolatey/build.ps1` fills in the version, the MSI's
URL and its SHA-256, and runs `choco pack`.

`.github/workflows/chocolatey.yml` builds the package from a release's MSI and keeps the
`.nupkg` as a workflow artifact. Nothing installs the package itself; the release has already
installed the MSI it wraps, machine-wide as the package does. The release workflow runs it for
every tag, and it can be run by hand for an existing release:

```sh
gh workflow run chocolatey.yml -f tag=v0.5.1 -f push=true
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

## Version strings

`parterre --version` and the foot of the ☰ menu show which build is running:

| Build | Version |
|---|---|
| Release workflow for `v0.5.0-rc1` | `parterre 0.5.0-rc1 (a1b2c3d)` |
| A clean checkout of tag `v0.5.1` | `parterre 0.5.1 (a1b2c3d)` |
| The published crate (`cargo install parterre --version 0.5.0-rc1`) | `parterre 0.5.0-rc1 (b4c5d6e)` |
| A checkout 3 commits past `v0.5.1` | `parterre 0.5.2-dev.3+a1b2c3d` |
| A checkout 3 commits past `v0.5.0-rc1` | `parterre 0.5.0-rc1.dev.3+a1b2c3d` |
| … with uncommitted changes to the sources (`crates/`, Cargo files) | `parterre 0.5.2-dev.3+a1b2c3d.dirty` |
| A git checkout without release tags (e.g. a shallow clone) | `parterre 0.0.0-dev+a1b2c3d` |
| Without git (e.g. from GitHub's source archive) | `parterre 0.0.0-dev` |

The version comes from the release tags, not from `Cargo.toml`: `build.rs` asks
`git describe --tags --match 'v[0-9]*'` for the nearest one. A plain version means a clean build
of a tag. A dev build is a pre-release of the version after the tag it follows, numbered by the
commits since, so it sorts after that release and before the next one: in semver, and in dpkg
and rpm, where the `.deb` and `.rpm` write its `-` as `~`. The commit is semver build metadata.
Only a checkout without release tags (CI checks out the whole history for this reason) or
without git falls back to `Cargo.toml`'s version, which is always `0.0.0` so that it can't pass
for a real one.

The published crate has no `.git`; `cargo package` records its packaging commit in
`.cargo_vcs_info.json`, which `build.rs` reads, and its `Cargo.toml` has the tag's version. Git
only counts when its top level is the workspace root, so sources unpacked inside some other
repository don't take that repository's commit or tags. The release workflow sets
`PARTERRE_RELEASE_TAG` to the tag, which supplies the binary version and makes anything but a
clean build of that tag fail. The logic is in `crates/parterre/src/version.rs`, which
`crates/parterre/build.rs` runs.
