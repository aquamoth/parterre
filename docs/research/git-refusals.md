# git's refusals and operations in progress

Research note for [#141](https://github.com/aquamoth/parterre/issues/141), part of the map
[Changing the repository from parterre](https://github.com/aquamoth/parterre/issues/137). The
question: *how does git refuse, and how can parterre recognise each refusal reliably: by exit
code, by stderr under `LC_ALL=C`, or by asking git beforehand?* It also covers operations in
progress (merge, rebase, cherry-pick, revert) and what two checks cost on a large repository.

Sources are git's documentation and source, pinned below, and experiments I ran. A statement
checked by running a command is marked **(tested)**. A statement that is my own conclusion is
marked **(derived)**. A statement I could not check is marked **(unverified)**.

All tests ran on 2026-09-29 on Windows 11, in throwaway repositories, with two gits:

- **Git for Windows 2.53.0.windows.3** (the installed one).
- **Git for Windows 2.34.1.windows.1** (PortableGit, the hard minimum; Ubuntu 22.04 ships 2.34.1).

Every command ran with `LC_ALL=C`, `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null` (no
user config), and stdin not a terminal, as parterre runs git. Where the two versions differ,
the text says so. Where it says nothing, the output was identical apart from paths and ids.

## TL;DR

1. **Never match stderr text.** It changed between 2.34 and 2.53 for many of the refusals here
   (§1.2): `already checked out at` became `already used by worktree at` (2.43),
   `The branch 'x' is not fully merged.` became `the branch 'x' is not fully merged` plus
   hints (2.44), `No rebase in progress?` became `no rebase in progress` (2.45). Some lines even
   moved between stdout and stderr. Show git's stderr to the user as is, and decide with the
   **exit code plus a check asked of git beforehand**.
2. **Exit codes are coarse.** `die()` gives 128, most `error()` paths give 1. `merge` refusing
   because of local changes gives **2** since 2.38 but **128** in 2.34. A stop on conflicts
   gives **1** for merge, rebase, cherry-pick and revert. A failed `worktree remove` that got
   half way gives **255**. So an exit code only says "refused" versus "stopped with conflicts"
   when parterre already knows which refusals were possible.
3. **Ask beforehand.** Nearly every refusal has a cheap pre-check (§2 to §6):
   - branch in use: `worktree list --porcelain`, **plus** `rebase-merge/head-name` /
     `rebase-apply/head-name` of each worktree, because a branch being rebased is listed as
     `detached` but git still refuses to take it.
   - not fully merged: `merge-base --is-ancestor <branch> <upstream, else HEAD>`.
   - dirty worktree: `status --porcelain --ignore-submodules=none`, the exact command
     `worktree remove` runs itself.
   - operation in progress: the state files in §7.
   The ones that can't be pre-checked are the server's (protected branches) and switch's
   "would be overwritten", which is cheap to just try: the refusal changes nothing.
4. **`push --porcelain`** gives one tab-separated line per ref on stdout
   (`!\t:refs/heads/x\t[remote rejected] (reason)`), so a partly failed delete is readable.
   Exit code is 1 if any ref failed, even when others were deleted. A plain git server refuses
   to delete the branch its `HEAD` points to (bare or not); GitHub refuses the default branch
   and protected branches. Only the default branch can be foreseen (`refs/remotes/<r>/HEAD`).
5. **`worktree remove`** refuses dirty or untracked files, submodules (even clean ones), locks
   (`-f -f` overrides) and the main worktree. **It deletes ignored files silently**, including
   an ignored nested repository. With `--force` it also deletes nested repositories and
   submodules together with their git dirs. A clean **detached** worktree is removed without a
   word even if its HEAD has commits that are on no branch. **On Windows**, a file held open in
   the folder makes it fail half way (exit 255): part of the folder stays, and the worktree's
   metadata is deleted anyway.
6. **Operations in progress** are per worktree: the files live in the worktree's own git dir,
   `<common>/worktrees/<id>/` for a linked worktree. `MERGE_HEAD`, `rebase-merge/`,
   `rebase-apply/`, `CHERRY_PICK_HEAD`, `REVERT_HEAD` and `sequencer/` (for ranges) are what git's
   own `wt_status_get_state` looks at. In a **reftable** repository `CHERRY_PICK_HEAD` and
   `REVERT_HEAD` are not files but refs, so ask git (`cat-file --batch-check` with
   `worktrees/<id>/CHERRY_PICK_HEAD`); `MERGE_HEAD` stays a file.
7. **`--continue` opens an editor** for merge and rebase (tested, both versions). Parterre must
   set `GIT_EDITOR=true` in the environment for continue: `-c core.editor=true` loses to a
   `GIT_EDITOR` the user has set. If the editor fails, the operation stays in progress.
8. **Conflicts are counted** from the index: `git ls-files -u -z` (three stages per path; count
   unique paths) reads only the index. Conflicts can exist with no operation in progress
   (`merge --squash`, `cherry-pick -n`, `merge --quit`).
9. **`watch.rs`** would add, per worktree git dir: `MERGE_HEAD`, `CHERRY_PICK_HEAD`,
   `REVERT_HEAD`, and the directories `rebase-merge`, `rebase-apply`, `sequencer` (their own
   metadata only), plus `index` while an operation is in progress, for the conflict count.
10. **Cost** (§8): see the numbers there. **`GIT_OPTIONAL_LOCKS=0` makes a stale index very
    expensive**: on 100,000 files, `status --porcelain` took 0.5 s normally but 4.7 to 7.2 s
    after every file's mtime changed, on every run, because it never writes the refreshed index
    back. "Is this commit on any other branch" costs about 0.25–0.4 s with
    `rev-list <c> --not --exclude=<its branch> --branches --remotes` on a clone of git.git with
    4,000 extra branches, in both versions, with or without a commit-graph. `--contains`
    (`branch`/`for-each-ref`) took 16–29 s in 2.53 and 60–84 s in 2.34 there without a
    commit-graph, which a fresh clone doesn't have.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT | git `v2.56.0` @ `a0189536` (2026-09-28) | https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/ |
| GIT234 | git `v2.34.1` | https://github.com/git/git/blob/v2.34.1/ |
| GHDOCS | GitHub docs: [About protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches), [Creating and deleting branches](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/proposing-changes-to-your-work-with-pull-requests/creating-and-deleting-branches-within-your-repository) (read 2026-09-29) | |
| PRIOR | [`docs/research/git-worktrees.md`](https://github.com/aquamoth/parterre/blob/research/git-worktrees/docs/research/git-worktrees.md) on `research/git-worktrees` | |

Which version changed a message was found by reading the file at consecutive release tags.

---

## 1. General rules

### 1.1 Exit codes

| Code | Where it comes from | Examples here |
|---|---|---|
| 0 | Success. Also some *warnings*: `branch -d` deleting a branch merged to its upstream but not to HEAD | |
| 1 | `error()` then return 1; a stop on conflicts | switch "would be overwritten", `branch -d` (all refusals), `push` (any ref failed), merge/rebase/cherry-pick/revert conflicts, `rebase` with unstaged changes |
| 2 | `merge` refusing before it starts (since 2.38) | merge "would be overwritten" |
| 128 | `die()` | switch to a branch in use, switch during an operation, all `worktree add`/`remove` refusals, cherry-pick/revert "would be overwritten", `--abort` with nothing to abort, merge in 2.34 |
| 255 | A command that died in a child, or a partial failure | `worktree add -b <existing>`, `worktree remove` that failed to delete files |

**(derived)** The only split that is reliable without context is 0 versus non-zero. Parterre
knows which command it ran and has already checked what it can, so a non-zero code then
means "git refused for a reason we couldn't foresee: show stderr".

### 1.2 Wording differs between 2.34 and 2.53

All **(tested)**. The version where 2.53's text first appears is from GIT at release tags.

| Refusal | 2.34.1 | 2.53 | Changed in |
|---|---|---|---|
| Branch checked out elsewhere (`switch`, `checkout`, `worktree add`) | `fatal: 'feat' is already checked out at '<path>'` | `fatal: 'feat' is already used by worktree at '<path>'` | 2.43.0 |
| `branch -d`/`-D` of a checked-out branch | `error: Cannot delete branch 'feat' checked out at '<path>'` | `error: cannot delete branch 'feat' used by worktree at '<path>'` | 2.43.0 |
| `branch -d` not merged | `error: The branch 'topic' is not fully merged.` / `If you are sure you want to delete it, run 'git branch -D topic'.` | `error: the branch 'topic' is not fully merged` / `hint: If you are sure …` / `hint: Disable this message …` | 2.44.0 |
| `branch -d` missing | `error: branch 'nosuch' not found.` | `error: branch 'nosuch' not found` | |
| `rebase --abort` with none | `fatal: No rebase in progress?` | `fatal: no rebase in progress` | 2.45.0 |
| merge "would be overwritten" | exit **128**, no last line | exit **2**, ends with `Merge with strategy ort failed.` | 2.38.0 |
| `worktree add -b` existing | `fatal: A branch named 'feat' already exists.` | `fatal: a branch named 'feat' already exists` | |
| `Preparing worktree (…)` | stdout | stderr | |
| `worktree prune -v` report | stdout | stderr | |
| Rebase progress | `Rebasing (2/2)\x1b[K…` (an ANSI erase even when not a terminal) | `Rebasing (2/2)…` | |

Rebase progress also uses `\r` between steps. **(derived)** The operation dialog's live
output must handle `\r` and strip `ESC [ K`.

---

## 2. `git switch`

Setup for all rows: `main` checked out in the main worktree, `feat` in a linked worktree,
`other` changes `f.txt` and adds `h.txt`. All **(tested)**, both versions unless noted.

| Case | Exit | stderr (2.53, `LC_ALL=C`) |
|---|---|---|
| Local change to a file the target changes | 1 | `error: Your local changes to the following files would be overwritten by checkout:` / `\tf.txt` / `Please commit your changes or stash them before you switch branches.` / `Aborting` |
| Untracked file the target tracks | 1 | `error: The following untracked working tree files would be overwritten by checkout:` / `\th.txt` / `Please move or remove them before you switch branches.` / `Aborting` |
| Local change to a file the target doesn't change | **0** | `Switched to branch 'other'`; stdout `M\tg.txt`. The change is carried along |
| Branch checked out in another worktree | 128 | `fatal: 'feat' is already used by worktree at '<path>'` (2.34: `is already checked out at`) |
| Branch being **rebased** in another worktree (its record says `detached`) | 128 | Same message |
| This worktree has a merge in progress | 128 | `fatal: cannot switch branch while merging` / `Consider "git merge --quit" or "git worktree add".` (also `while rebasing`, `while cherry-picking`, `while reverting`, [checkout.c#L1624-L1655](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/checkout.c#L1624-L1655)) |
| Unknown branch | 128 | `fatal: invalid reference: nosuch` |

- **In use elsewhere.** `die_if_checked_out` refuses if `is_shared_symref` matches any other
  worktree ([branch.c#L881-L896](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/branch.c#L881-L896)).
  That counts a worktree whose HEAD names the branch, **and a detached worktree that is
  rebasing or bisecting that branch**
  ([worktree.c#L500-L525](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L500-L525),
  [#L462-L477](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/worktree.c#L462-L477)).
  The rebased branch is read from `rebase-merge/head-name` or `rebase-apply/head-name`
  ([wt-status.c#L1847-L1875](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/wt-status.c#L1847-L1875)).
  `--ignore-other-worktrees` overrides it
  ([git-switch.adoc#L181-L185](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-switch.adoc#L181-L185)).
- **Detect beforehand:** the `branch refs/heads/<b>` lines of `worktree list --porcelain`
  (parterre already runs it), plus, for every `detached` record, the first line of
  `<its git dir>/rebase-merge/head-name` or `rebase-apply/head-name` if present. Bisect is the
  same idea with `BISECT_START`; it is out of scope for the map. And the operation-in-progress
  check of §7 for the worktree being switched.
- **"Would be overwritten":** git checks it atomically and changes nothing when it refuses
  (tested: status unchanged). A dry run exists: `git read-tree -m -u -n HEAD <target>` exited
  128 with `error: Entry 'f.txt' not uptodate. Cannot merge.` or `error: Untracked working tree
  file 'h.txt' would be overwritten by merge.`, and 0 when switch would succeed (tested). But it
  **takes `index.lock`** even with `-n`
  ([read-tree.c#L191](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/read-tree.c#L191))
  and is a second process for no gain. **(derived)** Run `git switch <b>` directly. Exit 1 after
  the pre-checks above means "local changes in the way": then show `status --porcelain` and
  offer `switch --discard-changes` (git-switch.adoc#L117) in a warning.

## 3. `git branch -d`

| Case | Exit | stderr (2.53) |
|---|---|---|
| Not merged into HEAD, no upstream | 1 | `error: the branch 'topic' is not fully merged` / `hint: If you are sure you want to delete it, run 'git branch -D topic'` / `hint: Disable this message with "git config set advice.forceDeleteBranch false"` |
| Has an upstream it isn't merged into, but merged into HEAD | 1 | `warning: not deleting branch 'up1' that is not yet merged to` / `         'refs/remotes/origin/main', even though it is merged to HEAD` then the same error |
| Merged into its upstream, not into HEAD | **0** | `warning: deleting branch 'up2' that has been merged to` / `         'refs/remotes/origin/up2work', but not yet merged to HEAD`; stdout `Deleted branch up2 (was 696be9c).` |
| Checked out in a linked worktree (also with `-D`) | 1 | `error: cannot delete branch 'feat' used by worktree at '<path>'` |
| The current branch | 1 | `error: cannot delete branch 'main' used by worktree at '<path>'` |
| Being rebased in another worktree (also `-D`) | 1 | Same, naming that worktree |
| Doesn't exist | 1 | `error: branch 'nosuch' not found` |

All **(tested)**, both versions (wording per §1.2).

- **What "merged" means.** The docs: "The branch must be fully merged in its upstream branch,
  or in `HEAD` if no upstream was set"
  ([git-branch.adoc#L103-L105](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-branch.adoc#L103-L105)).
  The source is exact: if the branch has an upstream **and the upstream ref resolves**, it is
  the reference; otherwise HEAD. Merged means the branch tip is an ancestor of the reference
  (`repo_in_merge_bases`). When upstream and HEAD disagree, the warnings above are printed
  ([branch.c#L135-L196](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/branch.c#L135-L196)).
  2.34 has the same logic
  ([GIT234 builtin/branch.c#L113-L164](https://github.com/git/git/blob/v2.34.1/builtin/branch.c#L113-L164)).
- **(derived)** Two consequences:
  - "HEAD" is the HEAD **of the worktree git runs in**. Without an upstream, `branch -d` from
    the main worktree and from a linked one can give different answers. Parterre should run it
    in the open worktree and compute the pre-check against the same HEAD.
  - An upstream whose remote branch was deleted (`[gone]`) doesn't resolve, so git falls back
    to HEAD.
- **Detect beforehand:** `git merge-base --is-ancestor <branch> <reference>`: exit 0 if merged,
  1 if not, anything else is an error
  ([git-merge-base.adoc#L55-L58](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-merge-base.adoc#L55-L58)).
  The reference is `%(upstream)` from `for-each-ref` if that ref exists, else `HEAD`.
  "Checked out" uses the same check as `switch` (§2): `branch_checked_out` covers worktree
  HEADs, rebases and bisects ([branch.c#L285-L300](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/branch.c#L285-L300)).

## 4. `git push <remote> --delete <branch>`

Tested against a local bare repository and a local non-bare clone.

| Case | Exit | What git prints (2.53 and 2.34 alike) |
|---|---|---|
| The remote's `HEAD` branch, bare remote | 1 | `remote: error: By default, deleting the current branch is denied, …` (eight lines) / `remote: error: refusing to delete the current branch: refs/heads/main` / `To ../bare.git` / ` ! [remote rejected] main (deletion of the current branch prohibited)` / `error: failed to push some refs to '../bare.git'` |
| Remote has `receive.denyDeletes=true` | 1 | `remote: error: denying ref deletion for refs/heads/x` / ` ! [remote rejected] x (deletion prohibited)` |
| A `pre-receive` hook refuses (a stand-in for a server's branch protection) | 1 | `remote: <hook's stderr>` / ` ! [remote rejected] y (pre-receive hook declined)` |
| An `update` hook refuses one of two refs | 1 | ` - [deleted]         w` / ` ! [remote rejected] z (hook declined)`: **the other ref is deleted** |
| Branch checked out in a non-bare remote | 1 | `remote: error: refusing to update checked out branch: refs/heads/work` (+ twelve lines) / ` ! [remote rejected] work (branch is currently checked out)` |
| No such branch on the remote | 1 | `error: unable to delete 'nosuch': remote ref does not exist` / `error: failed to push some refs to '../bare.git'` |

- **`--porcelain`** puts one line per ref on stdout, tab separated: `<flag>\t<from>:<to>\t<summary> (<reason>)`,
  then `Done`. The flag is `-` for deleted and `!` for rejected
  ([git-push.adoc#L165-L168](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-push.adoc#L165-L168),
  [#L505-L560](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-push.adoc#L505-L560)).
  Tested: `-\t:refs/heads/x\t[deleted]` and `!\t:refs/heads/z\t[remote rejected] (hook declined)`.
  The `remote:` lines and `error: failed to push some refs` still go to stderr. For "remote ref
  does not exist" there is **no** porcelain line at all, only stderr and exit 1.
- **The docs understate `denyDeleteCurrent`.** They say it applies to "the currently checked
  out branch of a non-bare repository"
  ([config/receive.adoc#L81-L83](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/receive.adoc#L81-L83)),
  but the code compares with the repository's `HEAD` whether bare or not
  ([receive-pack.c#L1534-L1560](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/receive-pack.c#L1534-L1560)),
  and a bare remote refused (tested).
- **GitHub.** "By default, you cannot delete a protected branch"; the push error quoted in the
  docs is `remote: error: GH006: Protected branch update failed for refs/heads/main.` (GHDOCS,
  protected branches). "If the branch you want to delete is the repository's default branch,
  you must choose a new default branch before deleting the branch" (GHDOCS, creating and
  deleting). Users report the default-branch refusal as `! [remote rejected] main (refusing to
  delete the current branch: refs/heads/main)`
  ([community discussion #21597](https://github.com/orgs/community/discussions/21597))
  **(unverified)**: I did not push to GitHub.
- **Detect beforehand:**
  - The remote's default branch: `refs/remotes/<remote>/HEAD`, which `clone` sets and
    `git remote set-head <remote> -a` refreshes (tested; a repository that only did
    `remote add` + `fetch` had none). It is local and may be stale. `git ls-remote --symref
    <remote> HEAD` is exact but costs a network round trip (tested:
    `ref: refs/heads/main\tHEAD`).
  - Protection lives only on the server. git has no way to ask. **(derived)** Parterre can't
    pre-check it with git; it should run the delete and report the porcelain `!` line's reason.
    (GitHub's REST API has a `protected` flag per branch; that is outside git and would need
    the same login the pull-request feature uses.)

## 5. `git worktree remove`

| Case | Exit | stderr (2.53 and 2.34 alike) |
|---|---|---|
| Modified tracked file | 128 | `fatal: '../wt-dirty' contains modified or untracked files, use --force to delete it` |
| Untracked file | 128 | Same |
| Only **ignored** files (including an ignored nested repository) | **0** | None. **They are deleted** |
| Untracked nested repository | 128 | Same as modified |
| Contains a submodule, even a clean one | 128 | `fatal: working trees containing submodules cannot be moved or removed` |
| Locked, with a reason | 128 | `fatal: cannot remove a locked working tree, lock reason: on usb` / `use 'remove -f -f' to override or unlock first` |
| Locked, no reason | 128 | `fatal: cannot remove a locked working tree;` / `use 'remove -f -f' to override or unlock first` |
| Locked, with one `-f` | 128 | Same as locked |
| Main worktree | 128 | `fatal: '.' is a main working tree` (the argument as given) |
| Not a worktree | 128 | `fatal: '../nosuch' is not a working tree` |
| Folder already gone | **0** | None. Removes `worktrees/<id>/` |
| Folder gone and locked | 128 | `fatal: cannot remove a locked working tree;` … (`-f -f` then works) |
| **Windows:** a file in the folder held open by another process | **255** | `error: failed to delete '<path>': Invalid argument` |
| **Windows:** removing the worktree git runs in (`-C wt worktree remove .`) | **255** | `error: failed to delete '<path>': Permission denied` |

All **(tested)**; the Windows rows only on 2.53.

How it works ([builtin/worktree.c#L1425-L1475](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1425-L1475),
identical in [GIT234 #L952-L1004](https://github.com/git/git/blob/v2.34.1/builtin/worktree.c#L952-L1004)):

1. Refuse the main worktree; refuse a locked one unless `force >= 2`.
2. If the folder exists and there is no `--force`, `check_clean_worktree`: refuse if any
   submodule is registered (a `worktrees/<id>/modules` directory, or a gitlink in its index),
   then run `git status --porcelain --ignore-submodules=none` in the worktree and refuse if it
   prints anything
   ([#L1372-L1409](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1372-L1409),
   [#L1250](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1250)).
   Ignored files don't show in that status, so they never block removal.
3. `remove_dir_recursively(path, 0)`: flag 0 means nested repositories are **not** kept
   ([#L1411-L1423](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L1411-L1423)).
4. Then, "continue on even if ret is non-zero, there's no going back from here": delete
   `worktrees/<id>/`, and `worktrees/` if it is now empty.

What that means:

- **Deleted without `--force`:** the working files, all ignored files and folders (build output,
  `node_modules`, an ignored nested repository with its own `.git` and unpushed commits: tested,
  gone), and `worktrees/<id>/` (the worktree's HEAD, index, reflog and any in-progress state).
- **Also deleted with `--force`:** modified and untracked files, untracked nested repositories
  (tested) and submodules. A submodule's git dir lives in `worktrees/<id>/modules/<name>`
  (tested: its `.git` file said `gitdir: ../../r/.git/worktrees/wt-sub/modules/sm`), so its
  unpushed commits go too (tested: both folders gone).
- **Not deleted:** branches, including the one the worktree had checked out (tested), and
  other shared refs such as stashes **(derived)**: `refs/` is shared (`common_list`, §7.3).
- **A detached HEAD's commits.** A clean worktree whose detached HEAD had a commit on no branch
  was removed with exit 0 and no warning; the commit was then reachable from nothing (`fsck
  --unreachable --no-reflogs` listed it) and will be lost once gc prunes it (tested, both
  versions). `rev-list <HEAD> --not --branches --remotes --tags` beforehand printed it.
- **Windows, partial failure.** Files that can't be removed stay; the rest go. In the test
  the worktree's `.git` file and the folder `a/` were gone, `z/held.txt` stayed, and
  `worktrees/<id>/` was deleted anyway: the leftover is an ordinary folder that `worktree list`
  no longer shows. With null stdin git did not prompt to retry (tested).

Detect beforehand, all **(derived)** from the code above:

| Refusal | Ask git |
|---|---|
| Main | The first record of `worktree list --porcelain` |
| Locked | `locked` in its record |
| Dirty or untracked | `git -C <wt> status --porcelain --ignore-submodules=none`: any output. Same command as git's own check |
| Submodules | `git -C <wt> ls-files -s` has a mode `160000` line, or `<common>/worktrees/<id>/modules` exists |
| Gone | `prunable` in its record, or stat `<path>/.git` (PRIOR §1.2: locked-and-gone is not `prunable`) |
| What `--force` would delete | The same status output, plus `git -C <wt> status --porcelain --ignored` for what goes even without it |
| Commits only its detached HEAD has | `git rev-list <HEAD> --not --branches --remotes --tags` (non-empty = would be lost; see §8.2 for cost) |

### 5.1 `--force --force` and `prune`

- `-f -f` is needed for a locked worktree: "To add a missing but locked worktree path, specify
  `--force` twice" and the same for `move`; `remove` says so in its message
  ([git-worktree.adoc#L178-L192](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L178-L192)).
- `git worktree prune [-n] [-v]` removes `worktrees/<id>/` for worktrees whose folders are
  missing ([git-worktree.adoc#L132-L140](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L132-L140)).
  Tested: `prune -n -v` printed `Removing worktrees/wt-gone: gitdir file points to
  non-existent location` and exited 0; a locked missing worktree was left alone.
  **(derived)** For one gone worktree, `worktree remove <path>` does the same job (exit 0,
  tested) and doesn't touch the others, so it fits a per-worktree menu better than `prune`.

## 6. `git worktree add`

| Case | Exit | stderr (2.53) |
|---|---|---|
| Branch checked out (or being rebased) elsewhere | 128 | `Preparing worktree (checking out 'feat')` / `fatal: 'feat' is already used by worktree at '<path>'` |
| Folder exists and isn't empty | 128 | `Preparing worktree (checking out 'other')` / `fatal: '../exists' already exists` |
| Folder exists and is empty | 0 | Works |
| Path registered, folder missing | 128 | `fatal: '../wt-gone' is a missing but already registered worktree;` / `use 'add -f' to override, or 'prune' or 'remove' to clear` |
| Same, and locked | 128 | `fatal: '../wt-lm' is a missing but locked worktree;` / `use 'add -f -f' to override, or 'unlock' and 'prune' or 'remove' to clear` |
| `-b <name>` that exists | **255** | `fatal: a branch named 'feat' already exists` |
| **Path inside the main worktree** | **0** | Works. The main worktree's `status --porcelain` then shows `?? inner/` |

All **(tested)** (`Preparing worktree` is on stdout in 2.34). The path rules are in
`check_candidate_path` ([builtin/worktree.c#L314-L340](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L314-L340)).

- **Inside the repository.** git allows it, but the main worktree then has an untracked folder,
  so its status is never clean again, which spoils every "is it clean?" check.
  `git clean -fd -n` in the main worktree skipped it and `clean -ffd -n` would remove it
  (tested: `Would remove inner/`), because a folder with a `.git` file is treated as a nested
  repository. **(derived)** Parterre should refuse or warn on a path inside any worktree, or
  add it to `.git/info/exclude`.
- **Detect beforehand:** branch in use as in §2; folder: `read_dir` is empty or missing; path
  registered: compare with the `worktree <path>` records; branch name taken:
  `git show-ref --verify --quiet refs/heads/<name>` (exit 1 when free).

## 7. Merge, rebase, cherry-pick and revert

### 7.1 Stopping on conflicts

| Command | Exit | Output on conflict (2.53) |
|---|---|---|
| `merge feat` | 1 | stdout: `Auto-merging f.txt` / `CONFLICT (content): Merge conflict in f.txt` / `Automatic merge failed; fix conflicts and then commit the result.` |
| `rebase feat` | 1 | stderr: `Rebasing (1/2)error: could not apply 34fb593... main1` + hints + `Could not apply 34fb593... # main1`; stdout: `Auto-merging …` / `CONFLICT …` |
| `rebase --apply feat` | 1 | stderr: `error: Failed to merge in the changes.` + hints; stdout: `Applying: main1` … `Patch failed at 0001 main1` |
| `cherry-pick <c>` | 1 | stderr: `error: could not apply 13dcb68... feat1` + hints; stdout: `CONFLICT …` |
| `revert <c>` | 1 | stderr: `error: could not revert f59bec8... feat3` + hints; stdout: `CONFLICT …` |

All **(tested)**, same in a linked worktree and in 2.34 (without the `advice.mergeConflict`
hint line). Stops **without** conflicts also exit 1 (rebase `exec` failure, a cherry-pick
that became empty) or 0 (`merge --no-commit`), so the exit code doesn't say whether there are
conflicts: count them (§7.4).

### 7.2 Refusals before starting

| Case | Exit | stderr (2.53) |
|---|---|---|
| `merge` with a local change to a file the merge touches | **2** (2.34: **128**) | `error: Your local changes to the following files would be overwritten by merge:` / `\tf.txt` / `Please commit your changes or stash them before you merge.` / `Aborting` / `Merge with strategy ort failed.` |
| `merge` with an untracked file it would create | 2 (2.34: 128) | `error: The following untracked working tree files would be overwritten by merge:` … |
| `merge` with an unrelated local change | 0 | Allowed |
| `rebase` with **any** unstaged change | 1 | `error: cannot rebase: You have unstaged changes.` / `error: Please commit or stash them.` |
| `cherry-pick` / `revert` with a local change in the way | 128 | `error: Your local changes … would be overwritten by merge:` … / `fatal: cherry-pick failed` (`revert failed`) |
| `merge` during an unresolved merge | 128 | `error: Merging is not possible because you have unmerged files.` + hints / `fatal: Exiting because of an unresolved conflict.` |
| `cherry-pick` during an unresolved cherry-pick | 128 | `error: Cherry-picking is not possible because you have unmerged files.` + hints / `fatal: cherry-pick failed` |
| `rebase` during an unresolved merge | 1 | `error: cannot rebase: You have unstaged changes.` … (it sees the conflict as a change) |
| `merge` during a merge whose conflicts are resolved | 128 | `fatal: You have not concluded your merge (MERGE_HEAD exists).` / `Please, commit your changes before you merge.` |
| `cherry-pick` during a resolved merge | 128 | `error: your local changes would be overwritten by cherry-pick.` / `hint: commit your changes or stash them to proceed.` / `fatal: cherry-pick failed` |
| `rebase` during a rebase | 128 | `fatal: It seems that there is already a rebase-merge directory, and` / `I wonder if you are in the middle of another rebase. …` ([rebase.c#L1456-L1474](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/rebase.c#L1456-L1474)) |
| **`merge` during a rebase** (conflict resolved and staged) | **0** | `Already up to date.`: **not refused** |

All **(tested)**, both versions. The merge exit code is from `ret = 2` in
[merge.c#L1846-L1850](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/merge.c#L1846-L1850).
**(derived)** Pre-check with `status --porcelain`: for rebase, any tracked change blocks it
(`-uno` output non-empty); for the others git decides per file, so just run them. And check
§7.3 first: parterre should never start an operation where one is already in progress,
because git doesn't always refuse (the last row).

### 7.3 Detecting an operation in progress

git's own check is `wt_status_get_state`
([wt-status.c#L1925-L1964](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/wt-status.c#L1925-L1964);
same order in [GIT234 #L1724-L1760](https://github.com/git/git/blob/v2.34.1/wt-status.c#L1724-L1760)):

1. `MERGE_HEAD` exists → merge.
2. Else `rebase-apply/` exists → `am` if `rebase-apply/applying` exists, else rebase;
   else `rebase-merge/` → rebase (interactive if `rebase-merge/interactive` exists)
   ([#L1847-L1875](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/wt-status.c#L1847-L1875)).
3. Else the ref `CHERRY_PICK_HEAD` → cherry-pick.
4. The ref `REVERT_HEAD` → revert.
5. `sequencer/todo` whose first command is `pick` or `revert` → cherry-pick or revert, even
   without the `_HEAD` ref ([sequencer.c#L2886-L2911](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L2886-L2911)).

The files, **(tested)** after each stop, in the main worktree (`<git dir>` = `.git`) and in a
linked one (`<git dir>` = `<common>/worktrees/<id>`); the set was the same in both places,
apart from an `ORIG_HEAD` left over from earlier steps:

| Operation | Files that mark it | Others left beside them | Continue | Skip | Abort | Quit (keep changes, forget state) |
|---|---|---|---|---|---|---|
| merge | `MERGE_HEAD` | `MERGE_MSG`, `MERGE_MODE`, `AUTO_MERGE`, `ORIG_HEAD`, maybe `MERGE_AUTOSTASH` | `merge --continue` (or `commit`) | — | `merge --abort` | `merge --quit` |
| rebase (default "merge" backend) | `rebase-merge/` | `REBASE_HEAD`, `MERGE_MSG`, `AUTO_MERGE`, `ORIG_HEAD`; branch in `rebase-merge/head-name` | `rebase --continue` | `rebase --skip` | `rebase --abort` | `rebase --quit` |
| rebase `--apply` (and `git am`) | `rebase-apply/` (`am`: plus `rebase-apply/applying`) | `REBASE_HEAD`, `ORIG_HEAD`; branch in `rebase-apply/head-name` | same | same | same | same |
| cherry-pick of one commit | `CHERRY_PICK_HEAD` | `MERGE_MSG`, `AUTO_MERGE` | `cherry-pick --continue` | `cherry-pick --skip` | `cherry-pick --abort` | `cherry-pick --quit` |
| cherry-pick of a range | `CHERRY_PICK_HEAD` **and** `sequencer/` (`todo`, `head`, `abort-safety`) | as above | same | same | same | same |
| revert of one commit | `REVERT_HEAD` | `MERGE_MSG`, `AUTO_MERGE`, `ORIG_HEAD` | `revert --continue` | `revert --skip` | `revert --abort` | `revert --quit` |
| revert of a range | `REVERT_HEAD` and `sequencer/` (plus `opts`) | as above | same | same | same | same |

Docs for the commands: [git-merge.adoc#L101-L125](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-merge.adoc#L101-L125),
[git-rebase.adoc#L182-L199](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-rebase.adoc#L182-L199),
[sequencer.adoc#L1-L16](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/sequencer.adoc#L1-L16)
(shared by cherry-pick and revert). All exist in 2.34 (`cherry-pick --skip` since 2.23).

Things the table doesn't show, all **(tested)**:

- **A range cherry-pick after a manual commit.** Resolving and running `git commit` removed
  `CHERRY_PICK_HEAD` but left `sequencer/`; git still counts it as a cherry-pick in progress
  (rule 5), and `cherry-pick --continue` picked the rest.
- **In progress without conflicts:** `merge --no-commit` (`MERGE_HEAD`, no unmerged paths),
  `rebase -x false` (`rebase-merge/` only), a cherry-pick that became empty
  (`CHERRY_PICK_HEAD`, clean tree, exit 1 with "The previous cherry-pick is now empty").
- **Conflicts without an operation:** `merge --squash`, `cherry-pick -n` and `merge --quit`
  leave unmerged paths and no marker file (only `MERGE_MSG`/`AUTO_MERGE`). So "operation in
  progress" and "has conflicts" are two separate facts.
- **A branch being rebased** is shown as `detached` by `worktree list` and
  `branch --show-current` prints nothing; the branch is in `rebase-merge/head-name`
  (`refs/heads/side`). git still treats it as in use (§2, §3, §6).
- **Nothing to abort:** `merge --abort` → 128 `fatal: There is no merge to abort (MERGE_HEAD
  missing).`; `rebase --abort` → 128 `fatal: no rebase in progress`; `cherry-pick --abort` →
  128 `error: no cherry-pick or revert in progress` / `fatal: cherry-pick failed`.
- **Continue with conflicts left:** `merge --continue` and `cherry-pick --continue` → 128
  `error: Committing is not possible because you have unmerged files.`; `rebase --continue` →
  1, stdout `f.txt: needs merge` / `You must edit all merge conflicts and then` / `mark them
  as resolved using git add`.

**Reftable repositories** (tested on 2.53 with `init --ref-format=reftable`; reftable needs
2.45, so 2.34 can't even open such a repository):

- Only `FETCH_HEAD` and `MERGE_HEAD` are pseudorefs stored as files
  ([refs.c#L887-L899](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/refs.c#L887-L899),
  [glossary-content.adoc#L502-L514](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/glossary-content.adoc#L502-L514)).
  `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `REBASE_HEAD`, `ORIG_HEAD` and `AUTO_MERGE` are ordinary
  refs, and in a reftable repository they live in the table, not as files. After a conflicted
  cherry-pick, the only new file was `MERGE_MSG`; `rev-parse --verify -q CHERRY_PICK_HEAD`
  still found it.
- Directories (`rebase-merge/`, `rebase-apply/`, `sequencer/`) and `MERGE_HEAD` are files in
  both backends.

**How to ask, per worktree, all from the open worktree** (tested on both versions):

- The per-worktree refs of another worktree are reachable as `worktrees/<id>/<ref>` and
  `main-worktree/<ref>` ([git-worktree.adoc#L300-L322](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-worktree.adoc#L300-L322)).
  One `git cat-file --batch-check` fed with `worktrees/<id>/CHERRY_PICK_HEAD`,
  `worktrees/<id>/REVERT_HEAD`, … answered for every worktree at once (`<oid> commit 168` or
  `<name> missing`), in both backends.
- But `worktrees/<id>/MERGE_HEAD` was reported **missing in a reftable repository** even with
  a merge in progress there (files backend: found). So stat `MERGE_HEAD` as a file.
- `git -C <wt> rev-parse --git-path MERGE_HEAD` gives the right per-worktree path
  (`…/.git/worktrees/files-wt/MERGE_HEAD`). Everything in the git dir is per worktree except the
  names in `common_list` ([path.c#L98-L124](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/path.c#L98-L124)),
  which doesn't list any of the state files.

**(derived)** Recommended check, per worktree, with its git dir from `worktree list` (main:
`<common>`; linked: `<common>/worktrees/<id>`):

1. stat `MERGE_HEAD`, `rebase-merge/`, `rebase-apply/` (+ `applying`), `sequencer/todo`;
2. stat `CHERRY_PICK_HEAD` and `REVERT_HEAD` in a files repository; in a reftable repository
   (`extensions.refStorage = reftable`) ask with one `cat-file --batch-check` for all worktrees.
3. Apply git's precedence above. Read `rebase-*/head-name` for the branch.

That is a handful of `stat` calls per worktree, cheap enough for every reload.

### 7.4 Counting conflicted files

| Command | Output | Cost |
|---|---|---|
| `git ls-files -u -z` | Up to three lines (stages 1–3) per path: `<mode> <oid> <stage>\t<path>` | Reads the index only |
| `git diff --name-only --diff-filter=U -z` | One path per conflict | Also compares the worktree |
| `git status --porcelain=v2 -z` | `u <XY> …` lines ([git-status.adoc#L385-L389](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-status.adoc#L385-L389)) | Full status, with untracked scan |

All three agreed in every test (one conflicted file each; tested). The cost column is
**(derived)** from what each command reads. Use `ls-files -u -z` and count distinct paths; it
needs no worktree scan.

### 7.5 Editors

- **`merge --continue` and `rebase --continue` opened the editor** (tested with a logging
  `GIT_EDITOR`, both versions, both in the main and a linked worktree). `cherry-pick
  --continue` and `revert --continue` didn't. `commit --no-edit` concludes a merge without one.
- A plain `merge` opens one only if stdin and stdout are the same terminal, or when
  `GIT_MERGE_AUTOEDIT` says so
  ([merge.c#L1158-L1181](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/merge.c#L1158-L1181)),
  and `revert` only if stdin is a terminal
  ([sequencer.c#L2217-L2226](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/sequencer.c#L2217-L2226)).
- With `GIT_EDITOR=false`, `rebase --continue` exited 1 (`error: there was a problem with the
  editor 'false'` / `Please supply the message using either -m or -F option.` / `error: could
  not commit staged changes.`) and the rebase stayed in progress; `merge --continue` likewise
  kept `MERGE_HEAD` (tested).
- The editor is chosen as "`$GIT_EDITOR`, then `core.editor` configuration value, then
  `$VISUAL`, then `$EDITOR`"
  ([git-var.adoc#L41-L48](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-var.adoc#L41-L48)).
  Tested: `-c core.editor=true` did **not** stop a `GIT_EDITOR` from the environment.
  **(derived)** Parterre's operation runner should set `GIT_EDITOR=true` (the message git
  prepared is kept) and pass `--no-edit` to `merge`, `revert` and `commit`.

### 7.6 What `watch.rs` would need to notice

Tested by recording size and mtime of everything under `<common>` (except `objects/`) before and
after each step in a linked worktree:

| Step | What changed under `<common>/worktrees/<id>/` |
|---|---|
| `merge` stops on a conflict | `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE`, `AUTO_MERGE`, `index` created/changed |
| `git add` of a resolved file | `index` only |
| `git status` | nothing but the directory's own mtime (an `index.lock` came and went) |
| `commit` concluding the merge | the four merge files removed, `index`, `logs/HEAD`; and the branch ref under `<common>/refs/heads` |
| `rebase` stops | `rebase-merge/` and its 16 files, `REBASE_HEAD`, `HEAD`, `ORIG_HEAD`, `MERGE_MSG`, `AUTO_MERGE`, `index` |
| `rebase --continue` finishes | `rebase-merge/` removed, `HEAD`, and the branch ref |
| range `cherry-pick` stops | `sequencer/` with `todo`, `head`, `abort-safety`, plus `CHERRY_PICK_HEAD` etc. |

Today `RefStorage` fingerprints `HEAD`, the refs, the reftables and, per linked worktree,
`HEAD`, `gitdir`, `locked` and the worktree folder's `.git`
([watch.rs](../../crates/parterre-core/src/watch.rs)). **(derived)** To notice operations, add
for this worktree's git dir and for each `worktrees/<id>/` (and for `<common>` itself, the main
worktree's git dir):

1. `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD` (files; in reftable repositories the last two
   change the per-worktree `reftable/`, which is already walked).
2. `rebase-merge`, `rebase-apply` and `sequencer`: the directory entries themselves, not walked.
   Their mtime changes whenever git renames a file into them, which happens at every rebase
   step, so the fingerprint also moves with the rebase's progress.
3. `index`, **only while an operation is in progress there** (for the conflict count). It
   changes on every `git add`, but also whenever another tool refreshes it, so watching it
   always would reload far too often.

Not needed: `MERGE_MSG`, `AUTO_MERGE`, `ORIG_HEAD`, `REBASE_HEAD`, the files inside the state
directories, and the `worktrees/<id>` directory's own mtime (PRIOR §3.2).

## 8. Cost

Measured on this machine (Windows 11, Git for Windows, NTFS, warm cache), 7 runs each (3 for
2.34 in §8.2), min / median in milliseconds, with `GIT_OPTIONAL_LOCKS=0` as parterre sets it.
Timings are **(tested)** but only a rough guide: another machine, or Linux, will differ. Starting any git
process cost about 80–110 ms here (`git --version`), so that is the floor for every row.

### 8.1 Does a worktree have uncommitted or untracked changes?

Two checkouts: git.git itself (4,854 files) and a synthetic one (100,000 small files in 100
folders), both clean.

| Command | git.git 2.53 | git.git 2.34 | 100k files 2.53 | 100k files 2.34 |
|---|---|---|---|---|
| `status --porcelain` | 168 / 190 | 134 / 273 | 446 / 519 | 470 / 588 |
| `status --porcelain -uno` (tracked only) | 115 / 194 | 120 / 135 | 397 / 704 | 417 / 454 |
| `status --porcelain=v2 --branch` | 134 / 153 | 114 / 120 | | |
| `diff --quiet HEAD` (tracked only; exit 1 if dirty) | 95 / 117 | 99 / 102 | 415 / 461 | 431 / 519 |
| `ls-files -o --exclude-standard --directory` (untracked only) | 87 / 98 | 90 / 94 | | |
| `status --porcelain` after **every file's mtime changed** (content the same) | | | **4,686 / 7,183** | **4,626 / 5,046** |
| … the same, after one `status` run *without* `GIT_OPTIONAL_LOCKS=0` | | | 484 / 567 | 492 / 515 |

- On a normal checkout the answer costs little more than starting git. Untracked files add
  little here because neither checkout has many folders to scan.
- **The stale-index trap.** When file mtimes change but contents don't (a checkout in another
  tool, a build that touches sources, an antivirus scan, a restore from backup), git must re-read
  every such file to compare it. Normally `status` then writes the refreshed stat data back to
  the index, so it pays once. With `GIT_OPTIONAL_LOCKS=0` it doesn't: "this will prevent `git
  status` from refreshing the index as a side effect"
  ([git.adoc#L1024-L1030](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git.adoc#L1024-L1030)).
  So every run pays again: 5–7 s instead of 0.5 s on 100k files, both versions (tested).
  **(derived)** The operation runner is about to take locks anyway, so its pre-checks should run
  *without* `GIT_OPTIONAL_LOCKS=0` (or run `git update-index -q --refresh` first). Polling
  status in the background with the flag set is what to avoid.
- git's own performance advice for big trees is `-uno`, `core.untrackedCache` and
  `core.fsmonitor`
  ([git-status.adoc#L472-L527](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/git-status.adoc#L472-L527)).
  Those are the user's settings; parterre should not set them.
- **(derived)** Which command: `status --porcelain --ignore-submodules=none` when the question is
  "will `worktree remove` refuse" (it is git's own check, §5); `status --porcelain -uno` when
  it is "will rebase refuse" (§7.2); `status --porcelain` for "anything to lose". One call
  answers both tracked and untracked; separate calls don't save anything.

### 8.2 Is this commit on any other branch?

Repository: a clone of git.git (82,333 commits) with 2,000 extra local branches and 2,000 extra
remote-tracking branches pointing at random commits. Two questions:

- **tip**: `C` is the tip of branch `solo`, one commit on top of `master`, on no other branch
  (the case "can I delete `solo` without losing work", or a detached worktree's HEAD).
- **old**: `C` is `master~20000`, contained in almost every branch.

`X` is the branch being asked about, excluded from the negative side. Milliseconds, min /
median; 2.53 with 7 runs, 2.34 with 3 runs.

| Command | tip, no commit-graph | old, no commit-graph | tip, commit-graph | old, commit-graph |
|---|---|---|---|---|
| `for-each-ref --contains C refs/heads refs/remotes` (2.53) | 25,968 / 27,507 | 16,588 / 17,009 | 261 / 315 | 1,225 / 1,501 |
| same (2.34) | 79,894 / 84,012 | 60,384 / 63,416 | 371 / 400 | 2,137 / 2,236 |
| `for-each-ref --count=1 --contains …` (2.53) | 12,531 / 13,481 | 381 / 513 | 172 / 180 | 158 / 312 |
| same (2.34) | 80,029 / 81,234 | 61,868 / 62,477 | 363 / 403 | 2,271 / 2,287 |
| `branch -a --contains C` (2.53) | 27,643 / 29,287 | 15,745 / 16,541 | 272 / 299 | 1,114 / 1,278 |
| **`rev-list -1 C --not --exclude=X --branches --remotes`** (2.53) | 224 / 248 | 226 / 370 | 263 / 343 | 207 / 247 |
| same (2.34) | 364 / 378 | 352 / 380 | 337 / 354 | 334 / 336 |
| `rev-list --count C --not --exclude=X --branches --remotes` (2.53) | 220 / 247 | 241 / 281 | 227 / 243 | 216 / 236 |
| `merge-base --is-ancestor C master` (one branch only) (2.53) | 68 / 71 | 564 / 687 | 64 / 223 | 81 / 92 |
| same (2.34) | 285 / 287 | 646 / 669 | 96 / 102 | 119 / 126 |

The `--contains` rows didn't exclude `X`, so for the tip they also list `solo` itself; that
doesn't change their cost.

- **`--contains` is the wrong tool without a commit-graph.** It tests the refs one by one
  (`branch --contains` and `for-each-ref --contains` share ref-filter), which cost 16–29 s in
  2.53 and 60–84 s in 2.34 here. With a commit-graph, generation numbers let each test stop
  early: 0.3–2.2 s. `--count=1` helped in 2.53 (it stopped at the first match) but not in 2.34.
  Its answer (the branch names) is also more than the question needs.
- **`rev-list C --not --exclude=X --branches --remotes` is flat at 0.2–0.4 s** in every case,
  both versions, with or without a commit-graph. Output: nothing if `C` is on another branch,
  otherwise the commits only `X` has, newest first (`--count` gives their number, which is
  exactly what a "you will lose N commits" warning needs). `--exclude` applies to "the next
  `--all`, `--branches`, `--tags`, `--remotes`, or `--glob`", so it must come first
  ([rev-list-options.adoc#L195-L199](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/rev-list-options.adoc#L195-L199)).
  Add `--tags` if a tag counts as keeping a commit. Why it is flat is **(derived)**: the walk
  goes back from `C` and all other tips together and stops once nothing interesting is left.
- A fresh clone has no commit-graph: `fetch.writeCommitGraph` defaults to false, and
  `gc.writeCommitGraph` (default true) only writes one when gc runs
  ([config/fetch.adoc#L97-L104](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/fetch.adoc#L97-L104),
  [config/gc.adoc#L63-L68](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/gc.adoc#L63-L68)).
  The benchmark clone had none. **(derived)** So parterre can't count on one.
- **(derived)** `branch -d` itself only compares with one reference (`merge-base
  --is-ancestor`, the last row), which is cheap. "On any other branch" is a stronger question;
  answer it with the `rev-list` form, once, when the warning is about to be shown, not for every
  node in the graph.
