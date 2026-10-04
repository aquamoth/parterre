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

**Stuck worktree**:
A worktree with an operation in progress or conflicted files. Parterre changes nothing there
until it's finished or aborted.
_Avoid_: blocked worktree, error state

**Conflicted file**:
A file git's index still holds unmerged. It's resolved once staged, whatever its contents.
_Avoid_: file with conflict markers

**Merge tool**:
The external program a conflicted file is resolved in: the one the user's git config names, or
one picked in parterre, which remembers the choice in git's config or only until it closes.
_Avoid_: diff tool, resolver, editor

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
Taking another branch's or commit's commits into the open worktree's branch, or, as a pull
request does, the open worktree's branch into another local branch, by a merge method. Into
another branch, the merge runs in that branch's worktree, or, when it's checked out nowhere,
moves its ref or switches to it and back. It can stop on a conflict, or when a hook refuses to
commit, leaving an operation in progress.
_Avoid_: pulling in, integrating

**Reverting a commit**:
Undoing a commit with a new one at the tip of the open worktree's branch, whose message names
the commit undone. A merge is undone against its first parent. It can stop on a conflict,
leaving an operation in progress.
_Avoid_: undoing a commit, rolling back, resetting

**Merge method**:
How a merge takes the other commits in: a fast-forward, which moves the branch up to them, or
a merge commit, which joins them in even where a fast-forward would do. Merging the open
worktree's branch into another, also rebase and fast-forward, or a semi-linear merge, which
rebase the open worktree's branch onto the other first.
_Avoid_: merge strategy (git's name for the algorithm that combines the changes), merge type

**Cherry-picking**:
Copying commits onto the open worktree's branch as new commits, oldest first, as if applied
one at a time: either the commits chosen in the log, which need not be adjacent, or, from the
graph, every commit of a node the branch lacks. Merges, and commits whose change is already
there, are left out. It can stop on a conflict, leaving an operation in progress.
_Avoid_: copying commits, porting, backporting

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

### What parterre sends

**Update check**:
Asking GitHub whether a newer release is out, at start and then once a day. It sends nothing of
parterre's own. When a newer version is out, the menu icon turns blue and offers *Download*.
_Avoid_: ping, heartbeat, telemetry

**Install ID**:
A random identifier created the first time parterre runs and never changed, so that each
installation is counted once. It goes with the usage statistics, is never tied to anything
personal and is never sent with a crash report.
_Avoid_: user ID, machine ID, device ID

**Usage statistics**:
What parterre sends to PostHog as it is used (installs, updates, launches, features, settings),
with the install ID, unless the user unticks them. They never contain personal information.
_Avoid_: analytics, telemetry, metrics

**First-run prompt**:
The dialog at parterre's first start that asks whether to send usage statistics and crash
reports. Only *Continue* closes it, and nothing is sent, the update check included, before it is
answered.
_Avoid_: consent dialog, opt-in screen, telemetry prompt

**Crash report**:
What parterre sends to PostHog the moment it crashes, if the user has ticked crash reports. It
may contain personal information, so it is off unless the user turns it on.
_Avoid_: error report, crash dump, bug report
