# Using parterre

The [README](../README.md) shows what parterre is for. This guide covers every option, key and
window.

- [Starting it](#starting-it)
- [The graph](#the-graph)
- [Branches and worktrees](#branches-and-worktrees)
- [Pull requests](#pull-requests)
- [Upstreams](#upstreams)
- [The log](#the-log)
- [Diffs](#diffs)
- [Comparing commits](#comparing-commits)
- [Blame](#blame)
- [Settings](#settings)
- [Colours](#colours)

## Starting it

```sh
parterre [PATH]                    # open the repository containing PATH (default: the
                                   # current directory's, or none: the window asks for one)
parterre --mode branches           # also show every fork point and merge
parterre --mode all --no-remotes   # every commit, local branches and tags only
parterre --look classic            # straight, unbundled edges like TortoiseGit
parterre --hide 'pipeline/*,release/*'        # leave out build and release branches
parterre --branch-color 'feature/*=#9b59b6'   # colour branches by name (repeatable)
parterre --pull-requests           # show GitHub pull requests even if turned off in settings
parterre --worktrees               # show worktrees even if turned off in settings
parterre --export graph.svg        # write an SVG without opening a window
parterre --export graph.png --zoom 2   # or a PNG (or .webp), here at 200%
parterre --help                    # all options
```

The installers also add *Revision Graph* to the folder menu of Explorer, Nautilus, Dolphin
and Nemo. `Ctrl+O` opens another folder, and the ☰ menu lists the recent ones.

## The graph

parterre shows the commits that have a branch, a tag or a worktree on them, and leaves out the
commits in between, as TortoiseGit's revision graph does. *Branchings and merges* adds every
fork point and merge, and *All commits* shows everything. Hover an edge to list the commits
collapsed into it; click it to keep it highlighted while you look around.

| Do | To |
|---|---|
| Drag a node | Move it, with the rest of the selection it belongs to. In *Adapt*, the graph gives way and keeps children above their parents. |
| `1` / `2` / `3` | Drag mode *Adapt* (the graph gives way) / *Free* (nothing else moves) / *Subtree* (take along everything that grows out of it) |
| Click, `Ctrl`+click, `Shift`+click a node | Select it / toggle it / add it to the selection |
| `Shift`+drag the background | Select the nodes in a rectangle |
| Hover / click an edge | List the commits collapsed into it / keep it highlighted while you look around |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo a move |
| Drag the background, wheel, Shift+wheel | Pan |
| Ctrl+wheel, pinch, `+` `-` `0` | Zoom |
| Ctrl+wheel, pinch anywhere but over the graph | Text size of every window (also *Settings → Appearance*, and `Ctrl`+`+` `-` `0` in the log, compare, diff, blame and settings windows). The graph keeps its own zoom. |
| `F`, double-click the background | Fit the whole graph |
| `Home` / `H` | Go to HEAD |
| `Ctrl+F`, then `Enter` / `F3` | Find branches, tags, hashes, subjects or authors |
| `L`, double-click a node | Show log: the node's history, or with two nodes selected the commits between them (first..second) |
| `Ctrl+C` | Copy the selected commit's hash |
| Click a pull request's number | Open the pull request on GitHub |
| Right-click a node | Show log; compare; create, switch to or delete a branch; rebase, merge, cherry-pick or reset; add, go to or delete a worktree; open its pull requests or its worktree's folder; copy its hash, ref names or folder; select its subtree; return it to the layout |
| `R` | Return all nodes to the layout |
| `Esc` | Clear the selection |
| `F5` | Reload the repository (it also reloads by itself when branches, tags or HEAD change) |
| `Ctrl+O` / `Ctrl+W` | Open / close a folder |
| `Ctrl+,` | Settings |

When you drag a node, edges at moved nodes are routed afresh through the gaps between nodes, so
they lose bends they no longer need and go around nodes that are now in the way. With
*Remember moved nodes* on (*Settings → Dragging*), parterre keeps your arrangement for each
repository; new commits keep their place beside a moved parent.

The toolbar holds what you use every day, the ☰ menu has all of that and more, and *Settings*
the rest. The graph shows every change while the settings are open. Besides TortoiseGit's
options (branchings and merges, local and remote branches, tags, arrows towards merges, zoom,
the overview map and export), there are:

- four directions and three vertical placements;
- edge bundling, row splitting and curved edges;
- a first-parent-only view, and stashes or other refs;
- hiding branches by wildcard, e.g. `pipeline/*` (the toolbar's filter button, or *Settings →
  Filters*). A hidden branch still shows where the history of a shown branch contains it, so
  only leaves vanish;
- colours by branch name, e.g. `feature/*` purple (*Settings → Branch colours*);
- light and dark themes.

## Branches and worktrees

Everything parterre changes, it changes with git, in the **open worktree**: the one you opened,
marked with a folder on its HEAD. Each dialog shows the git command it runs, under *Git
command*.

**Worktrees** are off by default; the toolbar's folder button shows them. Each one is marked
with a folder, first on its commit, in the graph and in the log: the branches they have checked
out, even where hidden, and detached ones in cyan, with the folder's name in italics. A worktree
whose folder is gone gets a crossed-out folder. From a node's menu:

- *Add worktree here…* makes a new worktree at the commit, on a new branch that can track a
  remote one, in a folder next to the repository's (`<repo>.worktrees/` unless you pick
  another). Tick *Go to new worktree* to make it the open one.
- *Go to worktree* makes another worktree the open one. The layout, the view, moved nodes and
  open windows stay: it is the same history.
- *Delete worktree* deletes one, with its folder, after asking.
- *Open* › *File system* / *Terminal* opens its folder; *Copy* copies the folder's path.

**Branches:** *Create branch here…* (optionally switching to it), *Switch to* a branch or a
commit (detached), and *Delete branch*.

**The open worktree's branch**, from the graph and the log:

| Menu entry | Does |
|---|---|
| *Merge into main* › … | Merges a branch or commit into it: fast-forward or a merge commit, with the message to use. |
| *Merge main into feature/x…* | Merges it into another local branch the way a pull request does: fast-forward, merge commit, rebase and fast-forward, or semi-linear merge. |
| *Rebase main onto* › … | Rebases it onto a branch or commit. Pick, squash or drop each commit first. |
| *Cherry-pick feature/x onto main…* | Picks the commits of another branch that it lacks; drop the ones you don't want. `-x` is remembered. |
| *Reset main to here…* | `git reset` with the mode you choose (soft, mixed, keep, hard); the dialog lists the files each mode leaves changed, and what would be lost. |
| *Revert in main…* (log) | Reverts a commit, with the message git words for it. |

Merges, rebases, cherry-picks and reverts offer to stash uncommitted changes first, and to put
them back afterwards. A worktree stopped part-way, by a conflict or a `break`, shows it in the
graph: a rebase with an orange zigzag from its HEAD (the commits replayed so far) to the branch
being rebased, a merge or cherry-pick with an orange dashed arrow to the commit coming in. A
banner across the graph says what is stopped and lists the conflicted files; finish or abort it
with git, or go to another worktree.

## Pull requests

Open pull requests show as labels on the commits they propose when `origin` is on GitHub and
[`gh`](https://cli.github.com) is signed in (`gh auth login`). Hover one for its title, author
and branches; click it to open it in the browser. Drafts are greyed out. The toolbar's
pull-request button hides them, and if they can't be shown, turning them on there says why.

A pull request shows once its branch has been fetched and its base branch is shown. parterre
asks GitHub about the fetched branches only, at most once a minute per repository, and never
without signing in. Once less than a tenth of your hourly API allowance is left, it waits for
the next hour.

## Upstreams

Hover or select a branch (or its upstream) and the commits between the two are coloured: green
ahead, blue behind, red lost to a force push, grey dashed replaced by a rebase. A rebased
branch has a dashed edge to its upstream. The status bar shows `branch 3|2` (ahead|behind), the
log's branch labels ↑3 ↓2, and *Compare → Upstream* compares the two. On by default
(*Settings → Advanced*).

## The log

*Show log* opens a window listing a node's history, or the commits between two selected nodes,
like TortoiseGit's log: a graph column, the selected commit's message and the files it changed,
which you can sort and filter. The arrow keys move through the commits, `Ctrl+F` finds, `F5`
reloads and `Esc` closes it. Right-click a row for the same actions as on a node, and to
revert the commit or copy its subject.

Four layouts arrange its panes: stacked as in TortoiseGit, side by side, details and files
below, or files on the right. Pick one in the window's header or in *Settings → Appearance*;
the dividers between the panes are remembered for each layout.

## Diffs

Double-click a changed file, or select some (`Ctrl`+click, `Shift`+click) and press `Enter`, to
see its diff in a window of its own; several can be open at once. The diff is side by side or
unified (`Ctrl+D`), with changed words marked, unchanged stretches folded (click a fold to open
it), an overview of the changes on the right, and long lines that scroll sideways. Code is
coloured by syntax, as in VS Code, for Markdown, Java, C#, Rust, TypeScript, Python, SQL,
Protocol Buffers and a dozen more languages, in the blame window too; the palette button in
either toolbar turns it off, for both, as does *Syntax colour* in *Settings → Appearance*.
`Ctrl+Down` / `Ctrl+Up` (or `F7` / `Shift+F7`) move between changes, and `Ctrl+F` finds. The
toolbar also picks how changed words are found and whether whitespace counts.

Drag over the old or the new text (double-click for a word, `Shift`+click to extend, `Ctrl+A`
for all) or click line numbers for whole lines, then `Ctrl+C` copies it as it is in the file,
tabs kept. In the unified form you choose one version: the one of the line you start on (a
removed line, or the old numbers, for the old version; `Ctrl` on an unchanged line for the old
one too), shown by its line number lighting up on the row under the pointer. Lines of the other
version are left out.

Files go through git's textconv filters, as `git show` does; binary files and submodules say
what changed instead.

## Comparing commits

Comparing two commits lists the files they differ in, in a window with the same table;
double-click one for its diff. Right-click a node for *Compare* › *HEAD*, *Working tree* (your
uncommitted changes, staged or not; `F5` lists them again), *Upstream*, or with two nodes
selected *Selected revisions*. To compare commits far apart, *Mark for comparison* one (from the
node menu or a row in the log) and pick *Compare with marked* on the other, from any log. A
range log's *Compare files* compares its two ends. The window's *Since common ancestor* shows
only what the right-hand side changed since the two forked, as a pull request does; *Swap sides*
turns the comparison round.

## Blame

Blame shows which commit last changed each line of a file: right-click a changed file in the
log or compare window and pick *Blame*, or click *Blame* in a diff window's toolbar (it opens at
the change in view). A gutter names each line's commit, author and date, shaded from plain
(oldest) to amber (newest). Click a line to highlight every line of its commit; the strip on the
right marks them in the whole file (click it to go there), and the bar at the bottom describes
the commit under the pointer.

Right-click a line to *Blame previous revision* (the file as it was before that commit, in a new
window, at that line), *Show changes* (that commit's diff of the file, at the line), *Show log*
from the commit, or copy its hash. Drag or `Shift`+click to choose lines, `Ctrl+C` to copy them.
The toolbar says whether whitespace changes and moved or copied lines count (`git blame -w`,
`-M`, `-C`). Blaming the working tree marks the lines you haven't committed; `F5` blames again.

Below the text, the **history pane** lists the commits that changed the file up to the blamed
revision, like the log: graph, hash (in its lines' shade), subject, author, date. Commits from
before a rename say the path the file had there; commits none of whose lines remain are greyed
out; *Working tree changes* sits on top when the file differs from `HEAD`. Clicking a line
selects its commit's row, and clicking a row highlights its commit's lines. `Up` and `Down` step
through the rows. Right-click a row to *Blame this revision*, *Show changes* (also a
double-click), *Show log* or copy its hash. Drag the divider to resize the pane, or hide it with
*History* in the toolbar.

## Settings

`Ctrl+,` opens the settings; the graph follows every change while they are open.

- **Per repository:** filters are kept for each repository, and shared by its worktrees.
- **Shared with a team:** *Settings → Manage* exports parterre's own settings, or a
  repository's filters, to a file that another computer or a whole team can import, choosing
  what to take. It also resets everything to the defaults. The files are versioned JSON: older
  and newer versions of parterre read all they know of them.

## Colours

Colours follow TortoiseGit:

| Label | Colour |
|---|---|
| Current branch | red |
| Local branches | green |
| Remote branches | light orange |
| Tags | yellow |
| Worktrees | cyan when detached |
| Pull requests | blue, grey for drafts |
| Commits without refs | pale lavender, showing an 8-digit hash |

Colours chosen per branch name replace these, except for the current branch. *Legend* in the
☰ menu shows them all.
