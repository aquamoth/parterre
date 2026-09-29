# parterre

A standalone viewer for a git repository's revision graph, in the style of TortoiseGit, with a
small set of git actions reached from the graph.

## Language

### The graph

**Node**:
A commit that the revision graph draws as a box, because a ref points at it or because the
chosen mode keeps it (branchings, merges, or every commit).
_Avoid_: vertex, box

**Edge**:
A line from a node to a parent node, standing for that parent link and any commits collapsed
into it.
_Avoid_: link, arrow

**Worktree**:
One of the repository's checkouts that git knows about: the main one and each linked one, each
with its own folder and its own HEAD. The one parterre opened is one of them.
_Avoid_: checkout, clone, working copy

**Open worktree**:
The worktree parterre has open, in its one main window. Going to another worktree of the same
repository replaces it rather than opening a second window.
_Avoid_: current worktree, active worktree; "open in parterre" or "switch" for going to one

**Deleting a worktree**:
Deleting a worktree's folder, with everything in it, and git's record of it. There is no
deleting only one of the two.
_Avoid_: remove worktree, prune

### Changing the repository

**Operation**:
A git command parterre runs that changes the repository: switching, creating, pushing, pulling
or deleting a branch, fetching, adding or deleting a worktree, merging, rebasing, cherry-picking
or reverting. Only the open worktree's branch is ever merged into, rebased or pulled; other
branches can only be switched to, pushed, fast-forwarded or deleted.
_Avoid_: action (actions also include opening a folder or copying a hash), command, task

**Upstream**:
The remote branch a local branch is set to track, which pull and push use by default. It may
have a different name from the local branch, or there may be none.
_Avoid_: tracking branch, remote branch (any branch on a remote), origin

**Operation in progress**:
A merge, rebase, cherry-pick or revert that stopped partway, usually on conflicts, in one
worktree, until it is continued or aborted there.
_Avoid_: conflict state, pending merge

**Lost work**:
What an operation would destroy with no copy left: commits that no other branch, remote branch,
tag or worktree reaches, and uncommitted changes to files that aren't ignored, whether staged,
modified or untracked. Ignored files are never lost work. A pushed branch deleted locally loses
nothing.
_Avoid_: unmerged, unpushed, data loss

### The log

**Log window**:
The separate window that lists commits one per row, shows the selected commit's details and
its changed files. Opened with *Show log*.
_Avoid_: log dialog, history view

**Log query**:
What the log window lists: the tips to walk back from and the commits to leave out (a
two-node range). It knows nothing about the graph.
_Avoid_: log filter, range spec

**Log layout**:
One of a fixed set of arrangements of the log window's three panes (commits, details, changed
files): stacked (A), side by side (B), details and files below (C), files on the right (D).
_Avoid_: view, perspective, docking

**Changed files**:
The files a commit changed compared with its first parent, with their status and line counts.
_Avoid_: file list, diff, changeset

**File diff**:
The line-by-line changes to one of the changed files: what was removed from it and what was
added, between the same two versions the changed files compare.
_Avoid_: patch, delta, compare

**Diff window**:
A separate window showing one file diff. Several can be open at once. It outlives the log window
it was opened from, but closes with the repository.
_Avoid_: diff viewer, compare window, merge view

### Blame

**Blame window**:
A separate window showing, for each line of one file at one revision, the commit that last
changed it. Several can be open at once; it closes with the repository.
_Avoid_: annotate, blame view

**Blamed revision**:
The version of the file whose text a blame window shows: a commit, or the working tree.
_Avoid_: current revision, selected revision

**Chosen commit**:
The commit a blame window highlights: every line it still owns in the blamed revision, and its
row in the history pane. Choosing a line or a row chooses its commit; it never changes the
blamed revision.
_Avoid_: selected commit, active commit

**History pane**:
The list at the bottom of a blame window of the commits that changed the file, one per row.
_Avoid_: log pane, commit list
