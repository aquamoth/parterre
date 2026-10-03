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

**Default branch**:
The branch `origin/HEAD` names as the repository's main line of work. The graph keeps it in
the leftmost column, whichever worktree is open and whatever HEAD is.
_Avoid_: trunk, main branch

**Worktree**:
One of the repository's checkouts that git knows about: the main one and each linked one, each
with its own folder and its own HEAD. The one parterre opened is one of them.
_Avoid_: checkout, clone, working copy

### Repository operations

**Local branch**:
A named line of work in this repository, pointing to a commit. A worktree can have it checked
out.
_Avoid_: worktree, checkout

**Remote-tracking branch**:
A local record of a branch on a remote, reflecting the last fetch from that remote.
_Avoid_: remote worktree

**Local tracking branch**:
A local branch configured with an upstream. Several local branches can share an upstream,
and their names and commits can differ from it.
_Avoid_: remote branch

**Switching branches**:
Changing which branch the open worktree has checked out.
_Avoid_: going to a worktree, opening a branch

**Open worktree**:
The worktree parterre currently shows in its main window.
_Avoid_: active checkout, current working copy

**Operation**:
A git action run from parterre that changes the repository or one of its worktrees.
_Avoid_: command, task

**Upstream**:
The branch configured as a local branch's tracking target, against which git counts ahead and
behind commits.
_Avoid_: parent branch, remote counterpart

**Operation in progress**:
A merge, rebase, cherry-pick or revert that git has started and left unfinished in a worktree.
_Avoid_: running command, pending task

**Lost work**:
Commits no other branch, remote branch, tag or worktree reaches, and uncommitted changes to
non-ignored files, whether staged, modified or untracked.
_Avoid_: unpushed work, dirty worktree

**Resetting a branch**:
Moving a local branch to another commit. For the open worktree's branch, it also decides what
happens to the changes in the commits moved away from, and to uncommitted changes.
_Avoid_: moving a branch, force-updating

**Rebasing a branch**:
Replaying the open worktree's branch's own commits onto another commit, one by one. Commits
whose change is already there are left out, and merges are flattened. It can stop on a
conflict, leaving an operation in progress.
_Avoid_: moving a branch, updating a branch

**Merging a branch**:
Taking another branch's or commit's commits into the open worktree's branch, by a merge
method. It can stop on a conflict, or when a hook refuses to commit, leaving an operation in
progress.
_Avoid_: pulling in, integrating

**Merge method**:
How a merge takes the other commits in: a fast-forward, which moves the branch up to them, or
a merge commit, which joins them in even where a fast-forward would do.
_Avoid_: merge strategy (git's name for the algorithm that combines the changes), merge type

**Adding a worktree**:
Creating a new worktree in its own folder, with a branch or a detached HEAD checked out.
_Avoid_: creating a worktree, new worktree

**Going to a worktree**:
Making another worktree of the same repository the open worktree, in the same window.
_Avoid_: switching to a worktree, opening a worktree

**Deleting a worktree**:
Removing a worktree from git and deleting its folder and everything in it.
_Avoid_: removing a worktree, closing a worktree

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
