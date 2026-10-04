# Which git versions parterre supports, and how to test them

Research note for [#166](https://github.com/aquamoth/parterre/issues/166): *which is the oldest
git that parterre works with, which git versions are still in use, and how CI can test parterre
against old git versions and the newest one.*

Sources are git's own release notes and documentation at pinned tags, vendor package indexes and
lifecycle pages, GitHub's docs, and experiments I ran. A statement that is my own conclusion is
marked **(derived)**. A statement I could not check is marked **(unverified)**. A statement I
checked by running a command is marked **(tested 2026-10-04: `command`)**. The experiments ran
on Ubuntu 24.04 with git built from the release tarballs on kernel.org (§7).

## TL;DR

- **The minimum today is git 2.31.0** **(derived, tested)**. Three features set it, all from
  2.31:
  - `diff-tree --diff-merges=first-parent`: the changed files of a commit.
  - `rev-parse --path-format=absolute`: the branch catalogue and the worktree folder check.
  - `locked` and `prunable` lines in `worktree list --porcelain`.

  With git 2.31.8, all 747 workspace tests pass. With 2.30.9, 36 tests in parterre-core fail
  (§3, §7). The Chocolatey package already declares `git >= 2.31.0`.
- **Newer features have fallbacks**, so they don't raise the floor: `worktree list -z` (2.36)
  and `fmt-merge-msg --into-name` (2.35). The `merge.rs` comment says the second is from 2.38;
  it is from 2.35 (§3.3).
- **Below 2.31, parterre fails silently, not with a clear error.** git 2.30 prints
  `--path-format=absolute` back as a line of output and exits 0
  (**tested 2026-10-04**: `git-2.30.9 rev-parse --path-format=absolute --git-common-dir`).
  parterre takes that as a path. In one case it then created a junk `info/exclude` under its
  own working directory. A version check at start-up would turn all of this into a clear
  message (§3.4).
- **Oldest gits still in use** (§4):
  - Ubuntu 20.04: 2.25.1 (paid ESM only).
  - Debian 11: 2.30.2 (free LTS ended 2026-08-31).
  - Ubuntu 22.04: 2.34.1 (standard support to May 2027).
  - Debian 12 and Apple's git: 2.39.5.
  - RHEL 8: 2.43.7.
  - Everything else ships 2.47 or newer.

  The Linux release binary needs glibc 2.35, which every distro with a git older than 2.34
  lacks. So on Linux, only a user who builds parterre from source can meet a git below 2.34
  **(derived)**.
- **Recommendation (derived):** keep 2.31 as the documented minimum and test it in CI. Raise it
  to 2.39 when Ubuntu 22.04's standard support ends (May 2027). That is the same date as the
  glibc baseline in `docs/distribution.md`, and it lets the two fallbacks go.
- **CI (derived), see §6:**
  - **On every PR.** The existing Linux leg (`ubuntu:22.04` container) gains three steps. It builds
    git from the kernel.org tarballs, caches each build with `actions/cache` by version, and
    re-runs the tests it has already built once per git on `PATH`:
    - 2.31.8, the floor;
    - 2.39.5, Debian 12 / Apple and the next floor;
    - the latest release.

    That adds about 20–30 s per uncached build and the test time per version. It is free for a
    public repository.
  - **Weekly, on a schedule.** git's `master` and `next`, and `next` built with
    `WITH_BREAKING_CHANGES=YesPlease` (a preview of Git 3.0).
  - **Already covered:** the existing matrix tests 2.34.1 (the container's git) and the runners'
    current git (2.55) on Windows and macOS.
  - **No setup action needed.** There is no maintained action that installs a chosen git.
- **Git 3.0 will break bare repositories in parterre** **(tested 2026-10-04)**. Filed as
  [#228](https://github.com/aquamoth/parterre/issues/228).
  - What happens: a 2.56.0 built with `WITH_BREAKING_CHANGES` defaults `safe.bareRepository` to
    `explicit`, so `git -C <bare repo> rev-parse` fails. parterre's `-C` then reports
    "not a git repository".
  - Fix: run git in a bare repository with `--git-dir` or `GIT_DIR`.
  - Also: one test writes `.git/refs/heads/<b>.lock` by hand, which fails under the new reftable
    default.
  - The other 745 tests pass (§6.6, §7).
- **Builds that need care:**
  - Since 2.55, git builds Rust by default, so a plain `make` needs `cargo` or `NO_RUST=1`.
  - gcc 15 (Ubuntu 26.04) can't build old tags without `-std=gnu17` **(unverified)**. Pin the
    build image rather than using `ubuntu-latest`.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| RN | git release notes, at `v2.56.0` @ `a0189536` (2026-09-28) | https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/ |
| GITDOC | git manual pages at the tag named in each row | https://github.com/git/git/blob/<tag>/Documentation/ |
| KORG | git release tarballs | https://mirrors.edge.kernel.org/pub/software/scm/git/ |
| SEC | git's security policy | https://github.com/git/git/blob/master/SECURITY.md |
| UBU | Ubuntu packages and release cycle | https://packages.ubuntu.com/search?keywords=git&searchon=names&exact=1&suite=all&section=all, https://ubuntu.com/about/release-cycle |
| DEB | Debian packages, releases, LTS | https://packages.debian.org/search?keywords=git&searchon=names&exact=1&suite=all&section=all, https://www.debian.org/releases/, https://wiki.debian.org/LTS |
| RHEL | Red Hat lifecycle API, Rocky and Alma mirrors | https://access.redhat.com/product-life-cycles/api/v1/products?name=Red%20Hat%20Enterprise%20Linux, https://dl.rockylinux.org/pub/rocky/, https://repo.almalinux.org/almalinux/ |
| BREAK | git's planned Git 3.0 changes, at `v2.56.0` | https://github.com/git/git/blob/v2.56.0/Documentation/BreakingChanges.adoc |
| MAINT | git's maintainer notes: branches and cadence | https://github.com/git/git/blob/todo/MaintNotes, https://github.com/git/git/blob/v2.56.0/Documentation/howto/maintain-git.adoc |
| RUNNERS | GitHub Actions runner images | https://github.com/actions/runner-images/tree/main/images |
| GHDOCS | GitHub Docs (read 2026-10-04) | https://docs.github.com/en/ |
| GFW | Git for Windows releases and snapshots | https://github.com/git-for-windows/git/releases, https://gitforwindows.org/git-snapshots/ |

---

## 1. How parterre runs git (context)

- **One runner for all git calls.** Every call in the app goes through `Git::command`
  ([git.rs:78](../../crates/parterre-core/src/git.rs#L78)). It runs
  `git -C <dir> -c core.quotepath=off -c log.showSignature=false
  -c i18n.logOutputEncoding=UTF-8 -c color.ui=false …` with `GIT_OPTIONAL_LOCKS=0` and
  `LC_ALL=C`.
- **Operations that change the repository** use `operation_command`
  ([git.rs:114](../../crates/parterre-core/src/git.rs#L114)). It turns the locks back on, keeps
  the user's locale, and sets `GIT_TERMINAL_PROMPT=0` and `GIT_EDITOR=:`.
- **Other crates** reach git through `Git::run` / `Git::query`. That includes the forge crate;
  its only other program is `gh`.
- **Which `git`:** plain `git` from `PATH`. On Windows, if it isn't on `PATH`, parterre falls
  back to Git for Windows' `cmd\git.exe`
  ([program.rs](../../crates/parterre-core/src/git/program.rs)). There is no setting to choose
  another git.
- **The test helpers spawn `git` from `PATH` too**
  ([tests/common/mod.rs:42](../../crates/parterre-core/tests/common/mod.rs#L42),
  [app/tool_harness.rs:17](../../crates/parterre/src/app/tool_harness.rs#L17)). So in a test
  run, parterre's git and the helpers' git are always the same program **(derived)**. The
  helpers set `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`, and run
  `git init -b main`.
- **`build.rs`** runs git only to work out the version string, at build time.
- **Not git commands:** `watch.rs` and the "operation in progress" checks in `branches.rs` read
  git's files directly: `HEAD`, `packed-refs`, `reftable/`, `rebase-merge/`, `sequencer/`,
  `MERGE_HEAD`, the worktrees' `gitdir`. They depend on git's on-disk layout, not on its
  options.

## 2. Inventory of git invocations

"Since" is the first git release that has the feature, with its source.
- "RN x.y.z:L" is a line of that release's notes at the pinned tag.
- "doc vX" means the option first appears in the manual page at tag vX (checked at the previous
  and the following tag).
- Rows marked "old" predate 2.25.5, the oldest git I ran. That was enough to show they don't set
  the floor, so I didn't look up their exact version.

### 2.1 Global options and environment

| Feature | Where | Since |
|---|---|---|
| `git -C <dir>` | [git.rs:84](../../crates/parterre-core/src/git.rs#L84), [merge.rs:163](../../crates/parterre-core/src/merge.rs#L163) | 1.8.5 ([RN 1.8.5:115](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.5.adoc#L115)) |
| `git -c name=value` (core.quotepath, log.showSignature, i18n.logOutputEncoding, color.ui) | [git.rs:86-89](../../crates/parterre-core/src/git.rs#L86-L89) | 1.7.2 ([RN 1.7.2:34](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.7.2.adoc#L34)); `log.showSignature` 2.10 ([RN 2.10.0:70](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.10.0.adoc#L70)), older gits ignore unknown config |
| `--no-optional-locks`, `GIT_OPTIONAL_LOCKS` | [git.rs:92](../../crates/parterre-core/src/git.rs#L92), [git.rs:556](../../crates/parterre-core/src/git.rs#L556) | 2.15 ([RN 2.15.0:99](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.15.0.adoc#L99)) |
| `--literal-pathspecs` | [git.rs:720](../../crates/parterre-core/src/git.rs#L720), [git.rs:745](../../crates/parterre-core/src/git.rs#L745), [git.rs:811](../../crates/parterre-core/src/git.rs#L811) | 1.8.2 (doc [v1.8.2 git.txt](https://github.com/git/git/blob/v1.8.2/Documentation/git.txt)) |
| `--git-dir=.` | [branches.rs:149](../../crates/parterre-core/src/branches.rs#L149) | old |
| `GIT_TERMINAL_PROMPT=0` | [git.rs:117](../../crates/parterre-core/src/git.rs#L117) | 2.3 ([RN 2.3.0:65](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.3.0.adoc#L65)) |
| `GIT_EDITOR` (`:`, or `cp <file>` for a revert message) | [git.rs:118](../../crates/parterre-core/src/git.rs#L118), [revert.rs:437](../../crates/parterre-core/src/revert.rs#L437) | old |
| `GIT_SEQUENCE_EDITOR=cp <file>` (rebase todo) | [rebase.rs:328](../../crates/parterre-core/src/rebase.rs#L328) | old |

### 2.2 Reading the repository

| Command and options | Where | Since |
|---|---|---|
| `rev-parse --is-bare-repository --absolute-git-dir` | [git.rs:231](../../crates/parterre-core/src/git.rs#L231), [rebase.rs:315](../../crates/parterre-core/src/rebase.rs#L315), [revert.rs:424](../../crates/parterre-core/src/revert.rs#L424) | `--absolute-git-dir` 2.13 (doc [v2.13.0 git-rev-parse.txt](https://github.com/git/git/blob/v2.13.0/Documentation/git-rev-parse.txt), absent at v2.12.0) |
| `rev-parse --git-common-dir` (relative output) | [git.rs:253](../../crates/parterre-core/src/git.rs#L253) | usable from 2.13 ([RN 2.13.0:71](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.13.0.adoc#L71)) |
| **`rev-parse --path-format=absolute --git-common-dir`** | [branches.rs:324](../../crates/parterre-core/src/branches.rs#L324) | **2.31** ([RN 2.31.0:37](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.31.0.adoc#L37)) |
| **`rev-parse --path-format=absolute --git-path info/exclude`** | [worktree_folder.rs:137](../../crates/parterre-core/src/worktree_folder.rs#L137) | **2.31** (same) |
| `rev-parse --show-toplevel`, `--is-inside-work-tree`, `-q --verify <rev>^{commit}`, `--short`, `<b>@{upstream}`, `HEAD:<path>` | [git.rs:244](../../crates/parterre-core/src/git.rs#L244), [branches.rs:224](../../crates/parterre-core/src/branches.rs#L224), [branches.rs:1226](../../crates/parterre-core/src/branches.rs#L1226), [merge.rs:279](../../crates/parterre-core/src/merge.rs#L279), [git.rs:770](../../crates/parterre-core/src/git.rs#L770) | old |
| `for-each-ref --format=%(refname)…%(objecttype)%(objectname)%(*objecttype)%(*objectname)%(symref)%(upstream)` (fields split by `\x1f`) | [git.rs:273](../../crates/parterre-core/src/git.rs#L273) | old |
| `for-each-ref --format=…%00…%(upstream:remotename)%00%(upstream:remoteref)` | [branches.rs:244](../../crates/parterre-core/src/branches.rs#L244) | 2.16 ([RN 2.16.0:50](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.16.0.adoc#L50)) |
| `for-each-ref --format=%(refname)%00%(upstream)`, `%(refname)%00%(symref)` | [parterre-forge lib.rs:282](../../crates/parterre-forge/src/lib.rs#L282), [github.rs:229](../../crates/parterre-forge/src/github.rs#L229) | old |
| `worktree list --porcelain` | [git.rs:279](../../crates/parterre-core/src/git.rs#L279) | 2.7 (doc [v2.7.0 git-worktree.txt](https://github.com/git/git/blob/v2.7.0/Documentation/git-worktree.txt)); the code expects it to fail before then |
| **`locked`, `prunable` lines in `worktree list --porcelain`** | [git.rs:1068](../../crates/parterre-core/src/git.rs#L1068), [branches.rs:290](../../crates/parterre-core/src/branches.rs#L290) | **2.31** ([RN 2.31.0:61](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.31.0.adoc#L61)) |
| `worktree list --porcelain -z` | [branches.rs:290](../../crates/parterre-core/src/branches.rs#L290), falls back to the plain form and refuses paths it can't read safely | 2.36 ([RN 2.36.0:110](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.36.0.adoc#L110)) |
| `cat-file --batch-check` (stdin `<ref>^{commit}`), `--batch-check=%(objecttype)` | [git.rs:296](../../crates/parterre-core/src/git.rs#L296), [reset.rs:760](../../crates/parterre-core/src/reset.rs#L760) | format 1.8.4 ([RN 1.8.4:116](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.4.adoc#L116)) |
| `cat-file -s`, `cat-file --textconv <rev>:<path>`, `cat-file blob` | [git.rs:615](../../crates/parterre-core/src/git.rs#L615), [git.rs:636](../../crates/parterre-core/src/git.rs#L636), [reset.rs:809](../../crates/parterre-core/src/reset.rs#L809) | old |
| `symbolic-ref -q HEAD` | [git.rs:308](../../crates/parterre-core/src/git.rs#L308), [branches.rs:226](../../crates/parterre-core/src/branches.rs#L226) | old |
| `log --no-color --no-decorate --date=format-local:%Y-%m-%d %H:%M -z --format=%H%x00%P%x00%T%x00%an%x00%ae%x00%at%x00%ad%x00%ct%x00%s --stdin` | [git.rs:326](../../crates/parterre-core/src/git.rs#L326) | `format-local` 2.7 (doc [v2.7.0 rev-list-options.txt](https://github.com/git/git/blob/v2.7.0/Documentation/rev-list-options.txt), absent at v2.6.0; `format:` alone is [RN 2.6.0:16](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.6.0.adoc#L16)) |
| `log --no-walk=unsorted --format=%h` (abbreviation length) | [git.rs:424](../../crates/parterre-core/src/git.rs#L424) | old |
| `log -1 --format=%ai%x00%cn%x00%ce%x00%ci%x00%B`; `log -1 --no-expand-tabs --format=fuller --notes` | [git.rs:465](../../crates/parterre-core/src/git.rs#L465), [git.rs:479](../../crates/parterre-core/src/git.rs#L479) | `--no-expand-tabs` 2.9 ([RN 2.9.0:17](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.9.0.adoc#L17)) |
| `--literal-pathspecs log --no-follow --topo-order --parents -z --format=… <revs> -- <path>` (file history) | [git.rs:719](../../crates/parterre-core/src/git.rs#L719), formats in [file_history.rs:30](../../crates/parterre-core/src/file_history.rs#L30) | `--no-follow` 1.8.2 ([RN 1.8.2:108](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.2.adoc#L108)) |
| **`diff-tree -r -M --root --diff-merges=first-parent --no-commit-id --no-ext-diff --no-textconv -z --raw --numstat`** | [git.rs:503](../../crates/parterre-core/src/git.rs#L503) | **2.31** ([RN 2.31.0:55](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.31.0.adoc#L55)); 2.30 has the option but not this value (§3.2) |
| `diff-tree -r -M … <old> <new>`; `diff-tree -r --raw -z --no-abbrev`; `diff-tree -r -z --no-commit-id --name-only --root -m --first-parent` | [git.rs:523](../../crates/parterre-core/src/git.rs#L523), [reset.rs:645](../../crates/parterre-core/src/reset.rs#L645), [revert.rs:301](../../crates/parterre-core/src/revert.rs#L301) | old |
| `diff` with `--raw/--numstat/--name-only -z`, `-M`, `--no-renames`, `--no-abbrev`, `-R`, `--cached`, `--quiet`, `--diff-filter=U`, `--textconv -U0` | [git.rs:555](../../crates/parterre-core/src/git.rs#L555), [git.rs:743](../../crates/parterre-core/src/git.rs#L743), [git.rs:809](../../crates/parterre-core/src/git.rs#L809), [reset.rs:646-659](../../crates/parterre-core/src/reset.rs#L646-L659), [branches.rs:428](../../crates/parterre-core/src/branches.rs#L428), [revert.rs:510](../../crates/parterre-core/src/revert.rs#L510) | old |
| `diff-index --cached -M -z --name-status HEAD` | [git.rs:775](../../crates/parterre-core/src/git.rs#L775) | old |
| `hash-object -t tree --stdin`, `hash-object -- <path>` | [git.rs:808](../../crates/parterre-core/src/git.rs#L808), [reset.rs:804](../../crates/parterre-core/src/reset.rs#L804) | old |
| `check-attr -z diff -- <path>`; `config diff.<driver>.textconv` | [git.rs:660](../../crates/parterre-core/src/git.rs#L660) | old |
| `blame --line-porcelain --root [-w] [-M [-C]] [<rev>] -- <path>` | [git.rs:679](../../crates/parterre-core/src/git.rs#L679), [blame.rs:74](../../crates/parterre-core/src/blame.rs#L74) | `--line-porcelain` 1.7.6 ([RN 1.7.6:47](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.7.6.adoc#L47)) |
| `merge-base <a> <b>`, `merge-base --is-ancestor` | [git.rs:541](../../crates/parterre-core/src/git.rs#L541), [merge.rs:541](../../crates/parterre-core/src/merge.rs#L541), [revert.rs:353](../../crates/parterre-core/src/revert.rs#L353), [branches.rs:1237](../../crates/parterre-core/src/branches.rs#L1237) | `--is-ancestor` 1.8.0 ([RN 1.8.0:73](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.0.adoc#L73)) |
| `rev-list` with `--count`, `--left-right`, `--right-only`, `--cherry-mark`, `--cherry-pick`, `--no-merges`, `--merges`, `--reverse`, `--topo-order`, `--parents`, `-n 1`, `--stdin`, `--no-walk=unsorted` | [upstream.rs:207](../../crates/parterre-core/src/upstream.rs#L207), [rebase.rs:166-187](../../crates/parterre-core/src/rebase.rs#L166-L187), [merge.rs:390-398](../../crates/parterre-core/src/merge.rs#L390-L398), [cherry_pick.rs:152-170](../../crates/parterre-core/src/cherry_pick.rs#L152-L170), [reset.rs:318](../../crates/parterre-core/src/reset.rs#L318), [revert.rs:141](../../crates/parterre-core/src/revert.rs#L141), [branches.rs:123](../../crates/parterre-core/src/branches.rs#L123), [branches.rs:1582](../../crates/parterre-core/src/branches.rs#L1582) | `--cherry-mark` 1.7.5 ([RN 1.7.5:74](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.7.5.adoc#L74)); rest old |
| `log -p --no-merges --no-ext-diff --no-textconv --format=commit %H --stdin [--no-walk=unsorted]` piped into `patch-id --stable` | [cherry_pick.rs:270](../../crates/parterre-core/src/cherry_pick.rs#L270), [cherry_pick.rs:297](../../crates/parterre-core/src/cherry_pick.rs#L297) | `--stable` 2.1 (doc [v2.1.0 git-patch-id.txt](https://github.com/git/git/blob/v2.1.0/Documentation/git-patch-id.txt), absent at v2.0.0) |
| `show -s --format=%B`; `show -s --pretty=reference` (only with `revert.reference`) | [revert.rs:267-276](../../crates/parterre-core/src/revert.rs#L267-L276) | `reference` 2.25 ([RN 2.25.0:58](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.25.0.adoc#L58)) |
| `status --porcelain`, `status --porcelain=v1 -z --untracked-files=no/all --ignore-submodules=none` | [rebase.rs:189](../../crates/parterre-core/src/rebase.rs#L189), [merge.rs:553](../../crates/parterre-core/src/merge.rs#L553), [revert.rs:323](../../crates/parterre-core/src/revert.rs#L323), [branches.rs:1513](../../crates/parterre-core/src/branches.rs#L1513) | `=v1` 2.11 (doc [v2.11.0 git-status.txt](https://github.com/git/git/blob/v2.11.0/Documentation/git-status.txt), absent at v2.10.0) |
| `stash list [--format=%gd%x00%gs / %gd%x00%H]`, `stash show --name-only -z <oid>` | [revert.rs:80](../../crates/parterre-core/src/revert.rs#L80), [revert.rs:400](../../crates/parterre-core/src/revert.rs#L400), [revert.rs:529](../../crates/parterre-core/src/revert.rs#L529) | old |
| `ls-files --others --exclude-standard [-z] [-- paths]` | [reset.rs:657](../../crates/parterre-core/src/reset.rs#L657), [revert.rs:502](../../crates/parterre-core/src/revert.rs#L502) | old |
| `read-tree -m -u -n <h> <t>` (dry run of `reset --keep`), `update-index -q --refresh` | [reset.rs:309-313](../../crates/parterre-core/src/reset.rs#L309-L313) | old |
| `config --get`, `config --bool`, `config --type=bool --get` | [merge.rs:520](../../crates/parterre-core/src/merge.rs#L520), [rebase.rs:193](../../crates/parterre-core/src/rebase.rs#L193), [revert.rs:144](../../crates/parterre-core/src/revert.rs#L144), [branches.rs:233](../../crates/parterre-core/src/branches.rs#L233) | `--type` 2.18 ([RN 2.18.0:57](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.18.0.adoc#L57)) |
| `fmt-merge-msg [--no-log]` (stdin `<oid>\t\tbranch '<b>' of .`) | [merge.rs:283](../../crates/parterre-core/src/merge.rs#L283), [merge.rs:359](../../crates/parterre-core/src/merge.rs#L359) | old |
| `fmt-merge-msg --no-log --into-name <branch>`, falling back to `retarget` | [merge.rs:363](../../crates/parterre-core/src/merge.rs#L363) | 2.35 ([RN 2.35.0:93](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.35.0.adoc#L93)) |
| `check-ref-format refs/heads/<name>`; `check-ignore -q -- <dir>/` | [branches.rs:1161](../../crates/parterre-core/src/branches.rs#L1161), [worktree_folder.rs:125](../../crates/parterre-core/src/worktree_folder.rs#L125) | `check-ignore` 1.8.2 ([RN 1.8.2:114](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.2.adoc#L114)) |
| `remote`, `remote -v`, `remote get-url origin` | [branches.rs:238](../../crates/parterre-core/src/branches.rs#L238), [parterre-forge lib.rs:265](../../crates/parterre-forge/src/lib.rs#L265), [github.rs:113](../../crates/parterre-forge/src/github.rs#L113) | `get-url` 2.7 ([RN 2.7.0:13](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.7.0.adoc#L13)) |

### 2.3 Operations (shown in the dialogs, run through `operation_command`)

| Command and options | Where | Since |
|---|---|---|
| `switch --create <b> --no-track <start>`, `switch --detach <oid>`, `switch --no-guess -- <b>`, `switch <b>` | [branches.rs:983](../../crates/parterre-core/src/branches.rs#L983), [branches.rs:1029-1030](../../crates/parterre-core/src/branches.rs#L1029-L1030), [merge.rs:168-170](../../crates/parterre-core/src/merge.rs#L168-L170) | 2.23 ([RN 2.23.0:61](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.23.0.adoc#L61)); "experimental" until 2.51 ([RN 2.51.0:67](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.51.0.adoc#L67)) |
| `branch --no-track -- <b> <start>`, `branch -d/-D -- <b>`, `branch --set-upstream-to=refs/remotes/<r>/<b> -- <b>` | [branches.rs:985](../../crates/parterre-core/src/branches.rs#L985), [branches.rs:1031](../../crates/parterre-core/src/branches.rs#L1031), [branches.rs:1209](../../crates/parterre-core/src/branches.rs#L1209), [branches.rs:1456](../../crates/parterre-core/src/branches.rs#L1456) | `--set-upstream-to` 1.8.0 ([RN 1.8.0:46](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.0.adoc#L46)) |
| `config --local --replace-all branch.<b>.remote/merge/rebase` | [branches.rs:1465-1487](../../crates/parterre-core/src/branches.rs#L1465-L1487) | old |
| `worktree add --no-track -b <b> -- <path> <start>`, `worktree add -- <path> <b>`, `worktree add --detach -- <path> <start>`, `worktree remove [--force] -- <path>` | [branches.rs:1002-1027](../../crates/parterre-core/src/branches.rs#L1002-L1027), [branches.rs:1323](../../crates/parterre-core/src/branches.rs#L1323) | `remove` 2.17 (doc [v2.17.0 git-worktree.txt](https://github.com/git/git/blob/v2.17.0/Documentation/git-worktree.txt), absent at v2.16.0); `--track` is in 2.25.5's `worktree add -h` (**tested 2026-10-04**) |
| `merge --ff-only / --no-ff [--autostash / --no-autostash] [--no-log] -m <msg> <rev>` | [merge.rs:137](../../crates/parterre-core/src/merge.rs#L137), [merge.rs:159](../../crates/parterre-core/src/merge.rs#L159) | `merge --autostash` 2.27 ([RN 2.27.0:87](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.27.0.adoc#L87)) |
| `fetch . <src>:<into>` (fast-forward a branch checked out nowhere) | [merge.rs:173](../../crates/parterre-core/src/merge.rs#L173) | old |
| `rebase [--interactive] [--autostash / --no-autostash] <onto>`, todo words `pick`, `squash`, `drop` | [rebase.rs:83](../../crates/parterre-core/src/rebase.rs#L83), [rebase.rs:48](../../crates/parterre-core/src/rebase.rs#L48) | `--autostash` 1.8.4 ([RN 1.8.4:196](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/1.8.4.adoc#L196)); `drop` 2.6 ([RN 2.6.0:22](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.6.0.adoc#L22)) |
| `cherry-pick [-x] <oids…>` | [cherry_pick.rs:48](../../crates/parterre-core/src/cherry_pick.rs#L48) | old |
| `revert --edit/--no-edit [-m 1] <oid>`, `revert --abort` | [revert.rs:40](../../crates/parterre-core/src/revert.rs#L40), [revert.rs:458](../../crates/parterre-core/src/revert.rs#L458) | old; the `Reapply "…"` wording parterre copies is 2.43 ([RN 2.43.0:63](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.43.0.adoc#L63)), and parterre hands git its own message, so older gits don't matter there |
| `revert.reference` config (read only) | [revert.rs:144](../../crates/parterre-core/src/revert.rs#L144) | 2.37 ([RN 2.37.0:62](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.37.0.adoc#L62); config in doc [v2.37.0 config/revert.txt](https://github.com/git/git/blob/v2.37.0/Documentation/config/revert.txt)) |
| `reset --soft/--mixed/--keep/--hard <oid>` | [reset.rs:65](../../crates/parterre-core/src/reset.rs#L65) | old |
| `stash push -m <msg>`, `stash pop [<entry>]` | [cherry_pick.rs:61](../../crates/parterre-core/src/cherry_pick.rs#L61), [revert.rs:67](../../crates/parterre-core/src/revert.rs#L67), [revert.rs:542](../../crates/parterre-core/src/revert.rs#L542), [branches.rs:1037](../../crates/parterre-core/src/branches.rs#L1037) | `push` 2.13 ([RN 2.13.0:95](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.13.0.adoc#L95)) |

### 2.4 Test helpers only

| Feature | Where | Since |
|---|---|---|
| `init -b main` | [tests/common/mod.rs:30](../../crates/parterre-core/tests/common/mod.rs#L30) | 2.28 (doc [v2.28.0 git-init.txt](https://github.com/git/git/blob/v2.28.0/Documentation/git-init.txt), absent at v2.27.0) |
| `GIT_CONFIG_GLOBAL=/dev/null`; CI's `GIT_CONFIG_SYSTEM=.github/autocrlf.gitconfig` | [tests/common/mod.rs:49](../../crates/parterre-core/tests/common/mod.rs#L49), [ci.yml](../../.github/workflows/ci.yml) | 2.32 ([RN 2.32.0:88](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.32.0.adoc#L88)); older gits ignore both |
| `branch --show-current` | tests | 2.22 ([RN 2.22.0:32](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.22.0.adoc#L32)) |
| `switch -q` (60 calls), `worktree lock`, `notes add`, `pack-refs --all`, `update-ref` | tests | `switch` 2.23 (above) |

## 3. The minimum git version

### 3.1 What sets the floor (derived, tested)

Three features in §2 are newer than 2.30, have no fallback, and all come from 2.31.0:

1. `diff-tree --diff-merges=first-parent`. It lists the changed files of every commit in the
   log, file diff and compare windows. That makes it the most visible of the three.
2. `rev-parse --path-format=absolute`. It finds each worktree's admin folder in the branch
   catalogue, and `info/exclude` in the worktree folder check.
3. `locked` / `prunable` in `worktree list --porcelain`. It feeds the worktree states and the
   checks that keep a locked worktree from being deleted.

So **git 2.31.0 is the floor** **(derived)**. All 747 workspace tests pass on 2.31.8
(**tested 2026-10-04**: `cargo test --workspace` with `/tmp/gitver/git-2.31.8/bin` first on
`PATH`, §7).

### 3.2 What breaks on 2.30 (tested 2026-10-04)

With git 2.30.9, parterre-core and parterre-forge passed 451 tests and failed 36. The failures
fall into these groups:

- 13 tests (log, file diff): `fatal: unknown value for --diff-merges: first-parent`. 2.29 added
  `--no-diff-merges` ([RN 2.29.0:33](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.29.0.adoc#L33)),
  but the `first-parent` value only came in 2.31.
- The other 23 fall into two groups. I read the messages but did not trace each test:
  - **Conflicts and "operation in progress".** In the merge, rebase, cherry-pick and revert
    tests, a stopped operation is reported as a failure instead of "stuck". The likely cause is
    that `rev-parse --path-format=absolute --git-common-dir` on 2.30 prints
    `--path-format=absolute` as an extra first line and exits 0, so the catalogue looks for
    `rebase-merge/`, `MERGE_HEAD` and the rest in the wrong folder **(derived from that output,
    tested)**. The worktree folder check fails the same way through `--git-path`.
  - **Worktree states.** Tests of locked or missing worktrees fail because there are no
    `locked` / `prunable` lines.

### 3.3 Newer features that already have fallbacks

- **`worktree list -z` (2.36).** The viewer always uses the plain form
  ([git.rs:1044](../../crates/parterre-core/src/git.rs#L1044)). The catalogue tries `-z` first,
  then the plain form, and refuses paths with newlines
  ([branches.rs:290-313](../../crates/parterre-core/src/branches.rs#L290-L313)).
- **`fmt-merge-msg --into-name` (2.35, not 2.38 as the comment at
  [merge.rs:366](../../crates/parterre-core/src/merge.rs#L366) says).**
  - Docs: the option is in [v2.35.0 git-fmt-merge-msg.txt](https://github.com/git/git/blob/v2.35.0/Documentation/git-fmt-merge-msg.txt)
    and not in v2.34.0.
  - Built gits: 2.34.8 rejects it and 2.36.6 has it (**tested 2026-10-04**:
    `git fmt-merge-msg -h`).
  - Effect: only the comment is wrong. On 2.35–2.37 the first call succeeds, so no behaviour
    changes.
- **Behaviour differences the code already absorbs:**
  - 2.34 leaves a revert that turns out empty in progress
    ([revert.rs:453](../../crates/parterre-core/src/revert.rs#L453)).
  - Before 2.43, git worded a revert of a revert differently
    ([revert.rs:243](../../crates/parterre-core/src/revert.rs#L243)).
  - Since 2.48, the worktree `.git` file can hold a relative path
    ([watch.rs:88](../../crates/parterre-core/src/watch.rs#L88)).
- On 2.34.8, 2.36.6, 2.39.5, 2.47.3 and 2.56.0, all tests pass (§7).

### 3.4 How parterre behaves below the floor

There is no version check. On 2.30, the graph loads, because `load()` uses nothing newer than
2.30. The first click on a commit then shows `unknown value for --diff-merges`. The branch tool
silently misreads "operation in progress", which matters for the safety checks before
reset/delete **(derived)**.

Worse, `worktree_folder::exclude` writes the garbage path. It takes
`rev-parse --path-format=absolute --git-path info/exclude` as the file to append to
([worktree_folder.rs:137](../../crates/parterre-core/src/worktree_folder.rs#L137)). On 2.30
that output is `--path-format=absolute` followed by the relative `.git/info/exclude`. So
parterre created a folder named `--path-format=absolute<newline>.git/info/` relative to its own working
directory, not the repository's, and wrote the pattern there (**tested 2026-10-04**: the 2.30.9
test run left `crates/parterre-core/--path-format=absolute\n.git/info/exclude` containing
`/trees/`; I deleted it).

A check at start-up would turn that into one clear message, for example "parterre needs git 2.31
or newer; this is 2.30.2". It would parse `git version`, whose first line is
`git version X.Y.Z[.suffix]`, e.g. `2.55.0.windows.5` or `2.39.5 (Apple Git-154)` **(derived)**.

### 3.5 Going lower would cost (derived, unverified)

Supporting 2.30 (Debian 11) would need three fallbacks:

- `-m --first-parent` instead of `--diff-merges=first-parent`. `revert.rs` already uses that
  form for `diff-tree`. git 2.30.9 with it printed the same bytes as 2.31.8 with
  `--diff-merges=first-parent` (**tested 2026-10-04**: the full `changed_files` argument list,
  on parterre's root commit, two merges and two ordinary commits). This one alone would be
  cheap.
- Joining the relative `--git-common-dir` / `--git-path` output to the folder, as
  `Git::git_dirs` already does.
- Locked/prunable worktrees read from `$GIT_COMMON_DIR/worktrees/*/locked` and `gitdir`.

Below 2.28 the test helpers break too: `init -b` fails all 269 repository tests on 2.25.5
(**tested 2026-10-04**). §4.2 explains why it isn't worth it.

## 4. Which git versions are in use (2026-10-04)

### 4.1 By platform

| Platform | git | Support ends | Source |
|---|---|---|---|
| Ubuntu 20.04 | 2.25.1 (+ESM fixes) | standard May 2025; ESM May 2030 | [UBU](https://ubuntu.com/about/release-cycle), [USN-5376-5](https://ubuntu.com/security/notices/USN-5376-5) |
| Ubuntu 22.04 | 2.34.1 (`1:2.34.1-1ubuntu1.17`) | standard May 2027; ESM May 2032 | [UBU](https://packages.ubuntu.com/search?keywords=git&searchon=names&exact=1&suite=all&section=all), [changelog](https://changelogs.ubuntu.com/changelogs/pool/main/g/git/git_2.34.1-1ubuntu1.17/changelog) |
| Ubuntu 24.04 | 2.43.0 | standard May 2029 | [UBU](https://ubuntu.com/about/release-cycle) |
| Ubuntu 26.04 | 2.53.0 | standard May 2031 | [UBU](https://packages.ubuntu.com/search?keywords=git&searchon=names&exact=1&suite=all&section=all) |
| Debian 11 | 2.30.2 (`+deb11u5`) | LTS ended 2026-08-31; paid ELTS to 2031-06-30 | [DEB](https://packages.debian.org/search?keywords=git&searchon=names&exact=1&suite=all&section=all), [LTS](https://wiki.debian.org/LTS), [ELTS](https://wiki.debian.org/LTS/Extended) |
| Debian 12 | 2.39.5 | regular 2026-07-11; LTS to 2028-06-30 | [DEB](https://www.debian.org/releases/) |
| Debian 13 | 2.47.3 | LTS to 2030-06-30 | [DEB](https://www.debian.org/releases/) |
| RHEL / Rocky / Alma 8 | 2.43.7 (`git-2.43.7-1.el8_10`) | maintenance 2029-05-31 | [RHEL API](https://access.redhat.com/product-life-cycles/api/v1/products?name=Red%20Hat%20Enterprise%20Linux), [Rocky 8](https://dl.rockylinux.org/pub/rocky/8/AppStream/x86_64/os/Packages/g/) |
| RHEL 9 | moves per minor release: 9.0 had 2.31.1, 9.8 has 2.52.0 | maintenance 2032-05-31 | [Rocky 9](https://dl.rockylinux.org/pub/rocky/9/AppStream/x86_64/os/Packages/g/), [Rocky 9.0 vault](https://dl.rockylinux.org/vault/rocky/9.0/AppStream/x86_64/os/Packages/g/) |
| RHEL 10 | 2.52.0 (10.0 had 2.47.3) | maintenance 2035-05-31 | [Rocky 10](https://dl.rockylinux.org/pub/rocky/10/AppStream/x86_64/os/Packages/g/) |
| Amazon Linux 2 / 2023 | 2.47.3 (AL2 ended 2026-06-30) / 2.50.1 | AL2023 2029-06-30 | [AL2 FAQ](https://aws.amazon.com/amazon-linux-2/faqs/), [ALAS2023-2025-1108](https://alas.aws.amazon.com/AL2023/ALAS2023-2025-1108.html) |
| Alpine 3.21–3.24 | 2.47.3 – 2.54.0 | 3.21 on 2026-11-01 | [Alpine releases](https://alpinelinux.org/releases/), [pkgs](https://pkgs.alpinelinux.org/packages?name=git&branch=v3.21) |
| Fedora 43 / 44 | 2.55.0 | — | [mdapi f43](https://mdapi.fedoraproject.org/f43-updates/srcpkg/git) |
| openSUSE Leap 15.6 / 16.0, SLES 15 SP7 | 2.51.0 (Leap 15.6 ended 2026-04-30) | SP7 2031-07-31 | [Leap 16.0 repo](https://download.opensuse.org/distribution/leap/16.0/repo/oss/x86_64/), [SUSE-SU-2025:03012-1](https://www.suse.com/support/update/announcement/2025/suse-su-202503012-1/), [SUSE lifecycle](https://www.suse.com/lifecycle/) |
| macOS, Apple's git (Xcode / Command Line Tools) | Apple Git-154 = 2.39.5, Git-155 = 2.50.1; which Xcode ships which is **(unverified)**: Apple's release notes don't say | — | [apple-oss-distributions/Git Git-155](https://github.com/apple-oss-distributions/Git/blob/Git-155/src/git/GIT-VERSION-GEN), [Git-154](https://github.com/apple-oss-distributions/Git/tree/Git-154) |
| macOS, Homebrew | 2.56.0 (latest only) | — | [formulae.brew.sh](https://formulae.brew.sh/formula/git) |
| Git for Windows | 2.56.0.windows.1 (2026-09-28); old installers stay downloadable | — | [releases](https://github.com/git-for-windows/git/releases) |
| Upstream | 2.56.0 (2026-09-28); 2.55.0 2026-06-29, 2.54.0 2026-04-20 | no LTS; fixes on the latest track, critical ones on "at least a couple more" | [KORG](https://mirrors.edge.kernel.org/pub/software/scm/git/), [SEC](https://github.com/git/git/blob/master/SECURITY.md) |
| GitHub-hosted runners | ubuntu-22.04/24.04: 2.55.0 (from the git-core PPA, not the distro); windows-2022/2025: 2.55.0.windows.5; macos-15/26: 2.55.0 | — | [RUNNERS](https://github.com/actions/runner-images/tree/main/images), [install-git.sh](https://github.com/actions/runner-images/blob/main/images/ubuntu/scripts/build/install-git.sh) |

Notes:
- **Distros backport fixes instead of upgrading.** Ubuntu and Debian keep the base version's
  features for the life of the release (22.04 is still 2.34.1). RHEL 9 and 10 move git forward
  at each minor release, so a user pinned to an old EUS minor keeps its older git.
- Some vendor pages lag one another. For example, debian.org/releases still calls bullseye "LTS"
  after the wiki moved it to ELTS.

### 4.2 Who can actually meet an old git (derived)

- **Linux.** The release binary needs glibc 2.35 or newer
  ([docs/distribution.md](../distribution.md#linux)). These can't run it:
  - Debian 11, glibc 2.31 ([libc6](https://packages.debian.org/bullseye/libc6)).
  - Ubuntu 20.04, glibc 2.31 (the `ubuntu:20.04` image, measured).
  - RHEL 8, glibc 2.28 ([Rocky 8 BaseOS](https://dl.rockylinux.org/pub/rocky/8/BaseOS/x86_64/os/Packages/g/)).
  - RHEL 9, glibc 2.34 ([Rocky 9 BaseOS](https://dl.rockylinux.org/pub/rocky/9/BaseOS/x86_64/os/Packages/g/)).
  - Among the distros that can, the oldest git is Ubuntu 22.04's 2.34.1. The `.deb`/`.rpm`
    packages depend on `git` without a version.
  - So a Linux user with git older than 2.34 must have built parterre from source **(derived)**.
- **macOS.** Without Homebrew, git is Apple's (2.39.5 or 2.50.1).
- **Windows.** Git for Windows doesn't update itself unless asked, so any version may linger
  **(unverified how common)**. The Chocolatey package pulls in `git >= 2.31.0`
  ([parterre.nuspec](../../packaging/chocolatey/parterre.nuspec)). The MSI and zip don't check.

## 5. Recommended minimum (derived)

**Document git 2.31 as the minimum now, and test it in CI. Raise it to 2.39 in May 2027.**

- 2.31 is what the code needs today, and the Chocolatey package already says so. Supporting
  2.30 would add three fallbacks (§3.5) for users who can't run the release binary anyway
  (§4.2).
- Testing 2.31 keeps the existing fallbacks for 2.34 (Ubuntu 22.04) honest. It also catches new
  code that reaches for a newer option without a fallback.
- When Ubuntu 22.04's standard support ends in May 2027, `docs/distribution.md` already plans to
  raise the glibc baseline. Raising git to 2.39 at the same time (Debian 12, Apple Git-154, both
  still supported) lets the `worktree list` and `fmt-merge-msg --into-name` fallbacks go.
- Add the start-up version check (§3.4), so a git below the minimum gets a clear message rather
  than wrong answers.
- Say the minimum in the README's "parterre runs the `git` you already have" paragraph, and fix
  the `merge.rs:366` comment (2.35, not 2.38).
- Before Git 3.0 ships, open bare repositories with `--git-dir`. Under Git 3.0's defaults,
  `-C <bare repo>` is refused (§6.6).

## 6. Testing against several git versions in CI

### 6.1 What CI tests today

[ci.yml](../../.github/workflows/ci.yml) runs `cargo test --workspace` on three legs:

| Leg | git it tests | Source |
|---|---|---|
| `ubuntu-latest` in an `ubuntu:22.04` container | 2.34.1, installed by apt in "Prepare container" | [UBU](https://packages.ubuntu.com/search?keywords=git&searchon=names&exact=1&suite=all&section=all) |
| `windows-latest` | 2.55.0.windows.5 | [Windows2025-Readme.md](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md) |
| `macos-latest` | 2.55.0 (Homebrew's, probably **(unverified)**) | [macos-15-Readme.md](https://github.com/actions/runner-images/blob/main/images/macos/macos-15-Readme.md) |

- The Linux leg also runs the tests with `GIT_CONFIG_SYSTEM` pointing at an autocrlf config.
- [linux-packages.yml](../../.github/workflows/linux-packages.yml) installs the `.deb`/`.rpm`
  in Debian 12, Ubuntu 22.04/24.04, Fedora and openSUSE Leap 15.6. It runs only
  `parterre --version`, so it tests no git command.
- So the floor (2.31) and the newest git are untested, and so is anything between 2.34 and the
  runners' 2.55 **(derived)**.

### 6.2 Ways to get a given git

1. **Build from the release tarball, cached.** Tarballs and `sha256sums.asc` for every release
   are on [KORG](https://mirrors.edge.kernel.org/pub/software/scm/git/).
   - **Dependencies.** zlib is required. curl, expat, Perl, Tcl/Tk, gettext and Python are
     optional ([INSTALL v2.51.0, lines 110–162](https://github.com/git/git/blob/v2.51.0/INSTALL#L110-L162)).
   - **What `NO_CURL` drops.** Only the http(s) transports
     ([Makefile v2.51.0, lines 456–458](https://github.com/git/git/blob/v2.51.0/Makefile#L456-L458)).
     parterre's tests use none.
   - **Rust.** Since 2.55, git builds its Rust parts by default; `NO_RUST` opts out until Git
     3.0 ([RN 2.55.0:88](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.55.0.adoc#L88),
     [Makefile v2.56.0, lines 495–501](https://github.com/git/git/blob/v2.56.0/Makefile#L495-L501)).
     CI has `cargo` anyway.
   - **Build time.** About 16–30 s with `make -j4` on 4 CPUs
     (**tested 2026-10-04**: 2.39.5, `docker run --cpus=4`; a second run measured 17–29 s for
     2.25 through 2.56). Hosted Linux runners for public repositories have 4 CPUs
     ([GHDOCS runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)).
   - **Toolchain.** Old tags build on Ubuntu 22.04 (gcc 11) and 24.04 (gcc 13), but not on
     26.04 (gcc 15, C23 by default) without `CFLAGS=-std=gnu17` (measured, see note). So pin
     the image; `ubuntu-latest` is moving to 26.04 ([docs/distribution.md](../distribution.md#linux)).
   - **Install path.** It is compiled in (`git --exec-path`), so restore the cache to the same
     path. `RUNTIME_PREFIX=YesPlease` makes the install relocatable
     ([Makefile v2.51.0, lines 365–370](https://github.com/git/git/blob/v2.51.0/Makefile#L365-L370)).
     A source-built git reads `<prefix>/etc/gitconfig` as its system config, not
     `/etc/gitconfig`.
   - **Caching.** `actions/cache` gives a repository 10 GB. Entries unused for 7 days are
     evicted, and a PR can restore caches saved on `main`
     ([GHDOCS caching](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)).
     One install is 56–89 MB unstripped (tested), about 26 MB with `INSTALL_STRIP=-s`.
2. **Distro containers.** Each image's own git is exactly one of the versions in §4
   (`ubuntu:20.04` 2.25.1, `debian:11` 2.30.2, `ubuntu:22.04` 2.34.1, `debian:12` 2.39.5,
   `ubuntu:24.04` 2.43.0; measured).
   - **Pro:** this tests the distro's patched build.
   - **Con: the Rust test binaries.** Each container would need its own build of them, or one
     build in the oldest glibc reused. That is minutes per leg instead of seconds.
   - **Con: old images.** `ubuntu:20.04` and `debian:11` are no longer listed as supported
     tags on Docker Hub ([ubuntu](https://hub.docker.com/_/ubuntu),
     [debian](https://hub.docker.com/_/debian)). Node 24 actions need glibc ≥ 2.28
     ([Node 24 BUILDING.md](https://github.com/nodejs/node/blob/v24.0.0/BUILDING.md#platform-list)),
     so `amazonlinux:2` (glibc 2.26) can't run `actions/checkout` at all
     ([actions/runner#2906](https://github.com/actions/runner/issues/2906)).
   - **Limits:** container jobs need a Linux runner
     ([GHDOCS container](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idcontainer)).
   - **Precedent:** git's own CI does run `actions/checkout@v6` in `ubuntu:20.04`,
     `almalinux:8`, `debian:12` and `alpine` ([main.yml v2.56.0](https://github.com/git/git/blob/v2.56.0/.github/workflows/main.yml)).
3. **Setup actions.** The only Marketplace action that installs a chosen git is
   [isikerhan/setup-git](https://github.com/isikerhan/setup-git): one release (v1.0.0,
   2025-03-31) and no stars.
   - On Linux it runs `sudo apt-get` and a serial `make` without flags
     ([install-linux.sh](https://github.com/isikerhan/setup-git/blob/v1.0.0/install-linux.sh)).
     So it fails from 2.55 on (Rust) and in containers without `sudo`.
   - Not recommended.
4. **The git-core PPA** publishes only the newest git per Ubuntu series, so it can't pin
   ([PPA](https://launchpad.net/~git-core/+archive/ubuntu/ppa)). The Ubuntu runners get their
   git from it ([install-git.sh](https://github.com/actions/runner-images/blob/main/images/ubuntu/scripts/build/install-git.sh)).
5. **Git for Windows.** Every release keeps `PortableGit-<v>-64-bit.7z.exe` and
   `MinGit-<v>-64-bit.zip` ([GFW](https://github.com/git-for-windows/git/releases/tag/v2.56.0.windows.1)).
   - MinGit is "intentionally minimal, non-interactive" and meant for apps that bundle git
     ([gitforwindows.org/mingit](https://gitforwindows.org/mingit)), so use PortableGit for
     tests.
   - Snapshots of the next version are at [git-snapshots](https://gitforwindows.org/git-snapshots/).
   - Upstream's `next`/`master` have no Windows builds.
6. **macOS.** Homebrew has only the latest git and no versioned formulae
   ([formula JSON](https://formulae.brew.sh/api/formula/git.json),
   [Homebrew versions policy](https://docs.brew.sh/Versions)). An old git on macOS means
   building it.

Note: some results in (1) and (2) are marked "measured". They come from a second set of
Docker runs on 2026-10-04: `ubuntu:22.04` to `ubuntu:26.04`, `make -j4` on 4 CPUs, and each
image's packaged git and glibc. I did not repeat the gcc 15 failure or the image survey myself,
so treat them as **(unverified)** until CI shows them.

### 6.3 Same git for the helpers and for parterre (derived)

Both spawn `git` from `PATH` (§1), so putting one git first on `PATH` tests parterre with it
and also builds the test repositories with it.

- **Down to 2.28 that is fine.** The helpers need `init -b` (2.28).
- **Below 2.32 there is a catch.** The helpers' `GIT_CONFIG_GLOBAL=/dev/null` and CI's
  `GIT_CONFIG_SYSTEM` are ignored
  ([RN 2.32.0:88](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.32.0.adoc#L88)).
  - The helpers then read `~/.gitconfig`. Run those tests with `HOME` at an empty folder, as §7
    did. Set `CARGO_HOME`/`RUSTUP_HOME` explicitly first, since rustup lives under `HOME`.
  - In the container, root's global config only holds the `safe.directory` line from "Trust the
    checkout", which is harmless, so this is belt and braces.
  - Skip the autocrlf run for gits below 2.32, because `GIT_CONFIG_SYSTEM` does nothing there.
- **When this would stop being enough.** Testing parterre against a git older than the helpers
  can use would need a separate override for parterre's git, e.g. an environment variable read
  in `program.rs`. With the floor at 2.31 that isn't needed.

### 6.4 Recommended setup (derived)

**On every PR and push to `main`: new steps in the Linux leg of `ci.yml`.**

The Linux leg already runs in `ubuntu:22.04` with the Rust build cached, so the extra gits cost
no second Rust build. A sketch (not run):

```yaml
      # Old and new gits from source, by version (docs/research/git-version-support.md §6).
      - uses: actions/cache@v5
        if: env.BUILD == 'true' && matrix.container
        id: gits
        with:
          path: /opt/git
          key: gits-ubuntu22.04-2.31.8-2.39.5-2.56.0-v1
      - name: Build gits
        if: env.BUILD == 'true' && matrix.container && steps.gits.outputs.cache-hit != 'true'
        run: |
          apt-get install -y --no-install-recommends zlib1g-dev xz-utils
          for v in 2.31.8 2.39.5 2.56.0; do
            curl -fsSL "https://mirrors.edge.kernel.org/pub/software/scm/git/git-$v.tar.xz" | tar xJ
            make -C "git-$v" -j"$(nproc)" prefix="/opt/git/$v" NO_CURL=1 NO_EXPAT=1 \
              NO_GETTEXT=1 NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 NO_RUST=1 INSTALL_STRIP=-s install
          done
      - name: Test with other gits
        if: env.BUILD == 'true' && matrix.container
        run: |
          export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
          for v in 2.31.8 2.39.5 2.56.0; do
            echo "::group::git $v"
            HOME=$(mktemp -d) PATH="/opt/git/$v/bin:$PATH" cargo test --workspace --no-fail-fast
            echo "::endgroup::"
          done
```

- **Versions:**
  - 2.31.8 is the floor; move it with the documented minimum.
  - 2.39.5 is Debian 12 and Apple Git-154, and the planned floor from May 2027.
  - 2.56.0 is the latest release; bump it by hand, or let the scheduled job (below) watch for
    newer ones.
  - 2.34.1 is covered by the leg's own git. Windows and macOS keep the runners' git.
- **Cost:**
  - The first run builds three gits (§6.2: about 20–30 s each).
  - Later runs restore about 80 MB from the cache, then pay three more test runs: about 15 s
    each here on 32 cores (§7), more on a 4-CPU runner **(unverified)**.
  - Standard runners are free for public repositories
    ([GHDOCS billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).
- **Alternative: a separate job.** A separate `git-versions` job would keep the required "Test"
  checks as fast as now, but it rebuilds the Rust tests (minutes rather than seconds) unless it
  shares a cache with the Linux leg.

**Weekly: a new `git-next.yml` workflow on `schedule`.**

- **What it builds:** `master`, `next`, and `next` with `WITH_BREAKING_CHANGES=YesPlease`
  ([BREAK lines 71–76](https://github.com/git/git/blob/v2.56.0/Documentation/BreakingChanges.adoc#L71-L76)).
  It fetches them as `https://github.com/git/git/archive/refs/heads/<branch>.tar.gz`, builds
  them without `NO_RUST`, as distros will, and runs `cargo test --workspace` with each.
  - `master` "aims to be more stable than any released version". `next` holds topics for at
    least a week before `master`
    ([MaintNotes](https://github.com/git/git/blob/todo/MaintNotes)).
  - A feature release comes every "eight to ten weeks", with release candidates about a week
    apart ([maintain-git.adoc lines 111–115](https://github.com/git/git/blob/v2.56.0/Documentation/howto/maintain-git.adoc#L111-L115)).
    In practice the last six releases were 10–13 weeks apart, with rc0 about 2.5 weeks before
    the final ([tags](https://github.com/git/git/tags)).
  - So a weekly run sees every release candidate before users do **(derived)**.
- **Schedule details:**
  - Use an odd minute, e.g. `cron: '17 4 * * 1'`. Scheduled runs can be delayed or dropped
    under load, especially at the top of the hour.
  - They run on the default branch.
  - In a public repository they are disabled after 60 days without activity
    ([GHDOCS schedule](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule)).
  - Add `workflow_dispatch` to run it by hand.
- **What a failure means:** a heads-up, not a blocker. Turn it into a `bug` issue by hand.
- **Optional, Windows:** a weekly Windows leg with the newest
  [Git for Windows snapshot](https://gitforwindows.org/git-snapshots/) (PortableGit). Not
  needed now: git's options don't differ by platform, and the Windows-only differences
  (autocrlf, paths) are already tested with the runner's git **(derived)**.

### 6.5 Version check in the app (derived)

If parterre gets a start-up check (§3.4), its parser must accept every form `git version`
prints:

- `2.55.0.windows.5`
- `2.39.5 (Apple Git-154)`
- `2.56.GIT`, from a source archive without `.git` (**tested 2026-10-04**:
  `git-next --version`)
- release candidates such as `2.56.0-rc1`

The CI legs above exercise the first and third forms.

### 6.6 Git 3.0 preview (tested 2026-10-04)

**What I ran.** 2.56.0 built with `WITH_BREAKING_CHANGES=YesPlease`, running
`cargo test --workspace`: 745 passed and 2 failed. That build makes new repositories reftable
(`rev-parse --show-ref-format` printed `reftable`) and still SHA-1 (`--show-object-format`
printed `sha1`). [BREAK](https://github.com/git/git/blob/v2.56.0/Documentation/BreakingChanges.adoc#L90)
plans SHA-256 too, which this build doesn't yet do.

**Failure 1: bare repositories.** `bare_current_branch_cannot_be_deleted_even_when_git_would_allow_it`
fails with `NotARepository`.
- Cause: Git 3.0 changes `safe.bareRepository` from `all` to `explicit`, and "Git will refuse to
  work with bare repositories that are discovered implicitly"
  ([BREAK line 219](https://github.com/git/git/blob/v2.56.0/Documentation/BreakingChanges.adoc#L219)).
- `git -C <bare> rev-parse` printed `fatal: cannot use bare repository '…'
  (safe.bareRepository is 'explicit')`. `git --git-dir=<bare> rev-parse` worked.
- So parterre can't open a bare repository under Git 3.0 until `Git::command` passes
  `--git-dir` for one **(derived)**.

**Failure 2: a test assumes the files backend.**
`a_missing_upstream_uses_git_head_fallback_and_a_ref_lock_never_forces_deletion` writes
`.git/refs/heads/topic.lock` by hand. In a reftable repository `.git/refs/heads` is a file, so
the write fails with `NotADirectory`. This is a test assumption, not a parterre bug **(derived)**.

`next` (2.56.GIT) built normally passed all 747 tests.

## 7. Experiments (2026-10-04, Ubuntu 24.04 host, local git 2.43.0)

- **Building git from release tarballs**, inside `ubuntu:24.04` with only `build-essential`
  and `zlib1g-dev`:
  `make -j32 prefix=/tmp/gitver/git-$v NO_CURL=1 NO_OPENSSL=1 NO_EXPAT=1 NO_GETTEXT=1
  NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 install`.
  - Each build took 4–6 s on 32 cores, and each install is 56–89 MB. 2.39.5 took 16 s with
    `docker run --cpus=4` and `make -j4`, about what a hosted runner has.
  - `next` came from `https://github.com/git/git/archive/refs/heads/next.tar.gz`. Without
    `.git`, it calls itself `2.56.GIT`.
  - 2.25.5 through 2.47.3 built with that.
  - 2.56.0 stopped at `cargo: not found`: git builds its Rust parts by default since 2.55
    ([RN 2.55.0:88](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/RelNotes/2.55.0.adoc#L88),
    "in Git version 3.0, Rust will become mandatory"). It built with `NO_RUST=1` added.
- **Running the tests.** `cargo test` with `PATH=/tmp/gitver/git-$v/bin:$PATH`, and `HOME` and
  `XDG_CONFIG_HOME` pointing at an empty folder, because gits before 2.32 ignore
  `GIT_CONFIG_GLOBAL`. The test binaries were built once and reused for every version.
  - parterre-core and parterre-forge took about 11 s per version; the whole workspace about 15 s.

  | git | `-p parterre-core -p parterre-forge` | `--workspace` |
  |---|---|---|
  | 2.25.5 | 218 pass, 269 fail (`init -b`: `unknown switch 'b'`) | — |
  | 2.30.9 | 451 pass, 36 fail (§3.2) | — |
  | 2.31.8 | 487 pass | 747 pass |
  | 2.34.8 | 487 pass | 747 pass |
  | 2.36.6 | 487 pass | — |
  | 2.39.5 | 487 pass | — |
  | 2.43.0 (system) | 487 pass | 747 pass |
  | 2.47.3 | 487 pass | — |
  | 2.56.0 (`NO_RUST`) | 487 pass | 747 pass |
  | 2.56.0 `WITH_BREAKING_CHANGES=YesPlease` | — | 745 pass, 2 fail (§6.6) |
  | `next` @ 2026-10-04 (`2.56.GIT`) | — | 747 pass |
- **Single commands**, to explain the failures:
  - `git-2.30.9 rev-parse --path-format=absolute --git-common-dir` printed
    `--path-format=absolute` and the path, and exited 0.
  - `git-2.30.9 log --diff-merges=first-parent` gave
    `fatal: unknown value for --diff-merges: first-parent` (exit 128).
  - `git-2.30.9 worktree list --porcelain -z` gave `error: unknown switch 'z'`.
  - `git-2.34.8 fmt-merge-msg --into-name x` gave `error: unknown option 'into-name'`
    (exit 129).
  - `git-2.39.5 fmt-merge-msg --into-name x` exited 0.

## 8. Open questions for the human

1. **The minimum.** Should the documented minimum be 2.31, the real floor? Or 2.34, the oldest
   distro git that can run the release binary? §5 argues for 2.31.
2. **Where the PR-time tests go.** Extra steps in the Linux leg (cheapest, but the required check
   gets slower), or a separate job (§6.4)?
3. **A version check at start-up**, with the parser in §6.5. Yes or no?
4. **Git 3.0 and bare repositories** (§6.6). This deserves a `bug` issue of its own. So does
   the reftable assumption in the `branches.rs` test.
