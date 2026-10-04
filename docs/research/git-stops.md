# How git operations stop, and what finishing each needs

Research note for [#268](https://github.com/aquamoth/parterre/issues/268), part of the map
[Finishing a stuck worktree](https://github.com/aquamoth/parterre/issues/267). The question:
*every way merge, rebase, cherry-pick, revert and `stash pop` can leave a **stuck worktree**,
how each is recognised on disk, and what finishes it.*

It builds on [`git-refusals.md`](https://github.com/aquamoth/parterre/blob/research/git-refusals/docs/research/git-refusals.md)
(#141, "REFUSALS" below) and doesn't repeat it: the state files and the continue / skip / abort /
quit table (REFUSALS §7.3), counting conflicts (§7.4) and editors (§7.5).

A statement checked by running git is marked **(tested)**. One read in git's source or docs is
linked. My own conclusions are marked **(derived)**.

All tests ran on 2026-10-04 on Linux, in throwaway repositories, with three gits:

- **2.34.1**, the hard minimum, built from the kernel.org tarball.
- **2.43.0**, Ubuntu 24.04's package (the installed one).
- **2.56.0**, the latest release, built from the kernel.org tarball.

Every command ran with `LC_ALL=C`, `GIT_CONFIG_NOSYSTEM=1`, `HOME` an empty folder (no user
config), stdin `/dev/null` and `GIT_EDITOR=:`, as parterre's operation runner does. Where
nothing is said, the three gave the same result apart from ids, hint lines and progress output.
Where they differ, the text says so.

## TL;DR

1. **Stops without conflicts are many, and they don't look alike on disk.** A hook that
   refuses, a pick that became empty, a rebase's `edit`/`break`/`exec`, a pick blocked by an
   untracked file. §1 has a table of what each leaves and what finishes it. **`edit` and `break`
   exit 0** while the rebase is stopped. Check the state files after every operation, not just
   the exit code.
2. **Two stops leave no operation in progress at all.** A single `revert` whose
   `prepare-commit-msg` hook refuses leaves the revert's changes staged and no `REVERT_HEAD`.
   A single `revert` that comes out empty leaves nothing (§1.2, §1.3). Neither is stuck. Parterre
   has to tell them from the exit code. The leftover `MERGE_MSG` becomes the next commit's
   message.
3. **`sequencer/` alone has three causes**, and `--continue` is wrong for one of them (§1.5).
   - A pick committed by hand: `--continue` is right.
   - A pick blocked before it started, by an untracked file or a local change: **`--continue`
     silently skips that commit** (all three versions, and in the source). Tell it by HEAD
     still equalling `sequencer/abort-safety` with a clean index.
   - A range `revert` whose hook refused: changes are staged and `--continue` refuses. Commit
     them, then continue.
4. **A refused hook during a rebase depends on the version** (§1.2).
   - 2.34 and 2.43 reschedule the pick. 2.56 doesn't.
   - In 2.43, `rebase --continue` then refuses with "you have staged changes".
   - `git commit --no-edit`, then `rebase --continue`, works on all three. When parterre's todo
     list is in use, an extra stop on an empty pick may follow; `--continue` again ends it.
5. **Per-file *Use mine*/*Use theirs* works for 9 of the 13 setups tested** (§2). The rule: if
   the side's stage exists, `checkout --ours|--theirs -- <path>` and `add`; if it doesn't, `rm`.
   It fails for three kinds:
   - **A submodule.** `checkout --theirs` does nothing and `add` stages the checked-out commit.
     Use `update-index --cacheinfo 160000,<oid>,<path>`.
   - **A file against a directory** (both ways), and **a symlink against a file.** git renames
     one side to `<path>~<label>`, so taking that side means a rename. The directory side's
     files aren't even unmerged.

   `git restore --theirs` on a path with no stage 3 **deletes the file and exits 0**. Use
   `checkout`, which refuses with exit 1.
6. **During a rebase, *ours* is the branch being rebased onto, and *theirs* is your own
   commit.** For a revert, *theirs* is the reverted commit's parent. For `stash pop`, *theirs*
   is the stash. For `checkout -m`, *ours* is the new branch and *theirs* your local changes
   (§2.3).
7. **`--continue` checks only the index** (§3). Conflict markers are committed if staged.
   `git diff --check` finds them (exit 2).
   - A file resolved to exactly HEAD's version: merge commits it; rebase drops the commit;
     cherry-pick and revert **stop again** as empty.
   - `rebase --continue` refuses any unstaged change; the others leave it alone.
8. **Aborts differ** (§4).
   - `merge`/`cherry-pick`/`revert --abort` are `reset --merge`. They keep untracked files and
     unstaged edits to files the operation didn't touch. They lose resolutions and staged edits.
     **They refuse (exit 128, nothing changed) when a file the operation auto-merged has been
     edited since.**
   - `rebase --abort` resets all tracked files and keeps untracked ones.
9. **Cheap "anything done since the stop?"** (§4.4), all three versions:
   - `git ls-files --resolve-undo`: files resolved since this stop.
   - `git diff --name-only AUTO_MERGE`: files edited since the stop. `AUTO_MERGE` is the tree
     ort wrote at the stop, conflict markers included; 2.34 writes it too.
   - `git diff --cached --name-only AUTO_MERGE`, minus the unmerged paths: edits staged since
     the stop.
   - Untracked files are never lost.
10. **Autostash** (§5).
    - When the stash can't be put back, git leaves conflicted files with markers and stages the
      rest. It keeps a stash entry named `autostash` and **exits 0**. No operation is in
      progress.
    - While a merge or rebase is stopped, the autostash waits in `MERGE_AUTOSTASH` or
      `rebase-merge/autostash`. `--abort` puts it back; `--quit` stores it as an entry.
    - `stash pop` with conflicts keeps the entry and stages the non-conflicting changes.
    - `checkout -m` keeps a stash entry only since 2.55. Before that, the local changes exist
      only in the conflicted files, and `reset --merge` loses them.
11. **rerere** (§6). With `rerere.enabled`, a recorded resolution is written to the file but the
    file **stays unmerged**, so parterre's list is right. With `rerere.autoUpdate` it is staged,
    and the operation still stops. `git rerere remaining` lists what rerere didn't resolve.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT | git `v2.56.0` @ `a0189536` (2026-09-28) | https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/ |
| GIT234 | git `v2.34.1` | https://github.com/git/git/blob/v2.34.1/ |
| REFUSALS | [`docs/research/git-refusals.md`](https://github.com/aquamoth/parterre/blob/research/git-refusals/docs/research/git-refusals.md) on `research/git-refusals` (#141) | |

Which release a change landed in was checked by comparing the commit against the release tags
(GitHub's compare API) and against the tested behaviour.

---

## 0. What parterre detects today

`in_progress` (`crates/parterre-core/src/branches.rs`) reports, in this order: `rebase-merge/`,
`rebase-apply/` (`git am` if `applying`), `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
`sequencer/`, `BISECT_START`. `Catalog::stuck` adds conflicted files (`diff --diff-filter=U`)
with no operation. Every stop below is caught by one of those, **except** the two revert cases
in TL;DR 2. Those leave a dirty worktree, which isn't stuck. **(derived)**

## 1. Stops without conflicts

### 1.1 Which hooks run

All **(tested)**, identical in the three versions. A logging script was installed for every
client hook.

| Operation | Clean run | After a conflict: `--continue` |
|---|---|---|
| `merge` | `pre-merge-commit`, `prepare-commit-msg`, `commit-msg`, `post-merge` | `pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit` (also with `commit --no-edit`) |
| `rebase` (merge backend; also `-i`) | `pre-rebase`, `post-checkout`, then per pick `prepare-commit-msg`, `post-commit`; `post-rewrite` | `prepare-commit-msg`, `post-commit`, `post-rewrite` |
| `rebase -i` with `reword` | adds `pre-commit` and `commit-msg` for that commit | |
| `rebase --apply` | `pre-rebase`, `post-checkout`, `applypatch-msg`, `pre-applypatch`, `post-applypatch`, `post-rewrite` | `pre-applypatch`, `post-applypatch`, `post-rewrite` |
| `cherry-pick`, `revert` | `prepare-commit-msg`, `post-commit` | `pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit` |
| `stash pop` | none | |

So a clean pick in a rebase, cherry-pick or revert never runs `pre-commit` or `commit-msg`. Only
`prepare-commit-msg` can refuse there. `--continue` after a conflict runs `git commit`, so all
the commit hooks can refuse it. `pre-merge-commit` runs only in `git merge` itself: "If the merge
cannot be carried out automatically … this hook will not be executed, but the 'pre-commit' hook
will" ([githooks.adoc#L115-L136](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/githooks.adoc#L115-L136)).
Failing `post-*` hooks change nothing (tested: `post-commit` exit 1, cherry-pick exit 0).

### 1.2 A hook refuses

Each hook was set to `exit 1` in turn; then the hook was removed and the finish tried.
All **(tested)**.

| Case | Exit | Left behind | Finish |
|---|---|---|---|
| `merge`, `pre-merge-commit` / `prepare-commit-msg` / `commit-msg` refuses | 1 | `MERGE_HEAD`, all changes staged, no unmerged files. `Not committing merge; use 'git commit' to complete the merge.` | `merge --continue` or `commit --no-edit` |
| `merge --continue`, `pre-commit` refuses | 1 | Unchanged | Again, or `commit --no-verify --no-edit` |
| `cherry-pick` (one or a range), `prepare-commit-msg` refuses | 128 (2.56: **1**) | `CHERRY_PICK_HEAD` (+ `sequencer/`), changes staged | `cherry-pick --continue` |
| `cherry-pick --continue`, `commit-msg` refuses | 1 | Unchanged | Again |
| **`revert` of one commit**, `prepare-commit-msg` refuses | 128 (2.56: 1) | **No `REVERT_HEAD`**: the changes are staged, `MERGE_MSG` is left, nothing is in progress. `revert --continue` says `no cherry-pick or revert in progress` | `git commit` (uses `MERGE_MSG`) |
| `revert` of a range, `prepare-commit-msg` refuses | 128 (2.56: 1) | `sequencer/` only, changes staged. `revert --continue` refuses: `your local changes would be overwritten by revert` | `git commit`, then `revert --continue` |
| `rebase`, `pre-rebase` refuses | 128 (2.56: 1) | Nothing: refused before starting | — |
| `rebase --apply`, `pre-applypatch` refuses | 1 | `rebase-apply/`, changes staged | `rebase --continue` |
| `rebase`, `prepare-commit-msg` refuses | 1 | `rebase-merge/`, `REBASE_HEAD`, **`CHERRY_PICK_HEAD`**, changes staged | See below |

Why revert differs from cherry-pick: the sequencer writes `CHERRY_PICK_HEAD` when the pick
merged cleanly or with conflicts, but `REVERT_HEAD` only on conflicts (or with `--no-commit`)
([sequencer.c#L2500-L2515](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L2500-L2515),
same in [GIT234 sequencer.c#L2263-L2278](https://github.com/git/git/blob/v2.34.1/sequencer.c#L2263-L2278)).

**A refused hook in a rebase** (merge backend):

| | 2.34 | 2.43 | 2.56 |
|---|---|---|---|
| The pick | rescheduled (back at the top of the todo list) | rescheduled; prints `Could not execute the todo command … It has been rescheduled` | not rescheduled |
| `rebase-merge/message`, `stopped-sha` | written | **not** written | written |
| `rebase --continue` | commits the staged changes, then replays the rescheduled pick | **refuses**: `error: you have staged changes in your working tree` (exit 1) | commits and goes on |
| `commit --no-edit`, then `rebase --continue` | works | works | works |
| `reset --hard`, then `rebase --continue` | re-picks it | re-picks it | **loses the commit** |

The 2.43 refusal comes from "rebase --continue: refuse to commit after failed command"
([405509c](https://github.com/git/git/commit/405509cbd6), in 2.43). It treats the missing
`message` file as "the pick never happened". 2.56 stopped rescheduling with "sequencer: never
reschedule on failed commit" ([18f5750](https://github.com/git/git/commit/18f5750beb), new in
2.56.0).

The rescheduled pick is replayed on top of the commit just made, so it comes out empty:

- A non-interactive rebase drops it ("patch contents already upstream").
- A `rebase -i`, which parterre uses whenever it writes a todo list, **stops on it as empty**
  (2.34 after `--continue`; 2.43 after commit + `--continue`). One more `--continue` drops it
  (tested with a todo written by `GIT_SEQUENCE_EDITOR=cp`).

**(derived)** For parterre: on a rebase stopped with `CHERRY_PICK_HEAD` and staged changes,
*Continue* = `commit --no-edit`, then `rebase --continue`. A following empty stop on the same
commit is the rescheduled copy. `git commit` takes the author from `CHERRY_PICK_HEAD` and the
message from `MERGE_MSG` (tested: the commit kept its subject).

`--no-verify` skips `pre-commit` and `commit-msg` (and `pre-merge-commit` for `merge`), but
`prepare-commit-msg` "is not suppressed by the `--no-verify` option"
([githooks.adoc#L97-L166](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/githooks.adoc#L97-L166)).
**(derived)** A `prepare-commit-msg` that always refuses can't be bypassed per command. So
*Continue* after a refused hook means "try again after fixing what it complained about".

### 1.3 A pick that comes out empty

Setup: the branch already has the change of the commit being picked. All **(tested)**.

| Case | Exit | Left behind | Finish |
|---|---|---|---|
| `cherry-pick` of a commit whose change is already there | 1 | `CHERRY_PICK_HEAD` (+ `sequencer/` in a range), clean tree, nothing staged. `The previous cherry-pick is now empty, possibly due to conflict resolution.` | `cherry-pick --skip`, or `commit --allow-empty` to keep it. **`--continue` stops again the same way.** `--allow-empty` on the original command doesn't help; `--keep-redundant-commits` does |
| `cherry-pick` of a commit that is itself empty | 1 | Same | Same |
| `revert` whose change is already undone | 1 | **Nothing in progress**, only `MERGE_MSG`. Prints `nothing to commit, working tree clean` | Nothing. `revert --skip` refuses: `no revert in progress` |
| `rebase` (no `-i`), a commit that becomes empty | 0 | Dropped: `dropping … patch contents already upstream` | — |
| **`rebase -i`** (and parterre's todo), a commit that becomes empty | 1 | `rebase-merge/`, `REBASE_HEAD`, **`CHERRY_PICK_HEAD`**, clean tree | `--continue` or `--skip` **drop** it; `commit --allow-empty`, then `--continue`, keeps it |

`rebase -i` implies `--empty=ask` (2.56 calls it `stop`, with `ask` a deprecated synonym)
([GIT234 git-rebase.txt#L265-L280](https://github.com/git/git/blob/v2.34.1/Documentation/git-rebase.txt#L265-L280),
[git-rebase.adoc#L259-L276](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-rebase.adoc#L259-L276)).
In the test, the duplicate wasn't a clean cherry-pick of an upstream commit, so the todo list
kept it. A leftover `MERGE_MSG` with nothing in progress is used by the next `git commit`: after
the empty revert, an unrelated `commit --no-edit` got the subject `Revert "one"` (tested, all
three).

### 1.4 Rebase stops of `edit`, `break` and `exec`

These come only from a todo list the user wrote. All **(tested)**, the same in the three
versions, from `rebase -i` with a todo list copied in by `GIT_SEQUENCE_EDITOR`.

| Stop | Exit | `rebase-merge/` has | Other files | Last line of `done` | Finish |
|---|---|---|---|---|---|
| `edit` | **0** | **`amend`**, `stopped-sha`, `message`, `author-script` | `REBASE_HEAD` | `edit <sha>` | `--continue`. Staged changes amend the commit; **an unstaged change makes `--continue` refuse** with `You must edit all merge conflicts and then mark them as resolved using git add` |
| `break` | **0** | no `amend`, no `stopped-sha` | none | `break` | `--continue` |
| `exec` failed | 1 | no `amend`, no `stopped-sha` | none | `exec <cmd>` | `--continue` (doesn't re-run it) |
| `exec` succeeded but left changes | 1 | same | unstaged changes | `exec <cmd>` | Commit or stash, then `--continue` |
| Conflict, for comparison | 1 | `stopped-sha`, `message`, `patch`, `author-script` | `REBASE_HEAD`, unmerged files | `pick <sha>` | Resolve, `--continue` |

`rebase -x <cmd>` without `-i` stops on a failed `exec` the same way. The map gives these stops
*Continue* and *Abort* only, so `amend` and the last `done` line are all parterre needs to
name them **(derived)**.

### 1.5 `sequencer/` with no `CHERRY_PICK_HEAD` or `REVERT_HEAD`

All **(tested)**, identical in the three versions:

| Cause | Index | HEAD vs `sequencer/abort-safety` | What `--continue` does |
|---|---|---|---|
| A, a range pick stopped on a conflict; the user resolved it and ran `git commit` (REFUSALS §7.3) | Clean | **Differs** (the user's commit) | Picks the rest: right |
| B, a range pick or revert where one commit was **blocked before it started** by an untracked file (`The following untracked working tree files would be overwritten by merge`, exit 128) or a local change | Clean, the untracked file is still there | **Equal** | **Skips the blocked commit** and picks the rest, exit 0. `--skip` does the same |
| C, a range `revert` whose `prepare-commit-msg` refused (§1.2) | **Staged** | Equal | Refuses until committed |

The skip is in the source: without a `_HEAD` ref, `sequencer_continue` checks that the index
matches HEAD and then advances past the current item (`todo_list.current++`), assuming it was
committed by hand ([sequencer.c#L5612-L5629](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L5612-L5629),
[GIT234 sequencer.c#L4812-L4825](https://github.com/git/git/blob/v2.34.1/sequencer.c#L4812-L4825)).
`abort-safety` holds HEAD after the last pick the sequencer made itself
([sequencer.c#L633-L645](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L633-L645),
written at the end of every pick, [#L2585](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L2585)).

**(derived)** Telling them apart:

- `HEAD != abort-safety`: case A, *Continue*.
- Equal and something staged: case C, `commit`, then *Continue*.
- Equal and clean: case B. *Continue* would lose the first commit in `sequencer/todo`. Offer
  *Abort*, or "remove the file, then pick the rest": `cherry-pick --quit` and a new
  `cherry-pick` of the remaining commits (tested: picks them all).

A single blocked pick (no range) leaves nothing (exit 128). The same block inside a rebase is
rescheduled (`REBASE_HEAD`, no `message`, the pick back at the top of `git-rebase-todo`, the
untracked file left). Removing the file and running `--continue` replays it (tested). `merge`
blocked the same way changes nothing (exit 2; 128 in 2.34, REFUSALS §7.2).

## 2. Kinds of conflict

### 2.1 What each looks like

`git merge theirs` into `main`, ort (the default since 2.34). All **(tested)**, identical in
the three versions apart from the submodule's hint text. In `status --porcelain=v2` an
unmerged entry is `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`; the XY codes are in
[git-status.adoc#L236-L245](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-status.adoc#L236-L245).

| Kind | git says | `ls-files -u` stages | v2 XY | Worktree |
|---|---|---|---|---|
| Content | `CONFLICT (content)` | 1 2 3 | `UU` | Markers |
| Modified here, deleted there | `CONFLICT (modify/delete): f deleted in theirs and modified in HEAD. Version HEAD of f left in tree.` | 1 2 | `UD` | Ours |
| Deleted here, modified there | `… deleted in HEAD and modified in theirs. Version theirs of f left in tree.` | 1 3 | `DU` | Theirs |
| Added on both sides | `CONFLICT (add/add)` | 2 3 | `AA` | Markers |
| Renamed differently on each side (`a`→`b` here, `a`→`c` there) | `CONFLICT (rename/rename)` | `a`: 1; `b`: 2; `c`: 3 | `a` `DD`, `b` `AU`, `c` `UA` | `b` and `c` |
| Renamed here, deleted there | `CONFLICT (rename/delete)` | `b`: 1 2 | `UD` | `b` |
| Deleted here, renamed there | `CONFLICT (rename/delete)` | `c`: 1 3 | `DU` | `c` |
| Renamed here onto a file added there | `CONFLICT (add/add)` | 2 3 | `AA` | Markers |
| Binary | `warning: Cannot merge binary files` + `CONFLICT (content)` | 1 2 3 | `UU` | **Ours**, no markers |
| Symlink, both retargeted | `CONFLICT (content)` | 1 2 3, mode 120000 | `UU` | Our link |
| Symlink here, regular file there | `CONFLICT (distinct types) … renamed one of them` | `link`: 1 2; **`link~theirs`**: 3 | `UD`, `UA` | Our link, their file as `link~theirs` |
| File here, directory there | `CONFLICT (file/directory): directory in the way of d from HEAD; moving it to d~HEAD instead.` | **`d~HEAD`**: 2 only | `AU`; `d/x` is staged as added (`1 A.`), not unmerged | `d/x` and `d~HEAD` |
| Submodule moved on both sides, diverging | `CONFLICT (submodule)` + how to merge it | 1 2 3, mode 160000 | `UU`, sub `S...` | The submodule stays at ours |

The `~HEAD` / `~theirs` suffix is the side's label.

### 2.2 Use mine, use theirs, delete

The rule tested, per unmerged path: if the side's stage (2 = mine, 3 = theirs) exists,
`checkout --ours|--theirs -- <path>` and `add`; otherwise `rm`. Applied to every unmerged path,
then the index compared with that side's commit. All **(tested)**, identical in the three
versions.

| Kind | Mine | Theirs |
|---|---|---|
| Content, add/add, binary, symlink, modify/delete both ways, rename/delete both ways, rename/rename (`a` and the other name `rm`) | Exact | Exact |
| Submodule | Exact, but only because the submodule was checked out at ours | **Wrong**: `checkout --theirs sub` exits 0 and changes nothing. `add sub` stages whatever the submodule has checked out (ours) |
| Symlink against file | Exact | **Wrong**: their file stays as `link~theirs` |
| File against directory | **Wrong**: our file stays as `d~HEAD`, and their `d/x` stays staged | Exact |
| Directory against file | Exact | **Wrong**, likewise |

What works where the rule fails (tested):

- **Submodule, either side:** `git update-index --cacheinfo 160000,<oid of stage N>,<path>`. That
  leaves the submodule's checkout behind (`status` `MM SC..`) until `submodule update`. Or
  check out that commit in the submodule, then `add`.
- **Renamed-aside side:** `rm -r -f -- d` (the other side, including staged non-conflicted
  files), `mv 'd~HEAD' d`, `rm --cached -- 'd~HEAD'`, `add d`. Then the index matched the side's
  commit.

Pitfalls (tested):

- `git checkout --theirs -- f` with no stage 3: `error: path 'f' does not have their version`,
  exit 1. **`git restore --theirs -- f` exits 0 and deletes `f`** from the worktree, leaving it
  unmerged.
- `git checkout -m -- <path>` recreates the conflict in the file, from the index stages or the
  resolve-undo record, with `ours`/`theirs` labels. `--conflict=diff3` adds the base.

**(derived)** *Use mine*/*Use theirs* per file fits every kind except the three above. A
submodule needs `update-index`. A path named `<x>~<label>` belongs to a pair that has to be
resolved together, not one file at a time.

### 2.3 Which side is "ours"

Content conflict on `f`; all **(tested)**, identical in the three versions:

| Operation | `<<<<<<<` label | `>>>>>>>` label | `--ours` gives | `--theirs` gives |
|---|---|---|---|---|
| `merge topic` on main | `HEAD` | `topic` | main | topic |
| `rebase main` on topic | `HEAD` | `<sha> (<subject>)` | **main** (onto) | **topic** (the commit being replayed) |
| `rebase --apply main` | `HEAD` | `<subject>` | main | topic |
| `cherry-pick topic` on main | `HEAD` | `<sha> (<subject>)` | main | topic |
| `revert X` | `HEAD` | `parent of <sha> (<subject>)` | HEAD | X's parent's version |
| `stash pop` | `Updated upstream` | `Stashed changes` | HEAD | the stash |
| `checkout -m other` | `other` | `local` | the branch switched to | your local changes |

"The side reported as 'ours' is the so-far rebased series, starting with `<upstream>`, and
'theirs' is the working branch. In other words, the sides are swapped"
([git-rebase.adoc#L341-L346](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-rebase.adoc#L341-L346);
also [git-checkout.adoc#L136-L140](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-checkout.adoc#L136-L140)).
**(derived)** *Mine* is stage 2 everywhere except a rebase, where *mine* (the user's commit) is
stage 3. `checkout -m` is the same inversion.

## 3. What `--continue` needs

All **(tested)**, identical in the three versions.

- **No unmerged entries, nothing else.** A file staged with its conflict markers was committed
  by `merge`, `rebase`, `cherry-pick` and `revert --continue` alike. `git diff --check` (exit 2,
  `c:1: leftover conflict marker`) and `git diff --cached --check` find markers. Both also exit 2
  on whitespace errors, so read their lines rather than the exit code
  ([diff-options.adoc#L505](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/diff-options.adoc#L505)).
- **`git add` of a conflicted file without editing it** resolves it, markers and all.
- **Resolved to exactly HEAD's version** (`checkout --ours`, `add`):
  - `merge --continue`: makes the merge commit.
  - `rebase --continue`: drops the commit and goes on.
  - `cherry-pick --continue`: stops again, `The previous cherry-pick is now empty`, exit 1,
    `CHERRY_PICK_HEAD` kept. `revert --continue` likewise, with `nothing to commit`. Finish
    with `--skip` or `commit --allow-empty`.
- **An unstaged change to another tracked file:** `merge`, `cherry-pick` and `revert --continue`
  commit the index and leave it. **`rebase --continue` refuses**, exit 1, with the misleading
  `You must edit all merge conflicts and then mark them as resolved using git add`.
- **Editor:** `merge --continue` and `rebase --continue` called `GIT_EDITOR`;
  `cherry-pick`/`revert --continue` and `commit --no-edit` didn't (re-confirms REFUSALS §7.5 on
  all three).
- **It can stop again:**
  - `rebase` and a range `cherry-pick`/`revert` stop on the next conflict, exit 1.
  - Cherry-pick and revert stop on an empty result (above).
  - Any commit hook can refuse (§1.2).
  - An autostash may conflict when it is put back, with exit 0 (§5).

## 4. Abort

### 4.1 What each abort keeps

Each operation was stopped on a conflict in `c`, with `a` auto-merged. Then one kind of change
was made and the operation aborted. `stash pop` is undone with `reset --merge`. All **(tested)**,
identical in the three versions.

| Change since the stop | `merge`/`cherry-pick`/`revert --abort` | `rebase --abort` | `reset --merge` after `stash pop` |
|---|---|---|---|
| Conflicted file resolved, staged or not | Lost | Lost | Lost (the stash entry is kept) |
| Conflicted file only `git add`ed | Lost (nothing of value) | Lost | Lost |
| **Auto-merged file edited** | **Abort refused**, exit 128: `error: Entry 'a' not uptodate. Cannot merge.` / `fatal: Could not reset index file to revision 'HEAD'.` Nothing changes; still in progress | Lost | — |
| Unrelated tracked file edited, unstaged | **Kept** | Lost | Kept |
| Unrelated tracked file edited and staged | Lost | Lost | Lost |
| New untracked file | Kept | Kept | Kept |
| Local change from before the operation (merge and cherry-pick allow unrelated ones) | Kept | (rebase refuses to start dirty) | — |

- `merge --abort` "is equivalent to `git reset --merge` when `MERGE_HEAD` is present", except
  that it puts an autostash back
  ([git-merge.adoc#L108-L115](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-merge.adoc#L108-L115)).
  The docs warn that it "will in some cases be unable to reconstruct" changes made before or
  during the merge ([#L53-L58](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-merge.adoc#L53-L58)).
  The refusal above is that case.
- **Past the refusal** (tested): `git checkout -- a` (the index holds the merged version), then
  `merge --abort` worked and kept the other local changes. `git reset --hard` also ends it, but
  takes every tracked change with it.
- `rebase --abort` after some picks are done returns the branch to where it was. The picks made
  so far are dropped (tested).

### 4.2 Abort after the user committed by hand

In a range cherry-pick or revert, committing by hand and then `--abort` printed `warning: You seem
to have moved HEAD. Not rewinding, check your HEAD!` and exited 0. It forgot `sequencer/` and
kept every commit (tested). The check compares HEAD with `abort-safety`
([sequencer.c#L3518-L3540](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L3518-L3540),
[#L3613-L3617](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L3613-L3617);
[GIT234 #L3098](https://github.com/git/git/blob/v2.34.1/sequencer.c#L3098), [#L3198](https://github.com/git/git/blob/v2.34.1/sequencer.c#L3198)).

### 4.3 Conflicted files with no operation

- After `stash pop`/`apply` or an autostash that conflicted, `reset --merge` restored HEAD's
  version and the stash entry stayed. The non-conflicting stashed changes, which `pop` had
  staged, were reset too; they are still in the entry. Unstaged edits to other files were kept
  (tested).
- **`checkout -m <branch>`, before 2.55**: the local changes exist only as stage 3 and in the
  markers. No stash entry is made, so `reset --merge` **loses** the conflicted file's local
  change (tested on 2.34 and 2.43: `f` went back to the branch's version; an unconflicted local
  change in `g` stayed). **Since 2.55** `checkout -m` makes an autostash (`autostash while
  switching to 'other'`, kept on conflict; tested on 2.56) ("checkout -m: autostash when
  switching branches", [c07039e](https://github.com/git/git/commit/c07039ebc4)).
- Nothing on disk says which command left the conflicts. The stash list's top entry's message
  (`autostash`, `autostash while switching to …`, `WIP on …`) is the only clue **(derived)**.

### 4.4 Has anything been resolved or edited since the stop?

`AUTO_MERGE` "records a tree object corresponding to the state the 'ort' merge strategy wrote
to the working tree when a merge operation resulted in conflicts"
([revisions.adoc#L75-L78](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/revisions.adoc#L75-L78)).
"Comparing the working tree with `AUTO_MERGE` shows changes you've made so far to resolve
textual conflicts" ([git-diff.adoc#L106-L113](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-diff.adoc#L106-L113)).
The docs only mention it from 2.42, but **2.34's ort writes it too**
([GIT234 merge-ort.c#L4229-L4234](https://github.com/git/git/blob/v2.34.1/merge-ort.c#L4229-L4234)).
In a reftable repository it is a ref, not a file (tested on 2.56), so ask git with
`rev-parse`/`diff`, don't read the file.

The resolve-undo extension records the stages of each path resolved by `git add`/`rm`. It is
what `checkout -m <path>` uses
([git-ls-files.adoc#L92-L97](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-ls-files.adoc#L92-L97)).
`ls-files --resolve-undo` works on 2.34 but isn't in its manual.

What three cheap commands showed, per change since the stop (tested, the three versions, all
operations):

| Change | `diff --name-only AUTO_MERGE` | `diff --cached --name-only AUTO_MERGE` | `ls-files --resolve-undo` |
|---|---|---|---|
| None | — | the unmerged paths only | — |
| `add` without editing | — | — | the path |
| Resolved and staged | the path | the path | the path |
| Resolved, not staged | the path | (unmerged) | — |
| Auto-merged file edited | the path | (unmerged only) | — |
| Unrelated file edited / staged | the path | — / the path | — |
| Untracked file | — | — | — |

**(derived)** "Touched since the stop" = `ls-files --resolve-undo` ∪ `diff --name-only
AUTO_MERGE` ∪ (`diff --cached --name-only AUTO_MERGE` minus `ls-files -u`). Empty means *Abort*
loses nothing, so it can be one click. It's a superset of what an abort loses, so it may warn
needlessly:

- **Local changes from before a merge or cherry-pick** show (they aren't in `AUTO_MERGE`) but
  survive the abort.
- **Files rerere resolved** show (§6).
- **Unstaged unrelated edits** show, but `merge`/`cherry-pick`/`revert --abort` keep them.

Caveats, all tested:

- **Resolve-undo is cleared by the next commit.** After `rebase --continue` stopped on the next
  conflict it was empty again, so it describes this stop only, as wanted.
- **`AUTO_MERGE` is per merge step.** A new stop writes a new one. It survives a clean finish
  (seen after a clean cherry-pick and rebase), so trust it only while an operation is in
  progress.
- **`stash pop`/`apply` on 2.34 writes no `AUTO_MERGE`** (2.43 and 2.56 do). A stale one from
  an earlier merge may be there. With no operation in progress on 2.34, treat the answer as
  unknown and warn.

## 5. Autostash and `stash pop`

All **(tested)**; the same in the three versions except the message wording (2.56: `Your local
changes are stashed, however applying them resulted in conflicts. … run "git reset --hard" and
apply the local changes later by running "git stash pop".`; 2.34/2.43: `Applying autostash
resulted in conflicts. Your changes are safe in the stash.`).

| Case | Exit | What's left |
|---|---|---|
| `rebase --autostash` finishes; the stash conflicts when put back | **0** | No operation. Conflicted files with `Updated upstream`/`Stashed changes` markers; the stash's other changes **staged**; entry `stash@{0}: autostash` kept |
| `merge --autostash` finishes; same | **0** | Same |
| `rebase --autostash` stops on a conflict | 1 | Stash commit in `rebase-merge/autostash`, nothing in the stash list |
| … then `rebase --abort` | 0 | `Applied autostash.` Local changes back, unstaged |
| … then `rebase --quit` | 0 | `Autostash exists; creating a new stash entry.` Conflicts stay |
| `merge --autostash` stops on a conflict | 1 | `MERGE_AUTOSTASH` (a ref; a file in a files repository). Prints `When finished, apply stashed changes with git stash pop` (2.43, 2.56; not 2.34). Git applies it itself on continue/abort |
| … then `merge --continue` | 0 | Commits, then `Applied autostash.`, or, if it conflicts, the same as the first row (exit 0) |
| … then `merge --abort` | 0 | `Applied autostash.` |
| … then `merge --quit` | 0 | Entry stored; conflicts stay |
| `stash pop` conflicts | 1 | Entry **kept** (`The stash entry is kept in case you need it again.`). Non-conflicting changes staged, conflicted files `UU`. No operation |
| `stash pop` refused (a local change in the way) | 1 (2.56: **128**) | Nothing changed; entry kept |
| `stash apply --index` with a conflict in the index | 1 (2.56: 128) | Nothing changed: `conflicts in index. Try without --index.` |

- "Applying the state can fail with conflicts; in this case, it is not removed from the stash
  list" ([git-stash.adoc#L109-L111](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-stash.adoc#L109-L111)).
- **Since 2.56, exit 1 means conflicts and only conflicts**
  ([git-stash.adoc#L429-L436](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-stash.adoc#L429-L436),
  "stash: reserve exit status 1 for conflicts", [786fc39](https://github.com/git/git/commit/786fc39046)).
  Before that, 1 meant either.
- **(derived)** An autostash that conflicted is visible only as conflicted files plus a new top
  stash entry whose message is `autostash`. Parterre already counts stash entries before and
  after (`merge.rs`, `rebase.rs`). That count, not the exit code, is the signal.

## 6. rerere

All **(tested)**, identical in the three versions. A resolution was recorded by resolving and
committing once, then the same merge was redone.

| Setting | Message | Index | Operation |
|---|---|---|---|
| `rerere.enabled` | `Resolved 'c' using previous resolution.` | **Still unmerged** (`UU`); the file holds the resolution, no markers | Stops as usual; `--continue` refuses until `add` |
| `rerere.enabled` + `rerere.autoUpdate` | `Staged 'c' using previous resolution.` | Resolved | **Still stops** (merge exit 1, `MERGE_HEAD`; rebase exit 1, `rebase-merge/`); needs `--continue` |
| No setting, but `.git/rr-cache` exists | Same as `rerere.enabled` | | rerere is on by default once `rr-cache` exists |

- The same happens for rebase and cherry-pick.
- `git rerere remaining` lists the conflicted paths rerere didn't resolve. `status` and `diff`
  printed nothing in the fully resolved case.
- `MERGE_RR` stays after the operation ends, so it says nothing about state.
- `rerere.autoUpdate` "updates the index with the resulting contents after it cleanly resolves
  conflicts", default false
  ([config/rerere.adoc#L1-L4](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/rerere.adoc#L1-L4)).
  `remaining` "includes paths whose resolutions cannot be tracked by rerere, such as
  conflicting submodules" ([git-rerere.adoc#L60-L64](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-rerere.adoc#L60-L64)).

**(derived)** rerere doesn't change what parterre lists: the index still decides. A file rerere
resolved shows as conflicted with no markers. *Mark as resolved* stages it without a marker
warning. It does show up in `diff AUTO_MERGE` (the tree has the markers ort wrote; tested), so
§4.4 counts it as touched.

## 7. Version differences found

| Behaviour | 2.34.1 | 2.43.0 | 2.56.0 |
|---|---|---|---|
| Rebase pick whose `prepare-commit-msg` refused | Rescheduled; `--continue` commits | Rescheduled; **`--continue` refuses** | Not rescheduled |
| Exit when a hook refuses a clean cherry-pick/revert, or `pre-rebase` refuses | 128 | 128 | 1 |
| `stash pop`/`apply` refused (not a conflict) | 1 | 1 | 128 |
| `stash pop`/`apply` writes `AUTO_MERGE` | No | Yes | Yes |
| `checkout -m` keeps an autostash entry | No | No | Yes (since 2.55) |
| `merge --autostash` stop hint | — | `When finished, apply stashed changes with git stash pop` | same |
| `rebase --empty=ask` | `ask` | `ask` | `stop`, `ask` a deprecated synonym |
| Submodule conflict help after `CONFLICT (submodule)` | none | plain text | `hint:` lines |

## 8. For the map (derived)

- **Undetected states.** Two revert outcomes leave no operation in progress (TL;DR 2). They are
  not stuck worktrees. The revert operation must report them from its exit code and the staged
  changes. They need no banner.
- **Recognising a stop** without parsing messages: the operation files (§0); unmerged entries;
  `CHERRY_PICK_HEAD` inside a rebase; staged changes; `rebase-merge/amend`; the last line of
  `rebase-merge/done`; and, for `sequencer/` alone, HEAD against `abort-safety` (§1.5).
- **Continue isn't always safe.** With `sequencer/` alone, HEAD equal to `abort-safety` and a
  clean index, `--continue` drops a commit. After a refused hook in a 2.43 rebase it refuses.
  The safe forms are in §1.2 and §1.5.
- **Abort isn't always possible.** `merge`/`cherry-pick`/`revert --abort` refuse once an
  auto-merged file has been edited (§4.1). The warning that lists touched files (§4.4) is also
  where to offer discarding them.
- **Per-file actions don't resolve everything.** A submodule needs `update-index`. Renamed-aside
  pairs (`<path>~<label>`: file against directory, distinct types) need to be resolved as a pair.
  `restore --theirs` must not be used (§2.2).
- **Mine/theirs labels flip in a rebase and in `checkout -m`** (§2.3).
- **Conflicts with no operation** come from `stash pop`/`apply`, a conflicted autostash, and
  `checkout -m`. Only the last, before 2.55, loses the user's changes on undo (§4.3), which
  matters for [#185](https://github.com/aquamoth/parterre/issues/185).
