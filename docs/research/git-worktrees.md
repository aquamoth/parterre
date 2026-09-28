# git's worktree facts

Research note for [#133](https://github.com/aquamoth/parterre/issues/133), part of the map
[Worktrees in the graph](https://github.com/aquamoth/parterre/issues/132). The question: *what
does git give us for worktrees, and what does each part of the map rest on?* It has four parts:
listing, loading, reloading, and opening a folder.

Sources are git's documentation and source at a pinned commit, platform docs, and experiments I
ran. A statement that is my own conclusion is marked **(derived)**. A statement I could not
check is marked **(unverified)**. A statement I checked by running a command is marked
**(tested)**. All tests ran on 2026-09-29 on Windows 11 with Git for Windows 2.53.0.windows.3,
in throwaway repositories under `%TEMP%`. macOS and Linux were not available, so everything
about them comes from documentation or source.

## TL;DR

1. **Listing.** `git worktree list --porcelain` prints one record per worktree: `worktree
   <path>`, then `bare`, or `HEAD <oid>` followed by `branch <ref>` or `detached`, then
   optionally `locked [reason]` and `prunable <reason>`, then an empty line. The main worktree
   comes first and the rest are sorted by path. Versions: `list --porcelain` with
   `worktree`/`HEAD`/`branch`/`detached`/`bare` since **2.7.0**. `locked` and `prunable` since
   **2.31.0**. `-z` since **2.36.0**. Ubuntu 22.04 ships git **2.34.1**, so it has
   `prunable` but not `-z`. Paths are never quoted, even without `-z`. On Windows they have
   forward slashes, an upper-case drive letter, and raw UTF-8. Surprises:
   - A **bare** main repository is listed, with `bare` and no `HEAD`.
   - An **unborn** branch shows `HEAD 0000…0000`.
   - A branch checked out twice shows in both records.
   - A worktree that is **locked and gone** is *not* marked `prunable`.
   - With a separate git dir (`--separate-git-dir`, submodules), the main worktree's path is
     **the git dir**, not the folder you work in.
2. **Loading.** `git log --all` already starts from other worktrees' HEADs (since 2.15.0), but
   `load` doesn't use `--all`. The cheapest way to add them is to take the `HEAD` of each
   `detached` record from `worktree list`, skip the null id, and add it to the `log --stdin`
   input. That costs one git process, and the listing is needed for the labels anyway.
   `%(worktreepath)` (since 2.23.0) is **not enough** for branches:
   - It gives only one path when a branch is checked out twice.
   - An unborn branch has no ref, so it gets no row.
   - It gives the **bare repository's path** to the branch the bare HEAD names.
   - It says nothing about locked or gone.

   Where it gives a path, it is the same string `worktree list` prints: both come from the
   same C struct.
3. **Reload.** Another worktree's checkout, reset or detached commit rewrites
   `<common>/worktrees/<id>/HEAD`. A commit on a branch changes only `refs/heads/…`, which is
   already watched. `add`, `remove` and `prune` add or remove `worktrees/<id>/`, which changes
   the mtime of `worktrees/`. `lock` and `unlock` add or remove `worktrees/<id>/locked`. `move`
   rewrites only `worktrees/<id>/gitdir`, in place. Deleting a worktree's folder by hand
   changes **nothing** in the repository. `watch.rs` would have to add:
   - `<common>/worktrees` itself.
   - For each entry, `HEAD`, `gitdir` and `locked`, plus `reftable/` in reftable repositories.
   - `<common>/HEAD`, which is missing today when parterre runs in a linked worktree.
   - Optionally one `stat` of each worktree folder, to see it go missing.

   It must **not** walk `worktrees/` recursively, nor hash the `worktrees/<id>` directories'
   mtimes: every `git status` or commit in another worktree touches those.
4. **Opening a folder.**
   - **Windows:** `explorer.exe` exits **1 whether it worked or not**, and for a missing
     folder it silently opens *Documents*. It also opens Documents for a path with forward
     slashes (git's form) and for a path containing a comma, because Rust's `Command::arg`
     quotes only arguments with spaces. Converting to backslashes and passing
     `"<path>"` through `raw_arg` opened every folder I tried. `ShellExecuteExW` handles all
     of these and returns `ERROR_FILE_NOT_FOUND` for a missing folder.
   - **macOS:** `open <path>`.
   - **Linux:** `xdg-open <path>`. It rejects any argument that starts with `-`, which an
     absolute path never does. Its exit code means something only in the generic mode (2 =
     missing file), and it may block until the app exits.
   - **Everywhere:** check `is_dir()` first and treat the opener's exit code as advisory.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT | git `v2.56.0` @ `a0189536` (2026-09-28) | https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/ |
| GITTAG | git source at older release tags, for "since which version" (fetched raw per tag) | `https://github.com/git/git/blob/<tag>/<path>` |
| RELNOTES | git's release notes, `Documentation/RelNotes/<version>.adoc` at GIT | GIT + `Documentation/RelNotes/` |
| RUST | Rust std docs and source for 1.98.1 | https://doc.rust-lang.org/std/ , https://github.com/rust-lang/rust/blob/1.98.1/ |
| MSDOCS | Microsoft Learn, ShellExecuteW (read 2026-09-29) | https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew |
| XDG | xdg-utils `v1.2.1`: `scripts/xdg-open.in` and `scripts/desc/xdg-open.xml` | https://gitlab.freedesktop.org/xdg/xdg-utils/-/blob/v1.2.1/ |
| OPEN1 | macOS `open(1)` man page (Apple's text, as mirrored at unix.com; Apple's own archive URL is gone) | https://www.unix.com/man_page/osx/1/open/ |
| DISTRO | Debian and Ubuntu package pages (read 2026-09-29) | https://packages.debian.org/bookworm/git , https://packages.ubuntu.com/jammy/git , https://packages.ubuntu.com/noble/git |

To find the oldest version of a feature, I looked for the code or doc line that implements it
in the source at consecutive release tags (GITTAG), and checked the result against RELNOTES.

---

## 0. Where parterre stands (context)

- `Git::load` ([crates/parterre-core/src/git.rs](../../crates/parterre-core/src/git.rs))
  reads refs with `for-each-ref` and this worktree's HEAD with `symbolic-ref`/`rev-parse`, then
  pipes exactly those commit ids into `git log --stdin`. That way refs are read before the
  walk, and a concurrent fetch can't leave a ref at a commit that wasn't loaded. Another
  worktree's detached HEAD is in neither list, so its commits (and anything reachable only
  from them) are missing.
- `RefStorage::locate` ([crates/parterre-core/src/watch.rs](../../crates/parterre-core/src/watch.rs))
  fingerprints `<git_dir>/HEAD`, `<common>/packed-refs`, `<common>/refs`, `<common>/reftable`,
  and in a linked worktree also `<git_dir>/refs` and `<git_dir>/reftable`. The path list is
  fixed when `locate` runs.
- `browser::open` ([crates/parterre/src/browser.rs](../../crates/parterre/src/browser.rs))
  runs `explorer.exe` / `open` / `xdg-open` with one argument through `std::process::Command`,
  with no shell and null stdio, and reaps the child on a thread. It accepts only github.com URLs.
- Every git call sets `LC_ALL=C` and `GIT_OPTIONAL_LOCKS=0`.

## 1. Listing: `git worktree list --porcelain [-z]`

### 1.1 The format

The docs promise the porcelain format "will remain stable across Git versions and regardless of
user configuration" ([GIT git-worktree.adoc#L259-L263](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L259-L263)).
It has "a line per attribute", "label and value separated by a single space". Boolean
attributes are "listed as a label only, and are present only if the value is true". "The first
attribute of a worktree is always `worktree`, an empty line indicates the end of the record"
([#L460-L470](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L460-L470)).
The printer is `show_worktree_porcelain`
([GIT builtin/worktree.c#L1010-L1041](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1010-L1041)):

```c
printf("worktree %s%c", wt->path, line_terminator);
if (wt->is_bare)
        printf("bare%c", line_terminator);
else {
        printf("HEAD %s%c", oid_to_hex(&wt->head_oid), line_terminator);
        if (wt->is_detached)
                printf("detached%c", line_terminator);
        else if (wt->head_ref)
                printf("branch %s%c", wt->head_ref, line_terminator);
}
/* then "locked" [ quoted reason ], then "prunable <reason>", then the empty line */
```

| Field | Meaning | Since |
|---|---|---|
| `worktree <path>` | Absolute path; never quoted (`%s`) | 2.7.0 |
| `bare` | The main repository is bare; no `HEAD` line follows | 2.7.0 |
| `HEAD <oid>` | Commit checked out; all zeros when unborn | 2.7.0 |
| `branch <refname>` | Full ref name (`refs/heads/x`), when HEAD is a symref | 2.7.0 |
| `detached` | HEAD holds an id | 2.7.0 |
| `locked` / `locked <reason>` | `worktrees/<id>/locked` exists; reason is its trimmed content | 2.31.0 |
| `prunable <reason>` | `git worktree prune` would remove the entry | 2.31.0 |
| `-z` (NUL instead of LF) | Unquoted lock reason; lines end in NUL, records in an extra NUL | 2.36.0 |

Where the versions come from:

- `list` and `--porcelain`: "git worktree learned a list subcommand" (RELNOTES 2.7.0). The
  porcelain printer with `bare`, `HEAD`, `detached` and `branch` is in `builtin/worktree.c` at
  v2.7.0 and absent at v2.6.0 (GITTAG).
- `locked` and `prunable`: "`git worktree list` now annotates worktrees as prunable, shows
  locked and prunable attributes in --porcelain mode, and gained a --verbose option" (RELNOTES
  2.31.0). 2.30.0 showed `locked` only in the human format (RELNOTES 2.30.0), and its
  porcelain printer has neither field (GITTAG v2.30.0).
- `-z`: "did not c-quote pathnames and lock reasons with unsafe bytes correctly, which is worked
  around by introducing NUL terminated output format with `-z`" (RELNOTES 2.36.0).
  `line_terminator` first appears in v2.36.0 (GITTAG). Since 2.36, without `-z` a lock reason
  with "unusual" characters is C-quoted
  ([git-worktree.adoc#L501-L510](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L501-L510)).
  2.31 to 2.35 quote it the same way (`quote_c_style`, which adds quotes only when a byte
  needs escaping; GITTAG v2.31.0). The **path is never quoted**, with or without `-z`, in any
  version, so only `-z` can carry a path containing a newline.
- Order: the main worktree comes first, and the rest are sorted by path
  (`pathsort(worktrees + 1)`,
  [builtin/worktree.c#L1157-L1158](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1157-L1158)).
  Before 2.11.1/2.12.0 they came in `readdir()` order (RELNOTES 2.12.0).
- Related commands: `worktree add` (and `$GIT_DIR/worktrees`, `rev-parse --git-common-dir`)
  since 2.5.0. `lock`/`unlock` since 2.10.0. `move`/`remove` since 2.17.0. `repair` since
  2.29.0. `add --orphan` since 2.42.0. `--relative-paths` since 2.48.0 (all GITTAG, confirmed
  against RELNOTES).

The prunable reason is a translated message
([worktree.c#L937-L1030](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L937-L1030)).
parterre runs git with `LC_ALL=C`, but **(derived)** the program should still use only the
presence of the field, never its text.

**Which git the platforms ship** (DISTRO, read 2026-09-29): Debian 12: 2.39.5. Ubuntu 22.04:
**2.34.1**. Ubuntu 24.04: 2.43.0. All three have `prunable` and `%(worktreepath)`. Only Debian 12
and Ubuntu 24.04 have `-z`. `docs/building.md` lists Ubuntu 22.04 among the Linux CI targets.

### 1.2 The cases the map cares about

All **(tested)** with `git worktree list --porcelain` in the repositories described.

| Case | What is printed |
|---|---|
| Main worktree on `main` | `worktree C:/…/e1/main`, `HEAD <oid>`, `branch refs/heads/main` |
| Linked, detached (`add --detach`) | `HEAD <oid>`, `detached` |
| **Unborn** branch (`add --orphan -b orph`) | `HEAD 0000000000000000000000000000000000000000`, `branch refs/heads/orph` |
| Same branch in two worktrees (`add -f ../wt-dup feat`) | Both records say `branch refs/heads/feat` |
| Branch being rebased (`rebase` stopped midway) | `HEAD <commit being rebuilt>`, `detached`. The branch name is not shown |
| Locked, with a two-line reason | `locked "on usb\nsecond line"` (with `-z`: raw, with the newline) |
| `add --lock` without `--reason` | `locked added with --lock`: git writes that text as the reason ([builtin/worktree.c#L884](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L884)) |
| Folder deleted by hand | `prunable gitdir file points to non-existent location`; its `HEAD` is still printed |
| **Folder deleted by hand, but locked** | Only `locked …`. **No `prunable`** |
| **Bare** main repository (`clone --bare`, then `worktree add`) | `worktree C:/…/e2.git`, `bare`. No `HEAD` line |
| **Separate git dir** (`init --separate-git-dir=e3-gitdir e3`) | `worktree C:/…/e3-gitdir`: the git dir, not `e3` |

- **Locked and gone.** A locked entry is never prunable. `should_prune_worktree` returns
  "don't prune" as soon as `locked` exists, before it looks for the folder
  ([worktree.c#L966-L969](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L966-L969)).
  So `prunable` means "gone and not locked", not just "gone". **(derived)** To mark every
  missing folder, stat the path as well.
- **Prunable means `<path>/.git` is gone.** `list` sets `expire = TIME_MAX`
  ([builtin/worktree.c#L1144](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1144)).
  So a worktree is prunable as soon as the `.git` file its `gitdir` names doesn't exist, however
  new the entry is ([worktree.c#L1013-L1019](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L1013-L1019)).
- **Entries that aren't listed at all.** A `worktrees/<id>/` with a missing or empty `gitdir`
  file is skipped silently (`get_linked_worktree` returns NULL,
  [worktree.c#L152-L155](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L152-L155)),
  even though `prune` would remove it with the reason "gitdir file does not exist".
- **Main worktree path.** It is `realpath(common dir)` with a trailing `/.git` stripped
  ([worktree.c#L113-L139](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L113-L139)).
  So a bare repository lists its own directory. A repository whose git dir isn't called `.git`
  lists the git dir (tested above). **(derived)** The same holds for a submodule, whose git dir
  is `<super>/.git/modules/<name>`. `git rev-parse --show-toplevel`, which `Git::locate`
  already runs, gives the real folder of the worktree parterre is in.
- **HEAD that can't be read.** If HEAD can't be resolved at all, `add_head_info` leaves both
  fields unset ([worktree.c#L40-L56](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L40-L56)).
  **(derived)** The record then has `HEAD 000…` and neither `branch` nor `detached`.
- **A checkout can't take a branch already used elsewhere.** `git checkout feat` in a third
  worktree failed with "fatal: 'feat' is already used by worktree at '…/wt-dup'" (tested). Only
  `-f`/`--ignore-other-worktrees` gets two worktrees onto one branch.
- **Folder name vs id.** `move` keeps the id: after `worktree move ../wt-new ../wt-moved` the
  entry was still `worktrees/wt-new` (tested). The id is also sanitised to a valid ref
  component (RELNOTES 2.23.0), and gets a number suffix when taken
  ([git-worktree.adoc#L363-L372](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L363-L372)).
  **(derived)** A folder label should come from the last component of `worktree <path>`, not
  from the id.

### 1.3 Paths

- **Windows** (tested):
  - Paths come out with forward slashes and an upper-case drive letter:
    `C:/Users/matti/AppData/Local/Temp/wtr/e1/wt space/ö-dir`.
  - A worktree added with a backslash path (`C:\…\wt-backslash`), or with a lower-case path
    (`c:\users\matti\…\wt-lower`), was stored in `gitdir` and listed as
    `C:/Users/matti/AppData/…`. Git for Windows canonicalised the case at `add` time.
  - **(derived)** For a linked worktree, `list` prints the absolute path in the `gitdir` file
    as written ([worktree.c#L152-L163](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L152-L163)).
    An entry written by another tool, or by git under WSL (`/mnt/c/…`), comes through
    unchanged **(unverified)**.
- **Spaces and non-ASCII** are printed raw, with or without `-z`: `wt space/ö-dir` came out
  as the bytes `77 74 20 73 70 61 63 65 2f c3 b6 2d 64 69 72`, i.e. UTF-8, no quoting
  (tested). `core.quotePath` doesn't apply to this field (it is a plain `%s`).
- **Leading `-`.** A worktree at `…/main/-dash` was added and listed normally (tested). Listed
  paths are always absolute, so they start with a drive letter or `/`, never with `-`.
- **Relative links** (`worktree add --relative-paths`, or `worktree.useRelativePaths`, since
  2.48.0). `gitdir` then held `../../../../wt-rel/.git` and the `.git` file held
  `gitdir: ../main/.git/worktrees/wt-rel`, but `list` still printed the absolute path
  (tested). git resolves it with `strbuf_realpath_forgiving`
  ([worktree.c#L159-L163](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L159-L163)).
  The same `add` also set `extensions.relativeWorktrees = true` and
  `repositoryformatversion = 1` in the repository's config (tested). **(derived)** A git older
  than 2.48 then refuses to open the repository at all, whether or not worktrees are shown.

## 2. Loading: adding other worktrees' detached HEADs

### 2.1 What git itself does

`--all` means "Pretend as if all the refs in `refs/`, along with `HEAD`, are listed on the
command line"
([GIT rev-list-options.adoc#L167-L169](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/rev-list-options.adoc#L167-L169)).
`--single-worktree` explains that "by default, all working trees will be examined by the
following options when there are more than one", `--all` among them
([#L230-L236](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/rev-list-options.adoc#L230-L236)).
In the source, `--all` calls `other_head_refs`
([revision.c#L2846-L2854](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/revision.c#L2846-L2854)).
That walks `get_worktrees()`, skips the current worktree, and resolves each one's
`worktrees/<id>/HEAD` with `RESOLVE_REF_READING`, so unborn HEADs are skipped silently
([worktree.c#L602-L637](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L602-L637)).
`other_head_refs` and `--single-worktree` first appear in v2.15.0 (GITTAG). RELNOTES 2.15.0
describes the same change for gc: "did not consider the index and per-worktree refs of other
worktrees as the root for reachability traversal".

Tested:

- `git log --all --format=%s` in the main worktree listed `detached-only`, a commit made only
  on a linked worktree's detached HEAD.
- `git rev-parse "worktrees/ö-dir/HEAD" worktrees/wt-gone/HEAD main-worktree/HEAD` resolved
  all three, including the HEAD of a worktree whose folder had been deleted.
  `worktrees/wt-orphan/HEAD` (unborn) failed with "ignoring dangling symref … ambiguous
  argument", exit 128. The `main-worktree/` and `worktrees/<id>/` names are documented in
  [git-worktree.adoc#L299-L326](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L299-L326).
  `main-worktree/` first appears in `refs.c` at v2.20.0 (GITTAG).
- `for-each-ref` lists none of them: `git for-each-ref worktrees/ main-worktree/` printed
  nothing, and `--include-root-refs` added only this worktree's `HEAD`.

### 2.2 The ways to feed them to `load`

| Way | Cost | Catches |
|---|---|---|
| **A.** Run `git worktree list --porcelain`; take `HEAD <oid>` from each `detached` record, skip the all-zero id, add the rest to the `log --stdin` input | One extra git process, which can run beside `for-each-ref` before the walk. It stats each worktree's `.git` for `prunable` | Every detached HEAD, including those of gone worktrees (their `HEAD` file remains until pruned) |
| **B.** `git log --all …` instead of the explicit list | No extra process | Also walks `refs/notes/*`, which `load` leaves out (`parse_refs`), and the refs are no longer read before the walk |
| **C.** Add `worktrees/<id>/HEAD` *names* to the `--stdin` input | Needs the ids (a `read_dir` of `<common>/worktrees`) | One unborn HEAD fails the whole `log` (tested with `rev-parse`, above) |
| **D.** Read `<common>/worktrees/<id>/HEAD` files directly | No process | Wrong with reftable: the file is only a stub, `ref: refs/heads/.invalid` (tested in a `git init --ref-format=reftable` repository, reftable since 2.45.0). Also has to parse symrefs |

**(derived)** A is the cheapest correct way, because the map needs the `worktree list` records
for the labels anyway. An id read before `git log` starts stays loadable. The walk reads
objects, and objects don't go away between two git calls (gc keeps loose objects for a grace
period, and since 2.15.0 it also counts other worktrees' HEADs as reachable, RELNOTES
2.15.0). The only extra rule is to drop `0000…` (unborn) before writing to `--stdin`.

Cost **(tested)**: `git worktree list --porcelain` took 0.19 to 0.32 s wall time on this
machine with three worktrees, and `git for-each-ref` took 0.14 s. Most of that is starting a
process on Windows. The numbers were noisy and are only a rough guide.

### 2.3 Is `%(worktreepath)` enough for branches?

`%(worktreepath)` is "the absolute path to the worktree in which the ref is checked out, if it
is checked out in any linked worktree. Empty string otherwise"
([GIT git-for-each-ref.adoc#L182-L185](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-for-each-ref.adoc#L182-L185)).
It first appears in the docs at v2.23.0 (GITTAG). The implementation builds a hash map from
`get_worktrees()`, keyed by each worktree's `head_ref`, and returns `wt->path` for the first
entry it finds
([ref-filter.c#L2383-L2425](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/ref-filter.c#L2383-L2425)).
Despite "linked", the main worktree is included: `refs/heads/main` got the main folder's path
(tested). It is **not enough** for the map:

| Situation | `worktree list` | `%(worktreepath)` (tested) |
|---|---|---|
| `feat` checked out in `wt-feat` and `wt-dup` | Two records | One path only. git's `hashmap_add` puts new entries at the head of the bucket ([hashmap.c#L232-L242](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/hashmap.c#L232-L242)), so it is the later worktree in `readdir()` order |
| Unborn `orph` in `wt-orphan` | Record with `branch refs/heads/orph` | No row: the ref doesn't exist |
| Bare main repository whose `HEAD` names `main` | `bare`, no branch | `refs/heads/main [C:/…/e2.git]`: **the bare repository counts as having `main` checked out** |
| `feat` mid-rebase in `wt-feat` | `detached` | Empty for that worktree (it then showed `wt-dup`) |
| Locked / gone | `locked` / `prunable` | Nothing |

- **Do they agree?** Where `%(worktreepath)` gives a path, it is byte-for-byte the string in
  `worktree <path>`, because both print `wt->path` from `get_worktrees()` (tested:
  `C:/Users/matti/AppData/Local/Temp/wtr/e1/wt-feat` in both).
- **(derived)** So `worktree list` alone gives everything `%(worktreepath)` does, and adding
  the atom to `load`'s `for-each-ref` would only duplicate it.

## 3. Reload: what changes on disk

### 3.1 Experiments

In each experiment I recorded the size and mtime (ns) of every file and directory under the
main repository's `.git` except `objects/`, did one operation in another worktree (or from
the main one for the `worktree` subcommands), and diffed. **(tested)**, files backend unless
noted. "dir" means only the directory's own mtime changed.

| Operation | Changed under `<common>` |
|---|---|
| `commit` in a linked worktree with **detached** HEAD | `worktrees/<id>/HEAD` (mtime; size stays 41), `worktrees/<id>/logs/HEAD`, `…/index`, `…/COMMIT_EDITMSG`; dir `worktrees/<id>`; dir `<common>` (git created a `packed-refs.lock` there and removed it, seen with a FileSystemWatcher; `packed-refs` itself unchanged) |
| `commit` in a linked worktree **on a branch** | `refs/heads/<b>` and `logs/refs/heads/<b>` (already fingerprinted); `worktrees/<id>/logs/HEAD`, `…/index`, `…/COMMIT_EDITMSG`. **`worktrees/<id>/HEAD` unchanged** |
| `checkout --detach` (branch → detached) | `worktrees/<id>/HEAD` (size and mtime), `…/logs/HEAD`, `…/index` |
| `switch -c feat2` | `refs/heads/feat2` and its log added; `worktrees/<id>/HEAD`, `…/logs/HEAD` |
| `reset --hard HEAD~1` (detached) | `worktrees/<id>/HEAD`, `…/ORIG_HEAD`, `…/index`, `…/logs/HEAD` |
| `update-ref --no-deref HEAD HEAD~1` | `worktrees/<id>/HEAD`, `…/logs/HEAD` |
| `worktree add --detach ../wt-new` | dir `worktrees`; new `worktrees/wt-new/` with `HEAD`, `ORIG_HEAD`, `commondir`, `gitdir`, `index`, `logs/HEAD`, `refs/` |
| `worktree lock --reason usb` / `unlock` | `worktrees/<id>/locked` added / removed; dir `worktrees/<id>` |
| `worktree move ../wt-new ../wt-moved` | **Only** `worktrees/wt-new/gitdir` (size and mtime, rewritten in place; the directory's mtime did not change) |
| `worktree remove ../wt-moved` | `worktrees/wt-new/` gone; dir `worktrees` |
| `worktree prune` (one gone entry) | `worktrees/wt-gone/` gone; dir `worktrees` |
| Worktree folder deleted by hand (`rm -rf wt-orphan`) | **Nothing** |
| `git status` in another worktree, with an untracked file | dir `worktrees/<id>` only (an `index.lock` came and went) |
| `commit` in a linked worktree, **reftable** repository | `worktrees/<id>/reftable/tables.list` and its tables replaced; `…/index`, `…/COMMIT_EDITMSG`; `worktrees/<id>/HEAD` stays `ref: refs/heads/.invalid` |

From the source:

- When the last entry goes, `remove` and `prune` also `rmdir` the `worktrees` directory itself
  (`delete_worktrees_dir_if_empty`,
  [builtin/worktree.c#L164-L169](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L164-L169)).
  So it can appear and disappear.
- gitrepository-layout says the mtime of `worktrees/<id>/gitdir` "should be updated every time
  the linked repository is accessed"
  ([GIT gitrepository-layout.adoc#L285-L290](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/gitrepository-layout.adoc#L285-L290)).
  git 2.53 doesn't do this: `gitdir` never changed in the commits, resets and `status` runs
  above (tested). No code in `setup.c`, `worktree.c` or `builtin/worktree.c` touches it
  (GIT, searched for `utime`).
- The per-worktree reftable stack is `<common>/worktrees/<id>/reftable`
  ([refs/reftable-backend.c#L198](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/refs/reftable-backend.c#L198-L199)).

### 3.2 What `watch.rs` would have to add

**(derived)** from the table above and the current `RefStorage::locate`:

1. **`<common>/worktrees`** (the directory's own metadata). Its mtime changes on `add`,
   `remove` and `prune`, and it may be missing. `collect` already treats a missing path as
   adding nothing.
2. **For each entry `<common>/worktrees/<id>/`**, the files **`HEAD`**, **`gitdir`** (for
   `move`) and **`locked`** (for `lock`/`unlock`), plus **`reftable/`** walked, as today,
   for reftable repositories. The entries come and go, so the list must be re-read from the
   directory on every fingerprint, not fixed in `locate`. That costs one `read_dir` plus about
   four `stat` calls per worktree.
3. **`<common>/HEAD`.** Today only `<git_dir>/HEAD` is included. That is the same file in the
   main worktree, but in a linked worktree the main worktree's detached checkouts are missed.
   Item 2 covers every linked worktree's HEAD except the current one, which is
   `<git_dir>/HEAD` already.
4. **Not** the `worktrees/<id>` directories' own mtimes, and **not** a recursive walk of
   `worktrees/`. `index`, `logs/HEAD`, `COMMIT_EDITMSG`, `ORIG_HEAD` and the `index.lock` from
   any `git status` (editors run it constantly) change there without any ref moving.
   `<common>`'s own mtime moves on a detached commit (`packed-refs.lock`), so it must not be
   fingerprinted either (today it isn't).
5. **A folder deleted by hand** leaves no trace in the repository. To notice it without git,
   stat `<path>/.git` for each worktree, where `<path>` is from the last `worktree list` or
   from `gitdir` (which may be relative since 2.48, resolved against `worktrees/<id>/`). This
   is the same test git uses for `prunable`.
6. A commit on a branch in another worktree needs nothing new: it moves `refs/heads/<b>`,
   which `<common>/refs` covers. A detached `HEAD` keeps its 41-byte size, so only its mtime
   shows the change. The existing `(path, size, mtime)` entry covers that.

As today, a changed fingerprint means "maybe". **(derived)** With the toggle on, the reload's
`same_refs` comparison would also have to compare the worktree records (path, HEAD, branch,
locked, prunable/missing). Otherwise a `lock` or `move` would reload and then be thrown away
as "no change".

## 4. Opening a folder in the file manager

### 4.1 Windows

**`explorer.exe <path>`**, run through Rust's `Command` exactly as `browser.rs` runs it (a
throwaway binary, rustc 1.98.1). I found which folder opened by listing Explorer windows with
`Shell.Application.Windows()` before and after, then closed them. **(tested)**:

| Argument | Exit code | Window that opened |
|---|---|---|
| `C:\…\e1\wt space\ö-dir` (`arg`) | 1 | That folder |
| `C:/…/e1/wt space/ö-dir` (forward slashes, as git prints it) | 1 | **Documents** |
| `C:/…/e1/wt-feat` (forward slashes, no space) | 1 | **Documents** |
| `C:\…\e1\wt-gone` (folder does not exist) | 1 | **Documents** |
| `C:\…\wtr\a,b` (`arg`) | 1 | **Documents** |
| `C:\…\wtr\x&y ^%PATH%` | 1 | That folder |
| `C:\…\wtr\semi;colon` | 1 | That folder |
| `C:\…\e1\main\-dash` | 1 | That folder |
| `C:\…\e1\wt-feat\` (trailing backslash) | 1 | That folder |
| `"C:\…\wtr\a,b"` (`raw_arg`, quoted) | 1 | That folder |
| `"C:\…\e1\wt space\ö-dir"` (`raw_arg`) | 1 | That folder |
| `"C:\…\e1\wt-feat\"` (`raw_arg`, trailing backslash inside the quotes) | 1 | That folder |
| `"C:\…\e1\wt-gone"` (`raw_arg`, missing) | 1 | **Documents** |
| The same existing folder twice | 1 | A second window each time |

- **Exit code:** 1 in every case, success or failure. It tells nothing.
- **Missing folder:** Explorer opens the user's Documents folder and reports nothing.
- **Forward slashes:** git's form doesn't work. Convert `/` to `\` first.
- **Comma:** Explorer reads commas as separators between its switches (`/select,<path>`,
  `/root,<path>`). Rust quotes an argument only when it "contains space, tab, or is empty"
  ([RUST library/std/src/sys/args/windows.rs, `append_arg`](https://github.com/rust-lang/rust/blob/1.98.1/library/std/src/sys/args/windows.rs)),
  so `a,b` reached Explorer unquoted. Wrapping the path in double quotes via
  `CommandExt::raw_arg` (stable since 1.62,
  [RUST CommandExt](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html))
  fixed it. Windows file names can't contain `"`, so quoting can't be broken out of.
  Microsoft publishes no current reference for Explorer's command line. Everything in this
  table is observed behaviour, not documented.
- **Leading `-`:** not special to Explorer (its switches start with `/`), and git's paths
  start with a drive letter anyway.
- **`cmd /c start`:** would read `&`, `^` and `%` as shell syntax. `browser.rs` already avoids
  it for that reason, and Rust's docs warn about `cmd.exe` arguments
  ([RUST Command::arg](https://doc.rust-lang.org/std/process/struct.Command.html#method.arg)).

**`ShellExecuteExW`** is the documented way. "To open a folder, use … `ShellExecute(handle,
NULL, <fully_qualified_path_to_folder>, NULL, NULL, SW_SHOWNORMAL)`", or with the verb
`"open"`, or `"explore"` to explore it. It returns a value greater than 32 on success and an
error such as `ERROR_FILE_NOT_FOUND` / `ERROR_PATH_NOT_FOUND` otherwise. COM should be
initialised first (`CoInitializeEx(NULL, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)`).
By default it may reuse an open Explorer window rather than start a new one (MSDOCS).
**(tested)** through .NET's `Process.Start` with `UseShellExecute = true`, which calls
`ShellExecuteEx`:

- `C:\…\a,b` opened correctly.
- `C:/…/wt space/ö-dir`, with forward slashes, opened correctly.
- The missing `C:\…\wt-gone` failed with native error 2 ("The system cannot find the file
  specified") and opened no window.

From Rust this means the `windows-sys` crate (feature `Win32_UI_Shell`). It is already in
`Cargo.lock` as a transitive dependency, but not as a direct one.

### 4.2 macOS

"The open command opens a file (or a directory or URL), just as if you had double-clicked the
file's icon". `-R` "reveals file(s) in Finder instead of opening them" (OPEN1; synopsis
`open [-e] [-t] [-f] [-F] [-W] [-R] [-n] [-g] [-h] [-b bundle_identifier] [-a application]
file ... [--args arg1 ...]`).

- `open <dir>` opens the folder in Finder. `open -R <dir>` shows its parent with the folder
  selected.
- **Leading `-`:** the man page lists no `--` end-of-options marker **(unverified whether one
  is accepted)**. git's paths are absolute and start with `/`, so they can't be read as
  options.
- **Missing folder and exit status:** the man page has no EXIT STATUS section. **(unverified)**
  `open` prints "The file … does not exist." and exits non-zero. Not tested (no Mac).

### 4.3 Linux and other Unix

`xdg-open` (XDG v1.2.1):

- **Arguments:** exactly one. Anything starting with `-` is rejected as "unexpected option"
  with a syntax error, exit 1 (`xdg-open.in`, the `while [ $# -gt 0 ]` loop). An absolute
  path starts with `/`. **(derived)** A relative path would need a `./` in front.
- **Exit codes** (`xdg-open.xml`, "Exit Codes"): 0 success, 1 syntax error, 2 "One of the
  files passed on the command line did not exist", 3 required tool not found, 4 "The action
  failed".
- **What it runs depends on the desktop.** KDE runs `kde-open`/`kde-open5`/`kfmclient exec`.
  GNOME and Cinnamon run `gio open` (older ones `gvfs-open`/`gnome-open`). Xfce, MATE, LXDE and
  others have their own tools. Each branch maps that tool's non-zero exit to 4. Only
  `open_generic` checks the file itself (`check_input_file`, exit 2), before it tries
  `xdg-mime` and the `.desktop` handler for `inode/directory`, then `run-mailcap`/`mimeopen`,
  then a browser. **(derived)** A missing folder gives 2 in the generic mode and 4 (or
  whatever the desktop tool does) under GNOME or KDE.
- **Blocking:** "In case of success the process launched from the .desktop file will not be
  forked off and therefore may result in xdg-open running for a very long time"
  (`xdg-open.xml`). `browser.rs` already spawns and reaps on a thread, so parterre never
  waits. **(derived)** An exit code can only be read after the child ends, which in the
  generic mode is when the file manager closes.
- `org.freedesktop.FileManager1.ShowFolders` over D-Bus is the other common way to ask a file
  manager to open folders **(unverified: freedesktop.org's spec page refused the fetch)**.

### 4.4 Common to all three

**(derived)**:

- The path comes from `worktree list`, and the folder can vanish at any time.
  `Path::is_dir()` just before running the opener is the only check that works everywhere.
  The exit code is useless on Windows with Explorer, not documented on macOS, and depends on
  the desktop on Linux.
- A folder deleted between the check and the opener opens Documents on Windows (Explorer), or
  fails quietly elsewhere.
- None of the three openers is given anything but the path. With no shell in between, `&`,
  `;`, `%` and `^` are plain characters (tested on Windows).
