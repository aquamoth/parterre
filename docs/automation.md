# Automation: screenshots, scripts and recordings

parterre can drive its own window, so agents and people can check changes without clicking
through them, and make pictures and videos for pull requests, tutorials and the docs. Three
options do all of it:

| Option | Does |
| --- | --- |
| `--screenshot FILE` | Saves the window as a PNG once the graph is in, then exits |
| `--script FILE` | Runs a [workflow script](#workflow-scripts): clicks, keys, typing, screenshots of any window, dialog or menu, then exits |
| `--record FILE` | [Records](#recordings) the window as a GIF, a video or PNG frames |

`--screenshot` is a script of one step: on its own it takes the picture as soon as the window
has settled, and with `--script` it takes it after the last step.

For pictures of the graph alone, without a window, use `--export FILE` (SVG, PNG or WebP).

## Every window, dialog and menu at once

```sh
scripts/screenshots.sh /tmp/shots
```

This builds the debug binary and makes a demo repository with a change to a file and a second
worktree. Then it saves one PNG for each of these into `/tmp/shots`, in about 15 seconds:

- the main window: as opened, fitted, after dragging a node, and while searching
- the ☰ menu and each of its submenus, and the menu while a newer release is out
- the toolbar popovers
- the context menus of a node and of the canvas, with their submenus
- every settings page, and Privacy with `DO_NOT_TRACK` set
- the first-run prompt
- the Keyboard and mouse, Legend and About windows
- the log, compare, diff and blame windows
- the question before resetting all settings, and the one asking what of a settings file to
  import
- the dialogs for creating a branch, adding or deleting a worktree, reset, rebase, merge
  (into the current branch, and of the current branch into another), cherry-pick and revert

Menus are cropped to the menus, and dialogs to the dialog. Use it to check a change everywhere
at a glance, or to pick before and after pictures for a pull request.

- `PARTERRE_ARGS="--theme dark"` (or `--text-size 1.5`, …) applies to every run.
- `PARTERRE=target/release/parterre` uses another binary.

The script runs itself under `xvfb-run`, so no real pointer hovers anything and the pictures are
the same on every machine. egui paints widget id clashes in red, and only in debug builds: look
for them.

## What a scripted run does

`--screenshot` and `--script` start a scripted run. It differs from a normal start like this:

- **No stored settings.** It reads none and saves none: no remembered window size, theme or
  moved nodes. Set what you need with the [options below](#setting-the-scene).
- **Sends nothing.** No update check and no usage statistics. The first-run prompt isn't
  shown, as if answered with its defaults; `open first-run` shows it. Settings › Privacy shows
  an example install ID, the same in every run. `DO_NOT_TRACK=1` in the environment shows the
  page as that leaves it.
- **One window.** Dialogs and the other windows (log, diff, settings, …), normally windows of
  their own, are drawn inside the main window, so one picture shows them with what they belong
  to.
- **A clock of its own.** Every frame is 1/60 second for animations, the physics and
  recordings, however long it took to draw. Runs are repeatable, and a recording plays at the
  speed a person would see.
- **Waits for parterre.** Steps wait until the graph is laid out, and until diffs, blames and
  pull requests have loaded.
- **Ends with an exit code.** It exits 0 when the script is done. It exits 1 if a step fails or
  a screenshot can't be saved, after saying why on stderr.
- **Reports frame times and tidiness.** It prints the frame interval on stderr at the end, for
  benchmarks: `frame interval: mean 3.7 ms, max 41.0 ms over 22 frames`. And how tidy the graph
  is, for checking a change to the layout or the physics: `graph: 0 overlapping boxes, 6 edges
  through boxes, 0 doubling back, 0 detours`. The layout's own curves count as a few edges
  through boxes; compare the numbers before and after a change.

## Workflow scripts

`--script FILE` (or `--script -` for standard input) runs a workflow in the window, one step per
line:

```text
# Branch off a tag from its context menu.
right-click node:v0.2.0
screenshot context-menu.png popup
click "Create branch here…"
type "feature/demo"
screenshot create-branch.png window
click "Create"
wait 1
```

```sh
parterre ~/repo --window-size 1200x800 --script branch.txt
printf 'open about\nscreenshot about.png window\n' | parterre ~/repo --script -
```

| Step | Does |
| --- | --- |
| `click T`, `double-click T`, `right-click T` | Moves the pointer to target `T` and clicks |
| `ctrl-click T`, `shift-click T` | Clicks with Ctrl (⌘ on macOS) or Shift held |
| `hover T` | Moves the pointer there and rests it, e.g. to open a submenu or a tooltip |
| `drag T DX,DY` | Presses on `T`, moves by DX,DY points and lets go |
| `scroll DX,DY` | Turns the wheel where the pointer is |
| `key K` | Presses a key: `Enter`, `Escape`, `F5`, `Ctrl+O`, `Ctrl+Shift+Z`, … |
| `type "text"` | Types text where the keyboard focus is |
| `wait SECONDS` | Waits, e.g. for the physics to settle after a drag |
| `wait-for T` | Waits until `T` shows |
| `open WHAT` | Opens a window or dialog directly, see [below](#open) |
| `drag-file PATH` | Drags a file or folder from elsewhere over the window, until `drop-file` |
| `drop-file` | Drops it, as from a file manager |
| `screenshot FILE [window\|popup]` | Saves a PNG: the whole window, only the topmost window (a dialog), or only the open menus and popovers |

`#` starts a comment. Quoted text may hold `\"` and `\\`.

### Targets

Pointer steps aim where a person would:

- `"text"`: text on screen. The topmost exact match wins, else the topmost text containing it.
- `node:REF`: the node of a branch, tag or hash prefix. It must be in view.
- `canvas`: the empty spot of the graph farthest from any node.
- `toolbar:menu` (or `filter`, `zoom`, `drag`): the toolbar's menu and popover buttons.
- `X,Y`: a point, in points from the window's top left.

A step waits up to 15 seconds for its target. If the target never shows, the run fails. The
error names the line and the topmost texts on screen, so a script works as a check, too.

### Open

`open` shows a window or dialog without the pointer. Use it where clicking there would take
several steps, or where the click isn't what the picture is about. Anything naming a commit
takes a branch, a tag or a hash prefix. A name that doesn't exist fails the run.

| `open …` | Shows |
| --- | --- |
| `menu`, `filter`, `zoom`, `drag` | The ☰ menu or a toolbar popover |
| `settings`, `settings:PAGE` | The settings, at `appearance`, `branchcolours`, `graph`, `filters`, `dragging`, `privacy`, `advanced` or `manage` |
| `export-settings:FILE`, `import-settings:FILE` | The settings' Manage page, as if FILE had been picked to export to (written at once) or import from (the dialog asking what to import) |
| `about`, `shortcuts`, `legend` | About parterre, Keyboard and mouse, the legend |
| `first-run` | The first-run prompt about usage statistics and crash reports, as at the first start |
| `log:REF`, `log:FIRST..SECOND` | The log of a commit, or of a range (as if the nodes were selected in that order) |
| `compare:FIRST..SECOND` | The compare window; `SECOND` may be `WORKING_TREE` |
| `diff:COMMIT:PATH` | The diff of a file as the commit changed it, against its first parent |
| `blame:COMMIT:PATH[:LINE]` | The blame of a file at a commit (or `WORKING_TREE`), with a line chosen |
| `create-branch:REF`, `add-worktree:REF` | The branch or worktree form, starting at a commit |
| `delete-worktree:FOLDER[,FOLDER…]` | The question before deleting worktrees, by their folders' names |
| `reset:REF[:MODE]` | The reset dialog, optionally with a mode (`soft`, `mixed`, `keep`, `hard`) chosen |
| `rebase:REF`, `merge:REF` | The rebase onto a commit, or the merge of one, into the current branch |
| `revert:REF` | The revert of a commit on the current branch |
| `merge-into:BRANCH` | The merge of the current branch into local branch BRANCH, as a pull request merges |
| `cherry-pick:REF` | The cherry-pick of the commits of REF the current branch lacks, as the graph's menu offers it |
| `fetch`, `pull` | Fetches every remote, or pulls the current branch, with the window showing git's output (and, for a diverged branch, the question how) |
| `push:BRANCH[:REMOTE]` | Pushes a local branch to the first remote, or to REMOTE, asking first when it needs a force push |
| `set-upstream:BRANCH` | The dialog setting a local branch's upstream |
| `delete-remote-branch:REMOTE/BRANCH[,…]` | The question before deleting branches on their remotes |

Everything else is done as a person would do it:

- select a node with `click node:REF`;
- mark one for comparison through its context menu: `right-click node:REF`, `hover "Compare"`,
  `click "Mark for comparison"`;
- drag a node with `drag node:REF DX,DY`, then `wait 1.5` for the physics.

## Recordings

`--record FILE` records the window at 30 frames a second. It draws the pointer in, with a ring
while a button is down.

- `.gif` is encoded by parterre. A still window costs nothing, but each change does: keep GIFs
  short and small, for pull requests.
- `.mp4`, `.webm`, `.mkv` and `.mov` are made by `ffmpeg`, which must be on the `PATH`. MP4 is
  the most widely playable, and GitHub plays it in pull requests and issues.
- Any other name, without an extension, is a folder of `frame-00001.png`, `frame-00002.png`, …,
  for editing elsewhere.

With `--script`, the pointer glides to each target and text is typed a few characters a second,
so the recording reads as a person working:

```sh
parterre ~/repo --window-size 1200x800 --script branch.txt --record branch.mp4
```

Script screenshots leave the drawn pointer out, and the recording skips their frame.

Without a script, `--record` records what you do until you close the window, for a tutorial
recorded by hand. Settings are read and saved as usual then. The dialogs are drawn inside the
main window, so that they are in the recording.

## Setting the scene

These options set up the window for a run. Most change a setting for that run only; scripted
runs start from the defaults otherwise.

| Option | Sets |
| --- | --- |
| `--window-size WxH` | The window's size in points, e.g. `1200x800` |
| `--theme light\|dark\|system`, `--text-size 1.5` | Colours and the size of all text |
| `--fit`, `--zoom 0.8` | Start with the whole graph in view, and zoom around the centre |
| `--mode`, `--direction`, `--look`, `--max-row-width`, `--trunk` | What the graph shows and how it is laid out |
| `--current-branch`, `--filter`, `--hide`, `--branch-color` | Which branches show, and their colours |
| `--no-remotes`, `--no-tags`, `--pull-requests`, `--worktrees` | Which refs show |
| `--pull-requests-from FILE` | Pull requests from a file instead of GitHub, see [below](#pull-requests-without-github) |
| `--drag-mode adapt\|free\|subtree` | What moves with a dragged node |
| `--log-layout a\|b\|c\|d` | The log window's layout: `stacked`, `side-by-side`, `details-below`, `files-right` |
| `--diff-form side\|unified`, `--diff-words`, `--diff-whitespace`, `--diff-unfolded` | The diff window's settings |
| `--no-syntax-colour` | Plain text in the diff and blame windows, as their toolbar button gives |
| `--newer-release VERSION` | A newer release out, e.g. `0.7.0`: the bold blue ☰ icon and *Download 0.7.0* in the menu |

Scripted runs never ask GitHub for newer releases: `--newer-release` is the only answer they
get. *Download* does what it does for the build being run, which for one without a channel
(any local build) is copying `cargo install --locked parterre`.

`parterre --help` lists the common ones. The window settings at the bottom are hidden there,
being of use mainly for automation.

## Pull requests without GitHub

`--pull-requests-from FILE` shows the pull requests in FILE instead of asking GitHub, so
pictures of them need no network, no signed-in `gh` and no pull requests on GitHub. It turns
pull requests on. FILE is a JSON array:

```json
[
  {"number": 146, "title": "Checkout redesign", "author": "mira", "head": "feature/checkout-redesign", "base": "main"},
  {"number": 139, "title": "Dark mode", "author": "sam", "draft": true, "head": "feature/dark-mode", "base": "main"}
]
```

`head` and `base` are branches of `origin`, and each pull request is shown on the commit
`origin/<head>` is at. `origin` must still point at GitHub (`git remote set-url origin
https://github.com/owner/name.git` after fetching will do); that is where clicking a pull
request goes. A mistake in the file, or a `head` that `origin` has no branch for, fails the run.

## Running without a display

To keep a run off your desktop, and to make its pictures the same on every machine, run it in a
virtual X server with a screen larger than the window. Unset `WAYLAND_DISPLAY`, or winit picks
Wayland:

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1200x24" \
    target/debug/parterre ~/repo --window-size 1200x800 --script branch.txt --record branch.gif
```

## In the code

- `crates/parterre/src/script.rs`: the script language. Steps are parsed up front, so a typo
  fails before the window opens.
- `crates/parterre/src/automation.rs`: the runner. It turns each step into the input events of
  the following frames, finds targets, crops and saves screenshots, and keeps the clock.
- `crates/parterre/src/record.rs`: the recorder and the drawn pointer.
- `ParterreApp::open_named` in `crates/parterre/src/app.rs`: what `open` can name. To make a new
  window or dialog reachable, add it there and to the table above.
