# What canary and nightly builds would cost CI

Research note for [#391](https://github.com/aquamoth/parterre/issues/391), part of
[#390](https://github.com/aquamoth/parterre/issues/390): *what CI and `release.yml` build today
and how long they take, what one release binary per target after every merge to `main` would
add, whether the `main` CI run could produce those binaries, whether a nightly or canary
workflow can reuse them as artifacts and for how long, what the package install checks cost,
and which GitHub Actions limits matter.*

Sources: the workflows at `main` a163d94 (`.github/workflows/ci.yml`, `release.yml`,
`linux-packages.yml`, `chocolatey.yml`), `docs/releasing.md`, `Cargo.toml`, the packaging
scripts, the run history from the GitHub API (queried 2026-10-16), and GitHub's docs. Run
figures are wall-clock job durations (`started_at` to `completed_at`) from
`gh api repos/aquamoth/parterre/actions/runs/<id>/jobs`. A statement that is my own conclusion
is marked **(derived)**; an estimate not backed by a measured run is marked **(estimate)**.

## TL;DR

- **Runner minutes are free here, and not the constraint.** The repository is public, and
  standard GitHub-hosted runners, macOS included, are "free and unlimited on public
  repositories" ([runners reference][runners]). The timing API reports 0 billable ms for every
  job of the v1.0.0 release run, macOS ones included (§5). What can bite: **5 concurrent macOS
  jobs**, **20 concurrent jobs in all**, and the **10 GB Actions cache**, which is already full
  (10.14 GB, §4.3).
- **CI today** builds no release binaries: it runs fmt, clippy and the tests in the dev/test
  profile on Linux (Ubuntu 22.04 container), Windows and macOS (arm64), with `rust-cache`. With
  a warm cache, a `main` push costs about **4 min Linux, 10 min Windows, 5–6 min macOS**
  (~20 runner-min, ~10 min wall). A Cargo.lock or toolchain change costs about 14 / 16 / 9.
- **`release.yml` today** builds 4 targets from cold (no cache), each **10–16 min**. Of that,
  the first `cargo build --release` is **5–9.5 min**; the rest is mostly **rebuilding parterre
  per package channel** (`PARTERRE_CHANNEL`): +6–7 min Linux (deb, rpm), +4.5 min per macOS
  target (dmg), +2.5–4 min Windows (msi). One binary per platform with a cheap channel marker
  removes those rebuilds: about **17–20 of the release run's ~57 build runner-minutes** (§2).
- **One release binary per target after every merge** would cost about **33–40 runner-min per
  merge, ~10–12 min wall, two of the four jobs on macOS** (estimate from the first-build step
  plus setup and archiving). Over the last 30 days `main` took **125 pushes** (up to 24 in a
  day), so ~4,000–5,000 runner-min a month, ~2,300 of them macOS. Free, but each merge would
  hold **3 macOS runners** at once instead of 1.
- **The `main` CI run can't produce them for free.** The test jobs compile the dev/test profile;
  the release profile (`lto = "thin"`, `codegen-units = 1`, line tables) shares no compiled
  output with it, so it is a second full compile whichever job runs it. Putting it in the test
  job saves only ~1 min of setup per OS, lengthens main CI's critical path (Windows ~10 →
  ~18 min), and still needs a 4th build for Intel macOS (§3) **(derived)**.
- **Reusing them later works.** `actions/download-artifact@v8` downloads from another run with
  `run-id` and a `github-token` with `actions: read`; release.yml's `CI passed` job already
  finds main's CI run for a commit by SHA. Artifacts are kept **90 days by default here, 1–90
  configurable for a public repo**, per upload with `retention-days` (§4).
- **Package install checks cost ~6–8 runner-min per run, ~1–2 min wall**: 5 Linux containers
  (0.5–1.7 min each), 2 Windows (0.2–0.4), macOS arm64 (0.5) and Intel (0.3–2.1, on
  `macos-26-intel`). Packaging itself without the rebuilds is cheap: the packaging scripts
  already take a prebuilt binary (`cargo deb --no-build`) (§2.3).
- **A nightly that reuses canary artifacts** would cost roughly **15–20 runner-min** (package
  from the downloaded binaries, install checks, publish), and only on days `main` moved: 19 of
  the last 30 **(estimate)**. A night `main` didn't move costs one ~10 s Linux job.

## 1. What CI builds today (`ci.yml`)

**Triggers:** `push` to `main` and every `pull_request`. PR runs are cancelled by a newer push
to the same PR (`cancel-in-progress` only for `pull_request`); `main` runs are never cancelled.
Pushes to `main` run so that they seed the Rust cache PR runs restore from (`ci.yml` header).

**Jobs:**

- `Changes` (ubuntu-latest, ~5 s): decides whether a PR touches anything but Markdown.
- `Test (os)` × 3: `ubuntu-latest` in an `ubuntu:22.04` container, `windows-latest`,
  `macos-latest` (arm64). Each: checkout with full history (`fetch-depth: 0`, for the
  version), stable toolchain with rustfmt and clippy, `Swatinem/rust-cache@v2`,
  `cargo fmt --check`, `cargo clippy --workspace --all-targets`, `cargo check` of each crate
  alone and without default features, `cargo test --workspace`, and on Linux the tests again
  with `core.autocrlf`.
- **Profile:** dev/test only. No `--release`, no `--target`, no artifacts uploaded.

**Durations, 16 `main` push runs (2026-10-06 … 10-16):**

| Job | Typical (warm cache) | Cold (lock/toolchain change: run 38855986407) |
|---|---|---|
| Test (ubuntu, 22.04 container) | 3.5–4.5 min (once 21.1) | 13.9 min |
| Test (windows) | 7.7–11.4 min, median ~9.7 | 16.3 min |
| Test (macos arm64) | 3.3–7.5 min, median ~5.6 | 9.0 min |
| Run wall time | 8–12 min | 16.5 min |

Warm, the steps are: cache restore 0.5–1.3 min, clippy 0.1–0.9, tests 1.8 (Linux), 3.8
(macOS), 6.8–8.1 (Windows). Cold, clippy is 2.5–3.9 min and the tests 4.5–9.3 min, and the
cache save on Linux takes another 1.9 min.

**PR runs (15 runs, 2026-10-08 … 10-16):** the same shape: Linux 3.1–6.3 min, Windows
7.7–14.0, macOS 3.5–6.4 (once 16.0). A Markdown-only PR runs three ~5 s jobs on Linux.

**Volume, last 30 days (2026-09-16 … 10-16):** 125 CI runs on `main` pushes, on 19 distinct
days, at most 24 in one day; 247 PR runs.

## 2. What `release.yml` builds today

**Triggers:** a `v*` tag push (publishes) or `workflow_dispatch` on a branch (builds, packages,
installs and uploads symbols, publishes nothing, #246).

### 2.1 Build jobs

A matrix of 4, **no `rust-cache`** ("caches saved by a tag run are never restored by later
runs, and a release should come from a clean build anyway"):

| Target | Runner | Notes |
|---|---|---|
| `x86_64-unknown-linux-gnu` | `ubuntu-latest`, `ubuntu:22.04` container | glibc 2.35 |
| `x86_64-pc-windows-msvc` | `windows-latest` | |
| `aarch64-apple-darwin` | `macos-latest` | |
| `x86_64-apple-darwin` | `macos-latest` | cross-compiled, so no `--version` check |

Each job: `cargo build --release --locked --target T` stamped with the archive's channel
(`tarball`/`zip`), split debug info (strip=none / `split-debuginfo=packed`), archive with
README, LICENSE, NOTICE and `cargo about` third-party notices, then **one more release build
per package**, each stamped with its own `PARTERRE_CHANNEL`: `deb` and `rpm` on Linux, `dmg`
per macOS target, `msi` on Windows. Linux and macOS upload their symbols to PostHog
(`posthog-cli`, pinned); Windows ships its PDBs as an asset. The packages and archives are
uploaded as one artifact per target.

**Step times** (two runs: v1.0.0 38164838727 and v0.7.0-rc2 37585106342):

| Step | Linux | Windows | macOS arm64 | macOS x86_64 |
|---|---|---|---|---|
| First `cargo build --release` | 5.0–5.7 | 5.7–9.5 | 7.6–7.8 | 7.5–7.8 |
| Package (`cargo about`, archive) | 1.1–1.2 | 1.0–1.5 | 1.1–1.2 | 1.3–1.4 |
| Channel rebuilds + packaging | 6.2–6.9 (deb, rpm) | 2.5–4.0 (msi) | 4.3–4.6 (dmg) | 4.4–4.8 (dmg) |
| Symbols upload | 0.2–0.3 | – | 0.2 | 0.2 |
| **Whole job**, 6 runs | 11.0–23.9, typ. 13.5–14.8 | 9.9–16.5 | 8.5–14.2 | 9.8–15.4 |

The channel rebuilds recompile only `parterre-telemetry` and `parterre` (release.yml comment),
but with thin LTO and one codegen unit the relink is most of a full build's tail: ~3 min each.
Removing them (the map's "one binary per platform") saves **~6.5 + ~3 + 2 × ~4 ≈ 17–20 min
of the build jobs** per release, and on Linux the 2 extra symbol sets **(derived)**.

### 2.2 Gates and publishing

- `CI passed` (ubuntu): polls the Actions API every 30 s for the CI run of the tagged commit on
  `main` and fails unless it succeeded (#236). The tests are not rerun. 0.1 min when CI is
  done, up to 21 min when the tag is pushed right after the merge.
- `Publish` (0.2–0.3 min), `Chocolatey / Package` (0.3–0.5), `Chocolatey / Push` (0.3, behind
  the `chocolatey` environment), `crates.io` (0.2): stable only, not relevant to the new rings.

### 2.3 Package install checks

| Job | Runner | Durations (6 runs) |
|---|---|---|
| Linux packages / Install (debian:12, deb) | ubuntu-latest + container | 0.5–0.6 |
| … (ubuntu:22.04, deb) | | 0.6–1.1 (once 13.8) |
| … (ubuntu:24.04, deb + desktop/AppStream validation) | | 0.7–0.8 (once 7.7) |
| … (fedora:latest, rpm) | | 0.5–0.7 |
| … (opensuse/leap:15.6, rpm) | | 1.1–1.7 |
| Install (Windows, user) / (Windows, machine) | windows-latest | 0.2–0.4 each |
| Install (macOS, aarch64) | macos-latest | 0.4–0.6 |
| Install (macOS, x86_64) | `macos-26-intel` | 0.3–2.1 |

**Per run: ~6–8 runner-min, 9 jobs, ~1–2 min wall** once the packages exist; 2 of the 9 are
macOS. The two outliers (rc3, 37685260481) were apt being slow, not the checks.

The packaging scripts already work from a prebuilt binary: `build-packages.sh` runs
`cargo deb --no-build` and `cargo generate-rpm` on `target/T/release/parterre`, and
`build-dmg.sh` and `build-msi.ps1` likewise expect `cargo build --release` to have run. They
still need a checkout and `cargo about` (~1–1.5 min) **(derived from the scripts)**.

### 2.4 Whole release run

About **57 runner-min of builds + ~7 of install checks + ~1.5 of the rest ≈ 65 runner-min**,
**~17–20 min wall** when CI has finished (v1.0.0 took 58 min wall only because the
Chocolatey push waited for its environment approval).

## 3. A release binary per target after every merge

### 3.1 What it would add

One `cargo build --release --target T` per target, debug info split as release.yml does it,
the archive, and the symbols upload, but **no channel rebuilds**. From §2.1, per merge:

| Target | Job | Of which build |
|---|---|---|
| Linux (22.04 container) | ~7–8 min | 5.0–5.7 |
| Windows | ~8–12 min | 5.7–9.5 |
| macOS arm64 | ~9–10 min | 7.6–7.8 |
| macOS x86_64 (cross) | ~9–10 min | 7.5–7.8 |
| **Total** | **~33–40 runner-min, ~10–12 min wall**, 2 macOS jobs | **(estimate)** |

Skipping `cargo about` and the archive (a canary needs only the binary) would save ~1.2 min
per job. At 125 merges a month: **~4,000–5,000 runner-min**, about 2,300 of them macOS. Free
(§5). At GitHub's list price for a private repository macOS alone would be about $140 a month
($0.062/min, [billing][billing]), which is the scale of what "free" is covering.

**Concurrency is the real cost.** A merge today holds 1 macOS runner (the test job); with
canary builds it would hold 3. GitHub's limit for the account is **5 concurrent macOS jobs**
and **20 jobs** in all ([limits][limits]). On a day like 2026-10-08 (several merges within
minutes, with PR runs alongside) macOS jobs would queue. Today macOS jobs wait a median 9 s
for a runner (max 14 s), Intel macOS 5 s.

Since only the newest canary build is kept (#390), a canary build superseded by a newer merge
is wasted work. A `concurrency` group with `cancel-in-progress: true` for the canary build
bounds a burst to the last merge plus the one running **(derived)**.

### 3.2 Could the `main` CI run produce them instead?

Not without paying for a second compile anyway **(derived)**:

- The test jobs build the dev/test profile (deps at `opt-level = 2`, line tables); the release
  profile is `lto = "thin"`, `codegen-units = 1`, line tables, stripped. Nothing compiled is
  shared, so a release build in the same job is a second full compile: 5–9.5 min more per OS,
  serially, on the run release.yml waits for. Same job saves only checkout, toolchain and
  container setup, ~0.5–1 min per OS.
- The Intel macOS binary is a 4th build that CI has no job for.
- `rust-cache` can't absorb it: the cache is already at **10.14 GB of 10 GB** (10 entries;
  `main`'s three test caches are 0.6–1.5 GB each, a PR's three about 2.2 GB) and GitHub evicts
  the least recently used entries when over the limit ([caching][caching]). Release-profile
  target dirs for 4 targets would evict the test caches PRs restore from. Going over 10 GB is
  billed. This is the same reason release.yml builds cold.
- **`main` runs are never cancelled**, and release.yml's `CI passed` gate needs the CI run of
  the tagged commit to *succeed*. A canary build job inside `ci.yml` that a newer merge
  cancels (job-level `concurrency`) would leave that CI run `cancelled`, so the commit could
  no longer be tagged. A separate workflow keeps the canary build cancellable without touching
  CI's conclusion **(derived)**.
- **PR builds can't stand in for them.** PRs merge with a merge commit (`Merge pull request
  #388 …`), so the commit on `main` is not the PR's `refs/pull/N/merge` commit, and `build.rs`
  stamps the commit into the version. And a PR is pushed several times; only the merge counts.

So the cheapest layout is a **separate canary build after each merge** (on `push` to `main`
in parallel with CI, or on `workflow_run` when CI completes), building each target once,
without cache, and cancelling superseded runs. Running in parallel gives the binaries ~10 min
after the merge; after CI it is ~20 min, but never builds a commit whose tests failed.
Which to pick belongs to the workflows ticket.

### 3.3 One question this leaves for the version ticket

A release binary carries its version at build time: `PARTERRE_RELEASE_TAG` makes `build.rs`
stamp the tag's version and fail unless the tag points at the commit, and without it the
version is `git describe`'s (`1.0.1-dev.3+a1b2c3d`). A binary built after a merge therefore
can't be reused for a later stable or rc tag of the same commit unless the version, too,
becomes something added after the build. The canary and nightly builds of a commit can share
one binary as long as they agree on its version **(derived from `docs/releasing.md` › Version
strings and release.yml)**.

## 4. Reusing the binaries in a later workflow

### 4.1 Downloading another run's artifacts

- `actions/download-artifact@v8` takes `run-id` and `github-token` (and `repository`) "when
  downloading artifacts from a different repository or from a different workflow run"; the
  token needs `actions: read` ([download-artifact][dl]). In the same repository the workflow's
  `GITHUB_TOKEN` with `permissions: actions: read` suffices.
- Finding the run: release.yml already queries
  `repos/$REPO/actions/workflows/ci.yml/runs?head_sha=$SHA&event=push&branch=main`; the same
  query on the canary workflow finds the binaries of `main`'s head.
- `workflow_run` can start a workflow when another completes, at most three levels deep, and
  only for workflow files on the default branch ([events][events]).

### 4.2 How long artifacts are kept

- Default **90 days**; for a public repository configurable **1–90 days**
  ([retention][retention]). This repository: `{"days":90,"maximum_allowed_days":90}`.
- `actions/upload-artifact`'s `retention-days` sets it per upload, within that maximum.
- A nightly reuses the binaries of `main`'s head, which is less than a day old when the nightly
  runs ("only if `main` moved"). A short `retention-days` (2–7) therefore suffices and keeps
  storage small; a binary already published as the canary release is also there as a release
  asset, which doesn't expire **(derived)**.

### 4.3 Storage

- Artifacts per release run today: **~143 MB** (Linux 31 MB, Windows 65 MB of which the PDB
  zip is 44 MB, macOS 22 and 25 MB). A binary-only artifact set is ~50 MB (archives
  11–13 MB each) plus the Windows PDBs.
- The repository holds **717 live artifacts, 7.9 GiB**, far past the Free plan's 500 MB
  "artifact storage", and nothing is billed. GitHub states that quota for private
  repositories ("For private repositories, each GitHub account receives a quota of free
  minutes, artifact storage, and cache storage", [billing][billing]); it doesn't say outright
  that artifact storage is free for public ones, but this repository shows it in practice.
- **The cache is the limited store**: 10 GB per repository, entries unused for 7 days evicted,
  LRU eviction when over ([caching][caching]). Currently 10.14 GB in 10 entries.

## 5. GitHub Actions limits that matter

| Limit | Value | Source |
|---|---|---|
| Standard runners in a public repo | free and unlimited (incl. `macos-latest`, `macos-26-intel`) | [runners][runners], [billing][billing] |
| Billable time of v1.0.0 release run | 0 ms on Ubuntu (18 jobs), macOS (8), Windows (10) | `actions/runs/38164838727/timing` |
| Concurrent jobs (Free plan) | 20, of which at most 5 macOS | [limits][limits] |
| Job / run time | 6 h per job, 35 days per run | [limits][limits] |
| Artifact retention, public repo | default 90 days, 1–90 | [retention][retention] |
| Cache | 10 GB per repo, 7-day idle eviction | [caching][caching] |
| Matrix | 256 jobs per run | [limits][limits] |
| `GITHUB_TOKEN` API | 1,000 requests / hour / repo | [limits][limits] |
| `schedule` | latest commit of the default branch; may be delayed at high load, "the start of every hour" in particular, and queued jobs may be dropped; disabled after 60 days without repository activity in a public repo | [events][events] |
| Larger runners | always billed, even in public repos | [billing][billing] |

Two follow from the `schedule` row **(derived)**: a cron at exactly 03:00 is at the top of
the hour, so a few minutes past (e.g. `17 3 * * *`) is less likely to be delayed or dropped;
and after 60 quiet days the nightly is switched off and has to be re-enabled by hand, which is
harmless since it would have nothing to build.

Intel macOS: the runners page lists `macos-15-intel` and `macos-26-intel` as standard runners
and gives no retirement date. The Intel *build* is cross-compiled on Apple silicon; only the
Intel dmg install check needs an Intel runner.

## 6. Sketch of the cost per ring (derived)

| | Per run | Runs / month (last 30 days) | Runner-min / month |
|---|---|---|---|
| CI on `main` (today) | ~20 runner-min | 125 | ~2,500 |
| CI on PRs (today) | ~20 runner-min | 247 | ~5,000 |
| Canary build, 4 targets, binary only | ~33–40 | ≤125 (fewer with cancellation) | ~4,000–5,000 |
| Nightly from canary artifacts: package + 9 install checks + publish | ~15–20 | ≤19 | ~300–400 |
| Nightly that rebuilds instead (release.yml minus stable publishing, no channel rebuilds) | ~45 | ≤19 | ~850 |
| Release run today | ~65 | a few | – |

The canary build roughly doubles what `main` merges cost and adds ~60% to all CI runner time;
the nightly, if it reuses the canary binaries, is small. All of it is free; the macOS
concurrency limit and the full cache are what to design around.

[runners]: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
[billing]: https://docs.github.com/en/billing/concepts/product-billing/github-actions
[limits]: https://docs.github.com/en/actions/reference/limits
[caching]: https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching
[retention]: https://docs.github.com/en/organizations/managing-organization-settings/configuring-the-retention-period-for-github-actions-artifacts-and-logs-in-your-organization
[events]: https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows
[dl]: https://github.com/actions/download-artifact
