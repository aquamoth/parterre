# Moving a branch pointer

Research for [#170](https://github.com/aquamoth/parterre/issues/170), part of the map
[#137](https://github.com/aquamoth/parterre/issues/137). Feeds
[#171](https://github.com/aquamoth/parterre/issues/171). Hard constraint: git 2.34 (Ubuntu 22.04).

Claims come from git's documentation and source at tag `v2.34.0`, git's release notes, and the
source of TortoiseGit, lazygit and VS Code. Behaviour was checked with throwaway repos (and a bare
"remote" with a second clone for the push cases) on **git 2.34.1** (Ubuntu 22.04 package
`1:2.34.1-1ubuntu1.17`, in a container) and **git 2.43.0** (Ubuntu 24.04). The results are the same
on both except where marked **2.34 only**; those differences are backed by git's history below.

## Sources

| Short name | What | Link |
|---|---|---|
| RESET | `git-reset.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-reset.txt |
| RESET.C | `builtin/reset.c` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/builtin/reset.c |
| BRANCH | `git-branch.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-branch.txt |
| BRANCH.C | `branch.c` at v2.34.0 and v2.35.0 | https://github.com/git/git/blob/v2.34.0/branch.c |
| UPDREF | `git-update-ref.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-update-ref.txt |
| CHECKOUT | `git-checkout.txt`, `builtin/checkout.c` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-checkout.txt |
| REVLIST | `rev-list-options.txt` at v2.34.0 (`--exclude`) | https://github.com/git/git/blob/v2.34.0/Documentation/rev-list-options.txt |
| PUSH | `git-push.txt` at v2.34.0 (`--force-if-includes`) | https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt |
| FIX235 | commit `593a2a5d0` "branch: protect branches checked out in all worktrees", in v2.35.0 | https://github.com/git/git/commit/593a2a5d0639b4b4f91ff6e6ffb64e72020f8fd8 |
| REL | Release notes 2.30.0, 2.42.0, 2.44.0 | https://github.com/git/git/blob/v2.44.0/Documentation/RelNotes/2.44.0.txt |
| TG | TortoiseGit `master` @ `7338078f` (2026-09-27) | https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |
| TGDOC | TortoiseGit manual, Reset | https://tortoisegit.org/docs/tortoisegit/tgit-dug-reset.html |
| LG | lazygit `master` @ `ff375b12` (2026-09-30) | https://github.com/jesseduffield/lazygit/tree/ff375b124d149f03f9156d3720a998cbc04482fa |
| VSC | VS Code `extensions/git` @ `0fe38943` (2026-10-01) | https://github.com/microsoft/vscode/tree/0fe38943a280609411483eb73e0ebb45d135d6b9/extensions/git/src |
| GK | GitKraken Desktop help, Commits | https://help.gitkraken.com/gitkraken-desktop/commits/ |
| ST | Sourcetree KB, Reset branch to a commit | https://support.atlassian.com/sourcetree/kb/reset-branch-to-a-commit/ |
| FORK | Fork release notes (macOS, Windows) | https://git-fork.com/releasenotes |

## Answer in short

- **Branch checked out nowhere:** `git branch -f <b> <commit>`. It moves the ref and nothing else.
  **On git 2.34 it does not check other worktrees**: it refuses only the open worktree's branch,
  and happily moves a branch checked out, being rebased or being bisected in another worktree,
  leaving that worktree's index out of step. Git 2.35 fixed this ([FIX235](https://github.com/git/git/commit/593a2a5d0639b4b4f91ff6e6ffb64e72020f8fd8)).
  Parterre must therefore check "checked out anywhere" itself, which it already does for delete.
- **The open worktree's branch:** `git reset --keep <commit>` is git's safe form. It moves the
  branch, updates the files that differ between the two commits, keeps every uncommitted change,
  and refuses (changing nothing) if a change or an untracked file is in the way. `git reset --hard`
  is the destructive form: it discards all tracked changes, staged or not, and silently deletes
  untracked files and folders in the way. `--merge` is *not* a safe form: it silently drops staged
  changes. `--soft` and `--mixed` lose nothing but leave the old commit's content as uncommitted
  changes, which is a different intent ("undo commits, keep the work").
- **Branch checked out in another worktree:** nothing in git moves it safely from here.
  `branch -f` refuses on 2.35+, `update-ref` and (before 2.44) `checkout -B` don't, and each leaves
  that worktree's index stale. Go to that worktree.
- **Operation in progress** in the open worktree: `--soft` and `--keep` refuse; `--hard`,
  `--mixed` and `--merge` run but leave a half-state (a cherry-pick's sequencer stays; mid-rebase
  they move the detached HEAD, not the branch, and `rebase --abort` undoes it). Abort first.
- **Lost commits** before moving `<b>` from `<old>` to `<new>`: parterre's existing
  `lost_commits(start = <old>, excluded_ref = refs/heads/<b>, leaving = open worktree when resetting,
  future_root = <new>)`. In git terms:
  `git rev-list <old> ^<new> --not --exclude=<b> --branches --remotes --tags <other worktrees' HEADs>`.
- **Lost uncommitted changes** (`--hard` only): `git diff --name-only HEAD`, plus each untracked
  file whose path, or a leading folder of it, exists in `<new>`'s tree.
- **Upstream:** `reset --keep @{u}` (or `branch -f <b> <remote>/<b>`) is "make my branch match the
  remote"; afterwards nothing needs pushing. After moving a pushed branch backwards or sideways,
  the push needs a force, and `push --force-with-lease=refs/heads/<b> --force-if-includes` **accepts**
  it (the old remote tip is in the branch's reflog) unless someone pushed after it, which it
  refuses even after a background fetch. A plain `--force-with-lease` would drop their commit.
- **Other clients:** TortoiseGit, GitKraken, Sourcetree and lazygit (and Fork, modes undocumented) offer
  *Reset current branch* with soft/mixed/hard (TortoiseGit preselects mixed, lazygit lists it first).
  None offer `--keep` in that menu. TortoiseGit moves another branch only through *Create Branch*
  with *Force* (`branch -f`) or *Switch/Checkout* with *Override branch if exists* (`checkout -B`).
  lazygit's auto-forward is the closest to this research: `update-ref --stdin` with the old value
  for branches checked out nowhere, `reset --keep` for checked-out ones. VS Code only has
  *Undo Last Commit* (`reset --soft HEAD~`).

## 1. The commands and what they touch

Tested in scenario S1 (below). "Reflog" is the message in the branch's reflog.

| Command | Ref | Index | Working tree | `ORIG_HEAD` | Reflog |
|---|---|---|---|---|---|
| `branch -f <b> <c>` | `<b>` | no | no | no | `branch: Reset to <c>` |
| `switch -C <b> <c>` / `checkout -B <b> <c>` | `<b>`, then checks it out | yes | yes (like switch) | no | `branch: Reset to <c>`, HEAD `checkout: moving from …` |
| `update-ref refs/heads/<b> <c> [<old>]` | `<b>` | no | no | no | empty, or `-m <reason>` |
| `reset --soft <c>` | HEAD's branch | no | no | yes | `reset: moving to <c>` |
| `reset --mixed <c>` (default) | HEAD's branch | reset to `<c>` | no | yes | same |
| `reset --hard <c>` | HEAD's branch | reset to `<c>` | reset to `<c>` | yes | same |
| `reset --keep <c>` | HEAD's branch | files differing HEAD→`<c>` | same files only | yes | same |
| `reset --merge <c>` | HEAD's branch | reset to `<c>` | files differing HEAD→`<c>` | yes | same |

- `branch -f` "Reset <branchname> to <startpoint>, even if <branchname> exists already"
  ([BRANCH L116-L124](https://github.com/git/git/blob/v2.34.0/Documentation/git-branch.txt#L116-L124));
  the reflog text is `branch: Reset to %s` with the start point as given
  ([BRANCH.C L318](https://github.com/git/git/blob/v2.34.0/branch.c#L318)). Passing the full hash
  makes the reflog unambiguous.
- `checkout -B` is "equivalent to running `git branch` with `-f`" and then checking out
  ([CHECKOUT L152-L156](https://github.com/git/git/blob/v2.34.0/Documentation/git-checkout.txt#L152-L156)).
  It is a switch, so it doesn't fit "move a pointer" and isn't considered further.
- `update-ref <ref> <new> <old>` only writes if the ref still holds `<old>`
  ([UPDREF L19-L26](https://github.com/git/git/blob/v2.34.0/Documentation/git-update-ref.txt#L19-L26)),
  a compare-and-swap that `branch -f` lacks. It checks nothing else: in S1 it moved the open
  worktree's checked-out branch, and the worktree then showed the reverse of the dropped commit as
  staged changes. Without `-m` the reflog entry has an empty message.
- `reset` saves the old tip in `ORIG_HEAD` and logs `reset: moving to <rev>`
  ([RESET.C L193](https://github.com/git/git/blob/v2.34.0/builtin/reset.c#L193),
  [RESET.C L451](https://github.com/git/git/blob/v2.34.0/builtin/reset.c#L451)). Every mode
  also removes `MERGE_HEAD`, `CHERRY_PICK_HEAD` and friends
  ([RESET.C L456-L457](https://github.com/git/git/blob/v2.34.0/builtin/reset.c#L456-L457)).
- The modes are documented in [RESET L56-L91](https://github.com/git/git/blob/v2.34.0/Documentation/git-reset.txt#L56-L91),
  with the full state tables in *DISCUSSION* ([RESET L378-L500](https://github.com/git/git/blob/v2.34.0/Documentation/git-reset.txt#L378-L500)).
  `--hard`: "Any untracked files or directories in the way of writing any tracked files are simply
  deleted." `--keep`: "If a file that is different between `<commit>` and `HEAD` has local changes,
  reset is aborted."

## 2. Refusals

### `branch -f` and checked-out branches

| Case | git 2.34.1 | git 2.43.0 |
|---|---|---|
| Open worktree's branch | refused: `Cannot force update the current branch.` | refused: `cannot force update the branch '<b>' used by worktree at '<path>'` |
| Checked out in another worktree | **moved**; that worktree now shows the reverse diff as staged | refused, same message |
| Being rebased (any worktree) | **moved**; `rebase --abort` later puts it back where the rebase started, and `--continue` fails at the end (`cannot lock ref 'refs/heads/<b>': is at … but expected …`), leaving the rebase stuck | refused |
| Being bisected from (any worktree) | **moved** | refused |
| Checked out nowhere | moved | moved |

**2.34 only.** git 2.34's check compares the name with the open worktree's `HEAD` and nothing else
([BRANCH.C v2.34.0 L200-L216](https://github.com/git/git/blob/v2.34.0/branch.c#L200-L216)); a branch
mid-rebase in the open worktree passes too, because `HEAD` is detached then. Commit
[593a2a5d0](https://github.com/git/git/commit/593a2a5d0639b4b4f91ff6e6ffb64e72020f8fd8) (merged as
`ak/protect-any-current-branch`, first in v2.35.0) switched to `find_shared_symref()` over all
worktrees ([BRANCH.C v2.35.0 L311-L315](https://github.com/git/git/blob/v2.35.0/branch.c#L311-L315)),
which also counts branches being rebased or bisected
([worktree.c v2.34.0 L405-L435](https://github.com/git/git/blob/v2.34.0/worktree.c#L405-L435)).
Git 2.42 reworded the message from "checked out at" to "used by worktree at"
([REL 2.42.0 L36-L39](https://github.com/git/git/blob/v2.42.0/Documentation/RelNotes/2.42.0.txt#L36-L39)).

`switch -C`/`checkout -B` moved and checked out a branch held by another worktree on both 2.34.1
and 2.43.0; git 2.44 made that a refusal
([REL 2.44.0 L6-L8, L29-L34](https://github.com/git/git/blob/v2.44.0/Documentation/RelNotes/2.44.0.txt#L6-L34)).
`update-ref` never refuses.

**Consequence:** parterre can't rely on git's guard on 2.34. It must not offer moving a branch that
`Catalog::occupied` lists (checked out, being rebased or bisected anywhere), exactly as for delete.

### `reset --keep` and `--merge` refusals

From S2 (scenarios below), moving from `B` to its parent `A`, where `B` changed `touched` and added
`addedlater`, and `same` is unchanged between them. "kept" means the content survived.

| Uncommitted state | `--hard` | `--keep` | `--merge` | `--soft`/`--mixed` |
|---|---|---|---|---|
| unstaged change, file not in the diff | **lost** | kept | kept | kept |
| staged change, file not in the diff | **lost** | kept (unstaged) | **lost** | kept |
| unstaged change, file in the diff | **lost** | refused | refused | kept |
| staged change, file in the diff | **lost** | refused | **lost** | kept |
| staged new file | **deleted** | kept (untracked) | **deleted** | kept |
| untracked file, not in either commit | kept | kept | kept | kept |
| untracked file at a path the target tracks | **overwritten** | refused | refused | kept |
| untracked folder where the target has a file (S5) | **deleted** | refused | — | — |
| change to a file the target doesn't have | **lost** | refused | refused | kept |

Refusals print `error: Entry '<path>' not uptodate. Cannot merge.`, `… would be overwritten by
merge`, `Untracked working tree file '<path>' would be overwritten by merge` or `Updating '<dir>'
would lose untracked files in it`, then `fatal: Could not reset index file to revision '<c>'`, exit
128, and change nothing. Only the first conflicting path is named, so a warning that lists them
must be worked out by parterre (as for *Switch* in #143).

So **`--keep` is git's safe form** in the sense of the map: it never loses content (only the
"staged" mark), and refuses otherwise. `--merge` is not: it drops staged changes without a word.
It exists to back out of a merge in a dirty tree ([RESET L235-L255](https://github.com/git/git/blob/v2.34.0/Documentation/git-reset.txt#L235-L255)),
which is what *Abort* is for.

### With an operation in progress (S3)

| In the open worktree | `--soft`, `--keep` | `--mixed`, `--hard`, `--merge` |
|---|---|---|
| merge stopped on a conflict | refused: `Cannot do a <mode> reset in the middle of a merge.` | run; `MERGE_HEAD` removed, merge gone |
| cherry-pick of several commits stopped | refused (unmerged index) | run; `CHERRY_PICK_HEAD` removed but `.git/sequencer` stays, so status still says a cherry-pick is in progress |
| rebase stopped | refused | run, but move the **detached HEAD**, not the branch; `rebase --abort` restores the branch |
| bisect | — | moves the detached HEAD; `bisect reset` returns to the branch unchanged |

The refusal is `die_if_unmerged_cache()`, applied to `--soft` and `--keep` only
([RESET.C L198-L203, L400-L404](https://github.com/git/git/blob/v2.34.0/builtin/reset.c#L198-L404)).
It triggers on `MERGE_HEAD` or unmerged entries, so a stopped-but-clean cherry-pick or rebase
would not be refused. Parterre should not offer the move while the open worktree has an
operation in progress, and point to *Abort*.

## 3. Lost work

### Commits

The commits lost by moving `<b>` from `<old>` to `<new>` are those reachable from `<old>`, not
from `<new>`, and not from any other branch, remote-tracking branch, tag, or another worktree's
HEAD. With git alone:

```
git rev-list <old> ^<new> --not --exclude=<b> --branches --remotes --tags <HEAD of each other worktree>
```

`--exclude` takes the name without `refs/heads/` when it is followed by `--branches`, and is
cleared by it ([REVLIST L170-L185](https://github.com/git/git/blob/v2.34.0/Documentation/rev-list-options.txt#L170-L185)),
so `origin/<b>` still counts. Branch names can't contain glob characters, so the pattern matches
only `<b>`. When moving the open worktree's branch with `reset`, the open worktree's HEAD is `<b>`
and moves with it, so it is left out.

Parterre already has this: `lost_commits` in `crates/parterre-core/src/branches.rs` takes
`excluded_ref`, `leaving` and `future_root`, which map to `refs/heads/<b>`, the open worktree (for
`reset`) and `<new>`. Verified cases (S4):

- In sync with `origin/<b>`, moved back one: nothing lost (`origin/<b>` reaches it).
- Diverged, moved to `@{u}`: the local-only commits are listed.
- A commit also reached by a tag, or by another worktree's detached HEAD: not listed.

The reflog still holds lost commits (90 days by default for commits once on the branch), which
the definition of **lost work** deliberately ignores.

Moving forward (to a descendant of `<old>`) never loses commits. For the open worktree that is a
fast-forward, and for a branch checked out nowhere it is the map's *Fast-forward to upstream*.

### Uncommitted changes

Only `--hard` loses any (table in section 2). Before running it, parterre can list:

1. `git diff --name-only HEAD`: every tracked change, staged or not, including staged new files.
2. Each untracked, non-ignored file (`git ls-files -o --exclude-standard`) whose path, or a
   leading folder of it, exists in `<new>`. One `git cat-file --batch-check` call with lines
   `<new>:<path>` and `<new>:<each leading folder>` answers it: anything but `missing` for the path,
   or a non-`tree` for a folder, means `--hard` overwrites or deletes it (S6).

Ignored files never count, per #143.

## 4. Upstreams and the follow-up push (S4)

"Make my branch match the remote" is `git reset --keep @{u}` for the open worktree, or
`git branch -f <b> <remote>/<b>` for a branch checked out nowhere. Afterwards `status` shows the
branch in sync: nothing to push. The lost commits are the branch's local-only ones; lazygit
computes "no local-only commits" against the upstream *and its reflog*
([LG `branch.go` L381-L406](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/commands/git_commands/branch.go#L381-L406)),
whereas parterre's definition only looks at current refs.

Moving a pushed branch **backwards or sideways**:

| Case | Plain `push` | `--force-with-lease=refs/heads/<b> --force-if-includes` | plain `--force-with-lease` |
|---|---|---|---|
| Nobody pushed since | rejected, non-fast-forward | **accepted** (`forced update`) | accepted |
| Someone pushed after, and a fetch brought it in | rejected | rejected: `remote ref updated since checkout` | **accepted, drops their commit** |
| Someone pushed after, not fetched | rejected | rejected: `stale info` | rejected: `stale info` |
| Branch moved with `branch -f` instead of `reset` | rejected | accepted | accepted |

`--force-if-includes` accepts because the old remote tip is still an entry in
`refs/heads/<b>`'s reflog: the branch held it before the move
([PUSH L352-L366](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L352-L366);
git 2.30+, [REL 2.30.0 L26-L28](https://github.com/git/git/blob/v2.30.0/Documentation/RelNotes/2.30.0.txt#L26-L28)).
`branch -f` and `reset` both write that reflog. So the guard from #140 needs nothing new for this
case. The upstream-with-a-different-name caveat from #140 applies unchanged.

One trap: after moving back, `git status` says the branch is **behind** and "can be
fast-forwarded" (`0|1` in parterre's marker). A plain *Pull* would bring the dropped commits
straight back. Nothing in the refs tells "I moved back on purpose" from "someone pushed"; only the
reflog does, which is what `--force-if-includes` reads.

## 5. Other clients

**TortoiseGit.** The log's context menu has *Reset "<current branch>" to this...* on every commit
except stashes, when there is a working tree
([TG `GitLogListBase.cpp` L1975-L1979](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/GitLogListBase.cpp#L1975-L1979)).
The dialog resets the **current branch only**, with *Soft*, *Mixed* (default) and *Hard: Reset
working tree and index (discard all local changes)*
([TG `TortoiseProcENG.rc` L1470-L1495](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L1470-L1495),
[TG `ResetDlg.cpp` L36, L80-L86](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/ResetDlg.cpp#L36-L86));
in a bare repository only *Soft*. It runs `git reset <mode> --end-of-options <rev> --`
([TG `AppUtils.cpp` L1544-L1593](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1544-L1593)),
with no check of its own; the manual warns that hard reset doesn't use the recycle bin ([TGDOC](https://tortoisegit.org/docs/tortoisegit/tgit-dug-reset.html)).
`--merge` appears only in *Abort Merge*; `--keep` nowhere. After a failed pull and after a fetch,
the progress dialog offers *Reset*, opening the dialog on the upstream with *Hard* selected
([TG `AppUtils.cpp` L2375-L2382, L2630-L2638](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L2375-L2638)).
A branch that isn't checked out can be moved only through *Create Branch* with *Force*
(`git branch -f`, [TG `AppUtils.cpp` L1140-L1141, L1181](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1140-L1181))
or *Switch/Checkout* with *Override branch if exists* (`checkout -B`,
[TG `AppUtils.cpp` L1302-L1303](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1302-L1303)).

**lazygit.** The commits, branches, tags and remote-branches panels have a reset menu (`g`) with
*Mixed*, *Soft*, *Hard* on the current branch, confirming only *Hard* with a dirty tree
([LG `refs_helper.go` L260-L302](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/gui/controllers/helpers/refs_helper.go#L260-L302)).
Its branch auto-forwarding moves branches that aren't checked out with
`git update-ref --stdin -m "lazygit: update to upstream branch"` and lines
`update <ref> <new> <old>`, and checked-out ones with `git reset --keep` in their worktree, after
refusing if the branch has local-only commits or the worktree has tracked changes
([LG `branches_helper.go` L531-L615](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/gui/controllers/helpers/branches_helper.go#L531-L615),
[LG `branch.go` L443-L452](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/commands/git_commands/branch.go#L443-L452),
[LG `working_tree.go` L527-L538](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/commands/git_commands/working_tree.go#L527-L538)).

**GitKraken Desktop.** Right-click a commit or branch for *Reset <branch> to this commit* with
*Soft*, *Mixed*, *Hard*, or drag a branch onto another commit ([GK](https://help.gitkraken.com/gitkraken-desktop/commits/)).
The docs don't say whether a branch that isn't checked out can be reset.

**Fork.** *Reset Current Branch to Here* (current branch only); a later note fixes the "reset
branch to here" window ([FORK](https://git-fork.com/releasenotes)).

**Sourcetree.** *Reset current branch to this commit* with *Soft*, *Mixed*, *Hard*;
check out the branch first ([ST](https://support.atlassian.com/sourcetree/kb/reset-branch-to-a-commit/)).

**VS Code.** No reset to an arbitrary commit. *Undo Last Commit* runs `git reset --soft HEAD~`
and puts the message back in the input box
([VSC `commands.ts` L2787-L2814](https://github.com/microsoft/vscode/blob/0fe38943a280609411483eb73e0ebb45d135d6b9/extensions/git/src/commands.ts#L2787-L2814),
[VSC `git.ts` L2322](https://github.com/microsoft/vscode/blob/0fe38943a280609411483eb73e0ebb45d135d6b9/extensions/git/src/git.ts#L2322)).

Nobody but lazygit's auto-forward uses `--keep`, and nobody checks for lost commits before a reset.

## 6. Open questions for the human

1. **Modes.** Offer only "move the branch" (`reset --keep`, then `--hard` behind a warning), or
   also *Soft*/*Mixed* ("move the branch, keep the old content as changes")? Every other client
   offers soft/mixed/hard; none offers `--keep`.
2. **Branches checked out nowhere: `branch -f` or `update-ref`?** `branch -f <b> <hash>` gives
   git's own reflog text (`branch: Reset to <hash>`). `update-ref -m "<msg>" refs/heads/<b> <new> <old>`
   (lazygit's choice) also refuses if the branch moved since parterre listed the lost commits.
   Either way parterre checks "in use anywhere" itself on 2.34.
3. **Another worktree's branch:** not offered (only *Go to worktree*), as for merge and rebase?
4. **Levels.** Nothing lost and `--keep` succeeds: one click, or a confirmation? Commits lost:
   warning. `--keep` refuses: warning listing the changes, then `--hard`.
5. **Upstream:** a named *Reset to `<remote>/<b>`* item for a diverged branch, next to
   *Fast-forward to upstream*, in #171?
6. **The follow-up push:** after moving a pushed branch back, it shows as behind, and *Pull* would
   undo the move. Should parterre offer the lease-guarded force push for a branch that is only
   behind, for example when `<remote>/<b>` is in the branch's reflog (what `--force-if-includes`
   checks)?

## Scenarios

All run on git 2.34.1 and 2.43.0, in fresh repos with `GIT_CONFIG_NOSYSTEM=1` and an empty `HOME`.

- **S1 — commands and checked-out refusals.** `main` A–B–C and `other`. Each command from
  section 1 on `other` (not checked out) and on `main` (open worktree), recording refs,
  `ORIG_HEAD`, `status` and reflog messages. Then `other` checked out in a second worktree, and
  `feat` stopped mid-rebase (then `--abort` or `--continue`) and mid-bisect in a second worktree, against `branch -f`, `switch -C`,
  `checkout -B` and `update-ref`. Results in section 2's first table.
- **S2 — uncommitted changes.** Commits A, then B (changes `touched`, adds `addedlater`); reset
  from B to A in each mode with each of eight kinds of uncommitted state. Results in section 2's
  second table.
- **S3 — operations in progress.** `main` and `side` both change `f`. A conflicted merge, a
  two-commit cherry-pick stopped on the first, and a stopped rebase, then each reset mode;
  separately a bisect. Results in section 2's third table.
- **S4 — lost commits and pushing.** A bare remote, my clone and a colleague's. `feat` pushed;
  moved back with `reset --keep`; pushed plain, with the lease and `--force-if-includes`, and with
  the lease only (dry run). Then the colleague pushes, with and without my fetching in between.
  Diverged `feat` reset to `@{u}`. A commit kept by a tag and by another worktree's detached HEAD.
  `branch -f` on a branch checked out nowhere, then the guarded push.
- **S5 — folder in the way.** An untracked `dir/x` where the target has a file `dir`: `--hard`
  deletes `dir/x` silently, `--keep` refuses (`Updating 'dir' would lose untracked files in it`).
- **S6 — listing what `--hard` overwrites.** Untracked `dir/x`, `sub`, `newf`, `keepme` against a
  target with the file `dir`, the folder `sub` and the file `newf`: `cat-file --batch-check` flags
  the first three, and `--hard` indeed replaced exactly those.
