# Screenshots and recordings

parterre can drive itself: take screenshots of any window, dialog or menu, and record workflows
as a GIF or a video, for pull requests, tutorials and the docs. For one plain screenshot,
`--screenshot out.png` is enough (see [building](building.md#checking-visuals-without-a-human)).

## Every window, dialog and menu at once

```sh
scripts/screenshots.sh /tmp/shots
```

This builds the debug binary and makes a demo repository with a second worktree. Then it saves
one PNG for each of the following into `/tmp/shots`, in about 30 seconds:

- the main window, fitted, and while searching
- the ☰ menu and each of its submenus
- the toolbar popovers
- the context menus of a node and of the canvas, with their submenus
- every settings page
- the Keyboard and mouse, Legend and About windows
- the log, compare, diff and blame windows
- the dialogs for creating a branch, adding or deleting a worktree, reset, rebase and merge

Menus are cropped to the menus, and dialogs to the dialog. Use it to check a change everywhere
at a glance, or to pick before and after pictures for a pull request. `PARTERRE_ARGS="--theme
dark"` (or `--text-size 1.5`, …) applies to every run, and `PARTERRE=target/release/parterre`
uses another binary.

It runs under `xvfb-run`, so no real pointer hovers anything and the pictures are the same on
every machine. Scripted runs read and save no settings. egui paints widget id clashes in red,
and only in debug builds: look for them.

## Workflow scripts

`--script FILE` (or `--script -` for standard input) runs a workflow in the real window, one
step per line, then exits:

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
```

| Step | Does |
| --- | --- |
| `click T`, `double-click T`, `right-click T` | Moves the pointer to target `T` and clicks |
| `hover T` | Moves the pointer there and rests it |
| `drag T DX,DY` | Presses on `T`, moves by DX,DY points and lets go |
| `scroll DX,DY` | Turns the wheel where the pointer is |
| `key K` | Presses a key: `Enter`, `Escape`, `F5`, `Ctrl+O`, `Ctrl+Shift+Z`, … |
| `type "text"` | Types text where the keyboard focus is |
| `wait SECONDS` | Waits, e.g. for an animation |
| `wait-for T` | Waits until `T` shows |
| `open WHAT` | Opens something without the pointer: `menu`, `filter`, `zoom`, `drag`, `settings` or `settings:PAGE`, `about`, `shortcuts`, `legend`, or a dialog as `--demo-open` names it (`create-branch:REF`, `add-worktree:REF`, `delete-worktree:FOLDER`, `reset:REF[:MODE]`, `rebase:REF`, `merge:REF`) |
| `screenshot FILE [window\|popup]` | Saves a PNG of the whole window, the topmost window (a dialog), or the open menus and popovers |

Targets are what a person would aim at:

- `"text"`: text on screen. The topmost exact match wins, else the topmost text containing it.
- `node:REF`: the node of a branch, tag or hash prefix.
- `canvas`: the empty spot of the graph farthest from any node.
- `toolbar:menu` (or `filter`, `zoom`, `drag`): the toolbar's menu and popover buttons.
- `X,Y`: a point, in points from the window's top left.

`#` starts a comment. Quoted text may hold `\"` and `\\`.

A step waits for its target to show. After 15 seconds the run stops with exit code 1, naming
the line and the topmost texts on screen, so a script works as a check, too. The `--demo-*`
flags still apply and run first, e.g. `--demo-log main` opens the log for a script to click in.
`--screenshot FILE` takes its picture after the last step.

The dialogs, which are windows of their own normally, are drawn inside the main window in
scripted runs, so one picture shows them with what they belong to.

## Recordings

`--record FILE` records the window at 30 frames a second, with the pointer drawn in (a ring
while a button is down):

- `.gif` is encoded by parterre. A still window costs nothing, but each change does: keep GIFs
  short and small, for pull requests.
- `.mp4`, `.webm`, `.mkv` and `.mov` are made by `ffmpeg`, which must be on the `PATH`. MP4 is
  the most widely playable, and GitHub plays it in pull requests and issues.
- Any other name, without an extension, is a folder of `frame-00001.png`, `frame-00002.png`, …,
  for editing elsewhere.

With a script, the recording runs on a clock of its own: each frame is 1/60 second, however
long the window took to draw and capture it, so the video plays at the speed a person would
see. The pointer glides to each target, and text is typed a few characters a second:

```sh
parterre ~/repo --window-size 1200x800 --script branch.txt --record branch.mp4
```

Without a script, `--record` records what you do until you close the window, for a tutorial
recorded by hand. The dialogs are drawn inside the main window then too, so they are in the
recording.

Recording a script under `xvfb-run` keeps your desktop free and the pictures identical:

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1200x24" \
    target/debug/parterre ~/repo --window-size 1200x800 --script branch.txt --record branch.gif
```
