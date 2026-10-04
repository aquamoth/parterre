# Running the user's merge tool with no terminal

Research note for [#269](https://github.com/aquamoth/parterre/issues/269): *how parterre runs
the user's own external merge tool for a conflicted file, with no terminal attached, and waits
for it, so the file drops off the list once it's resolved.* Part of
[Finishing a stuck worktree](https://github.com/aquamoth/parterre/issues/267). The runner's
environment is the one in [#139](https://github.com/aquamoth/parterre/issues/139)
(`docs/research/git-without-terminal.md` on `research/git-without-terminal`). Hard constraint:
git 2.34.

Sources are git's own scripts at pinned commits, the tools' official documentation and source,
and experiments I ran. A statement that is my own conclusion is marked **(derived)**. A statement
I could not check is marked **(unverified)**. A statement I checked by running it is marked
**(tested)**. The experiments ran on Linux (Ubuntu 24.04 base) with git 2.43.0 and git 2.34.1
built from source, every scenario on both. `git mergetool` was started the way parterre's runner
starts git: `setsid` (no controlling terminal), stdin `/dev/null`, output captured, isolated
config. The tool was a fake script set as `mergetool.fake.cmd`, which logged what it got and
then wrote, touched, left alone or failed on the file. Both versions behaved the same except
where noted. Windows and macOS were not tested, and no real merge tool was run.

## TL;DR

- **`git mergetool` runs headless for an ordinary content conflict** (both sides present, no
  symlink, no submodule) when it is told the tool and not to prompt:
  `git mergetool --no-prompt [--gui | --tool=<name>] -- <file>`. It writes BASE, LOCAL and
  REMOTE, runs the tool, waits for it, and stages the file itself. **(tested)**
- **Every other path reads stdin.** With stdin at end-of-file (or closed) each prompt answers
  "no" or "abort": exit 1, nothing staged. Most of them also **leave their temp files in the
  worktree** **(tested)**. They are:
  - deleted against modified, symlink and submodule conflicts (`(m)odified or (d)eleted`,
    `(l)ocal or (r)emote`);
  - **no merge tool configured**: git guesses one and then always asks "Hit return to start
    merge resolution tool", **even with `--no-prompt`**;
  - `mergetool.prompt=true` without `--no-prompt`;
  - "Was the merge successful?" for a tool whose exit code isn't trusted, when the file's mtime
    didn't change. Here end-of-file is the right answer: the file is put back, temps removed.
- **Exit code 0 doesn't mean resolved.** Git stages the file when a trusted tool exits 0, or an
  untrusted tool changed the file's mtime. Neither looks at the contents, so a file saved with
  conflict markers is staged **(tested)**. And if `git add` fails (another process holds
  `index.lock`) git still exits 0, and the file stays conflicted **(tested)**. The index decides,
  as the map already says.
- **Exit code 1 doesn't mean nothing changed.** A trusted tool that saves and then exits
  non-zero has its result thrown away: git puts the backup back **(tested)**.
- **Killing `git mergetool` leaves `<name>_BACKUP_<pid>.<ext>`, `_BASE_`, `_LOCAL_` and
  `_REMOTE_` in the worktree**, as untracked files, unless `mergetool.writeToTemp=true`. Killing
  only git's pid orphans the tool, whose window stays open **(tested)**.
- **One run at a time per worktree.** Two runs that finish together collide on `index.lock`;
  the loser exits 0 but nothing is staged **(tested** on 2.34.1**)**.
- **Launching the tool from parterre** means re-implementing 30-odd command lines that git
  keeps in shell scripts. Git has no stable way to get a built-in tool's command line.
  `--tool-help` lists names, not commands. git-gui does it this way and has drifted from
  `git mergetool` (§6).
- **Recommendation (derived):** run `git mergetool --no-prompt --gui -- <file>` (or with
  `--tool=<name>`) per file, one at a time, behind three checks parterre makes first:
  a tool is configured, the conflict is a content conflict, and the tool is not a terminal tool.
  Then re-read the index. Everything else gets parterre's own *Use mine* / *Use theirs* (§7).

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT234 | git `v2.34.1` @ `e9d7761b` | https://github.com/git/git/blob/e9d7761bb94f20acc98824275e317fa82436c25d/ |
| GIT | git `master` @ `8103b446` (v2.56.0-72) | https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/ |
| GFW | Git for Windows `v2.56.0.windows.1` @ `49d759b6` | https://github.com/git-for-windows/git/blob/49d759b698127791a5f3f2759c69b983846711dd/ |
| GITDOCS | git reference manual (current) | https://git-scm.com/docs |
| WINMERGE | WinMerge `master` @ `004e5ab4` | https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/ |
| KDIFF3 | KDiff3 `master` @ `65c4c35a` | https://invent.kde.org/sdk/kdiff3/-/blob/65c4c35a715e868fade6cc288a09fc70d9bd86f6/ |
| MELD | Meld `main` @ `e0931d11` | https://gitlab.gnome.org/GNOME/meld/-/blob/e0931d117c187fc7ef14f38932e86aba4a00accb/ |
| TGIT | TortoiseGit `master` @ `7338078f` | https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |

The mergetool scripts in GFW are byte-identical to GIT master's: Git for Windows doesn't patch
`git-mergetool.sh`, `git-mergetool--lib.sh` or the `mergetools/` it ships **(tested:** `diff`
of both files and of `winmerge`, `kdiff3`, `tortoisemerge`, `bc`, `p4merge`, `meld`,
`vscode`**)**.

## 1. What `git mergetool -- <file>` does

`git-mergetool.sh` is a shell script; on Windows it runs in Git for Windows' `sh`. For one file
([GIT `git-mergetool.sh` L249-429](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L249-L429)):

1. `git ls-files -u -- <file>`. No stages: prints "No files need merging" and **exits 0**, also
   for a path that doesn't exist **(tested)**.
2. Moves the file to `<name>_BACKUP_<pid>.<ext>` and copies it back. Writes stages 1, 2 and 3 to
   `<name>_BASE_<pid>.<ext>`, `_LOCAL_`, `_REMOTE_`, next to the file, or in a `mktemp -d` folder
   with `mergetool.writeToTemp=true` (L286-291, L323-334). Paths are relative to the worktree's
   top, which is the tool's working directory **(tested)**.
3. Submodule, deleted and symlink conflicts go to their own prompts (L314-321, L370-388).
4. With `mergetool.hideResolved=true` (2.31+, default false), LOCAL and REMOTE are rewritten so
   only the unresolved hunks differ (L336-368; tested).
5. "Hit return to start merge resolution tool" if the tool was guessed or `prompt` is true
   (L393-397).
6. Runs the tool and decides success (§2). On failure it prints `merge of <file> failed`, puts
   the backup back, and removes the temps (L406-417).
7. On success, the backup becomes `<file>.orig` (`mergetool.keepBackup`, **default true**) or is
   deleted, then **`git add -- <file>`**, whose result is not checked, then the temps are removed
   (L419-428).

With no file arguments it walks every conflicted file. After a failure it asks "Continue
merging other unresolved paths [y/n]?" (L431-445, L563-576).

**The exit code** is 0 when every file succeeded or none needed merging, and 1 otherwise. The
same 1 covers a declined prompt, a failed tool, an unknown tool and no tool found **(tested)**.

**`BASE`, `LOCAL`, `REMOTE` and `MERGED` are shell variables, not environment variables.** A
`mergetool.<name>.cmd` is `eval`ed with them in scope, so it must name them (`"$MERGED"`). A
script named alone sees them empty **(tested;**
[GIT `git-mergetool--lib.sh` L167-182](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L167-L182)**)**.

## 2. Success: `trustExitCode` and the mtime check

`mergetool.<name>.trustExitCode`, else the tool script's own `exit_code_trustable`, else false
([GIT `git-mergetool--lib.sh` L294-304, L335-345](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L294-L345);
[`mergetool.<tool>.trustExitCode`](https://git-scm.com/docs/git-config#Documentation/git-config.txt-mergetoollttoolgttrustExitCode)).
Built-in tools that trust their exit code: `kdiff3`, `vimdiff`/`nvimdiff`/`gvimdiff`, `emerge`,
`tkdiff`, `diffmerge`, `guiffy`, `deltawalker`, `kompare`. Not: `meld`, `winmerge`, `bc`,
`p4merge`, `tortoisemerge`, `vscode`, `smerge`, `araxis`, and every custom `cmd`
(same in 2.34.1 and master).

- **Trusted:** exit 0 is success, anything else is failure.
- **Not trusted:** git touches the backup before the tool runs. Success if the file is newer than
  the backup afterwards. Otherwise it prints "`<file>` seems unchanged." and asks "Was the merge
  successful [y/n]?"; end-of-file answers no
  ([GIT `git-mergetool--lib.sh` L143-159](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L143-L159)).

What happens with stdin `/dev/null` **(tested,** same on 2.43.0 and 2.34.1**)**:

| The tool… | not trusted (default) | `trustExitCode=true` |
|---|---|---|
| writes a resolution, exits 0 | staged, exit 0, `.orig` left | staged, exit 0, `.orig` left |
| saves the file **with markers** (or just touches it), exits 0 | **staged**, exit 0 | **staged**, exit 0 |
| leaves the file alone, exits 0 | "seems unchanged" → conflicted, exit 1 | **staged with markers**, exit 0 |
| leaves the file alone, exits 1 | "seems unchanged" → conflicted, exit 1 | conflicted, exit 1 |
| writes a resolution, exits 1 | staged, exit 0 | **conflicted, the resolution is overwritten by the backup**, exit 1 |
| returns at once and writes later (forks) | "seems unchanged" → conflicted, exit 1; the later write is left unstaged | would stage the unresolved file at once **(derived)** |

When a run fails this way, no temp files are left: the failure path cleans up **(tested)**.

## 3. Where it still reads stdin

Every prompt is `read ans || return 1`, so end-of-file and a closed stdin both give "no" or
"abort" **(tested:** `</dev/null` and `<&-`**)**:

| Prompt | When | At end-of-file **(tested)** |
|---|---|---|
| `Use (m)odified or (d)eleted file, or (a)bort?` (`(c)reated` without a base) | One side deleted it ([L117-152](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L117-L152)) | exit 1; **`_BACKUP_`, `_BASE_`, `_LOCAL_`, `_REMOTE_` left in the worktree** |
| `Use (l)ocal or (r)emote, or (a)bort?` | Either side a symlink ([L92-115](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L92-L115)) | exit 1; `_BASE_`, `_LOCAL_`, `_REMOTE_` left |
| `Use (l)ocal or (r)emote, or (a)bort?` | Either side a submodule ([L154-213](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L154-L213)) | exit 1, nothing written |
| `Hit return to start merge resolution tool (<tool>):` | Tool guessed, **or** `mergetool.prompt=true` ([L393-397](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L393-L397)) | exit 1; all four temps left |
| `Was the merge successful [y/n]?` | Untrusted tool, file unchanged | exit 1, cleaned up (§2) |
| `Continue merging other unresolved paths [y/n]?` | Several files, one failed | stops there, exit 1 |

- `--no-prompt` (`-y`) turns off `mergetool.prompt`, but **not the prompt for a guessed tool**:
  `guessed_merge_tool=true` prompts regardless
  ([L393](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool.sh#L393);
  tested with `-y`). The manual says `-y` is "the default if the merge resolution program is
  explicitly specified" ([git-mergetool](https://git-scm.com/docs/git-mergetool)); passing
  `--tool=<name>` also makes the tool not guessed **(tested)**.
- The tool's own stdin is the same `/dev/null` **(tested)**. Binary files go to the tool like
  any other content conflict **(tested)**.

## 4. Which tool: configuration and the fallback

**Selection** ([GIT `git-mergetool--lib.sh` L438-524](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L438-L524);
[GIT234 L389-427](https://github.com/git/git/blob/e9d7761bb94f20acc98824275e317fa82436c25d/git-mergetool--lib.sh#L389-L427)):

1. `--tool=<name>`.
2. In GUI mode, `merge.guitool`, then `merge.tool`. Otherwise `merge.tool` only. `diff.tool` is
   never read by `git mergetool` (only `git difftool` falls back to `merge.tool`).
3. A name that is neither a built-in nor has `mergetool.<name>.cmd` prints "git config option
   merge.tool set to unknown tool: … Resetting to default..." and falls through to guessing
   **(tested)**.
4. Guessing (§4.1).

**GUI mode** is `-g`/`--gui` (git 2.20), with the fall-back from `merge.guitool` to `merge.tool`
since 2.22. **`mergetool.guiDefault` is git 2.41**: `true` acts like `--gui`, and `auto` means
GUI mode when `DISPLAY` is set; `--no-gui` overrides it
([commit `42943b95`](https://github.com/git/git/commit/42943b950e), in v2.41.0;
[`mergetool.guiDefault`](https://git-scm.com/docs/git-config#Documentation/git-config.txt-mergetoolguiDefault);
[GIT `git-mergetool--lib.sh` L100-137](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L100-L137)).
2.34.1 ignores it: with `merge.tool=kdiff3`, `merge.guitool=meld`, `guiDefault=auto` and
`DISPLAY` set, 2.43 ran meld and 2.34.1 ran kdiff3. With `--gui` both ran meld **(tested)**.

**A tool's path:** `mergetool.<name>.path`, else the script's `translate_merge_tool_path`, else
the name on `PATH`. A custom `cmd` is run as written. A built-in that isn't found fails with
"The merge tool kdiff3 is not available as 'kdiff3.exe'", exit 1 (the `.exe` on Linux comes
from kdiff3's Windows fallback) **(tested;**
[GIT L478-505](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L478-L505)**)**.
A user `cmd` for a built-in name overrides the built-in command
([GIT L261-263](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L261-L263)).

**An unknown `--tool=nosuch` fails silently before 2.48**: exit 1, only "Merging: a.txt"
printed **(tested** 2.34.1 and 2.43**)**. 2.48 added "error: mergetool.nosuch.cmd not set for tool 'nosuch'"
([commit `bba503d4`](https://github.com/git/git/commit/bba503d43e)).

**What counts as configured (derived):** `merge.tool` (or, with `--gui`, `merge.guitool`) is set
to a name that is either a built-in (a file in `$(git --exec-path)/mergetools/`, or one of its
variants such as `bc4`, `vimdiff3`) or has `mergetool.<name>.cmd`. Whether the program is
installed is a separate question that only running it answers.

### 4.1 When nothing is configured

`guess_merge_tool` prints "This message is displayed because 'merge.tool' is not configured."
and the candidate list, then takes the first one whose program is found
([GIT L347-377, L417-436](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L347-L436);
the same in [GIT234 L298-387](https://github.com/git/git/blob/e9d7761bb94f20acc98824275e317fa82436c25d/git-mergetool--lib.sh#L298-L387)):

| Environment | Candidates, in order |
|---|---|
| `DISPLAY` set, GNOME (`GNOME_DESKTOP_SESSION_ID`) | meld opendiff kdiff3 tkdiff xxdiff tortoisemerge gvimdiff diffuse diffmerge ecmerge p4merge araxis bc codecompare smerge, then the editors |
| `DISPLAY` set, otherwise | opendiff kdiff3 tkdiff xxdiff meld tortoisemerge gvimdiff … smerge, then the editors |
| `DISPLAY` unset | **tortoisemerge**, then the editors |
| editors (`VISUAL`/`EDITOR`) | `*nvim*`: nvimdiff vimdiff emerge; `*vim*`: vimdiff nvimdiff emerge; else emerge vimdiff nvimdiff |

- **`DISPLAY` is the only GUI test.** Windows and macOS normally have no `DISPLAY`
  **(unverified)**, so there git only tries `tortoisemerge` (found when TortoiseGit is
  installed: its installer appends `…\TortoiseGit\bin` to the system `PATH`,
  [TGIT `StructureFragment.wxi` L379](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseGitSetup/StructureFragment.wxi#L379))
  and then vim, which Git for Windows ships **(derived)**. `vscode` and `winmerge` are never
  guessed.
- A guessed tool always prompts (§3), so **a headless run with nothing configured always
  fails**, and leaves temp files when a tool was found **(tested:** no tool on `PATH`: exit 1,
  "No known merge tool is available."; `kdiff3` on `PATH` and `DISPLAY` set: exit 1 at "Hit
  return", temps left**)**.

**`--tool-help`** lists the tools found and the ones not found, and the user's
`mergetool.*.cmd` names. It took 0.3 s **(tested)**. It is porcelain: 2.34 prints one name per
line, 2.37+ adds a description after it
([commit `980145f7`](https://github.com/git/git/commit/980145f747)). The user-defined lines come
out as `mine.cmd true` (name, `.cmd` and the value) because of the script's `IFS` **(tested** on
both**)**. The tools without "(requires a graphical session)" in 2.37+'s list are the terminal
ones: `emerge`, `vimdiff*`, `nvimdiff*`. It gives names, not command lines.

## 5. The common tools

| Tool | Built into git | git's three-way command (`mergetools/<name>`) | Trusted exit code | Blocks until closed? | Exit codes |
|---|---|---|---|---|---|
| WinMerge | `winmerge` | `WinMergeU.exe -u -e -dl Local -dr Remote "$LOCAL" "$REMOTE" "$MERGED"` (no BASE) | no | yes by default (below) | 0, unless `/enableexitcode` |
| Meld | `meld` | `meld [--auto-merge] --output="$MERGED" "$LOCAL" "$BASE" "$REMOTE"` (`--output` detected from `meld --help`) | no | yes, also when Meld is already running | 0 |
| KDiff3 | `kdiff3` | `kdiff3 --auto --L1 … --L2 … --L3 … -o "$MERGED" "$BASE" "$LOCAL" "$REMOTE"` | **yes** | yes | 0 if saved, 1 if not |
| Beyond Compare | `bc`, `bc3`, `bc4` | `bcomp "$LOCAL" "$REMOTE" "$BASE" -mergeoutput="$MERGED"` (`bcompare` if no `bcomp`) | no | `bcomp`/`BComp.com` yes; `BCompare` no | 0 success, 101 "conflicts detected, merge output not written", 100+ errors |
| P4Merge | `p4merge` | `p4merge "$BASE" "$REMOTE" "$LOCAL" "$MERGED"` (a virtual base when none) | no | macOS: only via `launchp4merge` **(unverified)** | not documented |
| VS Code | `vscode` **since 2.47** | `code --wait --merge "$REMOTE" "$LOCAL" "$BASE" "$MERGED"` | no | with `--wait` | not documented |
| TortoiseGitMerge | `tortoisemerge` | `tortoisegitmerge -base "$BASE" -mine "$LOCAL" -theirs "$REMOTE" -merged "$MERGED"`; fails without a base | no | yes **(unverified)** | 1 if conflicts remain on close, else 0 |
| IntelliJ IDEA | no: a custom `cmd` | JetBrains: `idea64.exe merge "$LOCAL" "$REMOTE" "$BASE" "$MERGED"`, `trustExitCode = true` | (user's) | **(unverified)** | not documented |

Sources: git's scripts
([GIT `mergetools/`](https://github.com/git/git/tree/8103b446517e0c44e67561b9d0ccce56efa60a71/mergetools));
VS Code added in [commit `6b77283f`](https://github.com/git/git/commit/6b77283f5e), v2.47.0.

- **WinMerge.** git finds it with `mergetool_find_win32_cmd "WinMergeU.exe" "WinMerge"`: first
  `WinMergeU.exe` on `PATH`, then `WinMerge\WinMergeU.exe` under each of `%ProgramFiles%`,
  `%ProgramFiles(x86)%` and `%ProgramW6432%`, else the bare name, which then fails
  ([GIT `git-mergetool--lib.sh` L526-549](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-mergetool--lib.sh#L526-L549);
  [`mergetools/winmerge`](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/mergetools/winmerge);
  identical in GFW and GIT234). WinMerge's installer goes to `{autopf}\WinMerge`, which is
  `C:\Program Files\WinMerge` when installed for all users, but **`{userpf}` (the user's
  `%LOCALAPPDATA%\Programs`) when installed for the current user only**; "Add to PATH" is an
  unticked option
  ([WINMERGE `WinMergeX64.is6.iss` L72, L85, L326](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Installer/InnoSetup/WinMergeX64.is6.iss#L72-L85);
  [`WinMergeX64NonAdmin.iss` L82](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Installer/InnoSetup/WinMergeX64NonAdmin.iss#L82);
  [Inno Setup constants](https://jrsoftware.org/ishelp/topic_consts.htm)). So `merge.tool=winmerge`
  fails for a per-user install unless `mergetool.winmerge.path` is set **(derived)**. Both
  installers register `App Paths\WinMergeU.exe` and `Software\Thingamahoochie\WinMerge\Executable`
  (HKLM or HKCU), which is where parterre could find it
  ([L750-756](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Installer/InnoSetup/WinMergeX64.is6.iss#L750-L756)).
  With three paths WinMerge compares three files, left, middle and right
  ([manual, Command line](https://manual.winmerge.org/en/Command_line.html)), so git's command
  shows LOCAL, REMOTE and the file with markers, and no BASE **(derived)**.
  **Blocking:** the "single instance" option defaults to 0, a new process per call that runs until
  its window closes. At 1, a second call hands its command line to the running WinMerge and
  **returns at once**, which git reads as "unchanged". At 2 it hands over and waits for that
  instance to exit
  ([WINMERGE `Src/Merge.cpp` L355-372](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Src/Merge.cpp#L355-L372);
  [`Src/OptionsInit.cpp` L136](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Src/OptionsInit.cpp#L136)).
  The exit code is 0 unless `/enableexitcode`
  ([`Merge.cpp` L694](https://github.com/WinMerge/winmerge/blob/004e5ab410f6631a37feeb1e58e3c03b62671f78/Src/Merge.cpp#L694)).
- **Meld** is a single-instance GApplication. A call while Meld runs opens a new window in the
  running Meld, and the call is held until that comparison tab closes, then returns the tab's
  status, which is always 0
  ([MELD `meldapp.py` L338-348, L449-457](https://gitlab.gnome.org/GNOME/meld/-/blob/e0931d117c187fc7ef14f38932e86aba4a00accb/meld/meldapp.py#L338-L348);
  [`melddoc.py` L145](https://gitlab.gnome.org/GNOME/meld/-/blob/e0931d117c187fc7ef14f38932e86aba4a00accb/meld/melddoc.py#L145)).
  `mergetool.meld.useAutoMerge` (`--auto-merge`) and `mergetool.meld.hasOutput` are git config
  ([`mergetool.meld.*`](https://git-scm.com/docs/git-config#Documentation/git-config.txt-mergetoolmeldhasOutput)).
- **KDiff3** exits 0 when the output was saved and 1 otherwise
  ([KDIFF3 `kdiff3_shell.cpp` L58-70](https://invent.kde.org/sdk/kdiff3/-/blob/65c4c35a715e868fade6cc288a09fc70d9bd86f6/src/kdiff3_shell.cpp#L58-L70);
  [`kdiff3.cpp` L1223-1230](https://invent.kde.org/sdk/kdiff3/-/blob/65c4c35a715e868fade6cc288a09fc70d9bd86f6/src/kdiff3.cpp#L1223-L1230)).
  git passes `--auto`: "No GUI if all conflicts are auto-solvable"
  ([KDiff3 handbook](https://docs.kde.org/stable_kf6/en/kdiff3/kdiff3/documentation.html)), so
  the window may never appear and the file is resolved at once **(derived)**.
- **Beyond Compare:** `bcomp` (macOS, Linux) and `BComp.com` "wait for the comparison to
  complete"; `BComp.exe` doesn't wait when started from a console; `BCompare` is a singleton
  that hands off and exits at once. Three-way merge needs the Pro edition
  ([Command Line Reference](https://www.scootersoftware.com/v5help/command_line_reference.html);
  [Using BC with version control](https://www.scootersoftware.com/kb/vcs)). Scooter's Windows
  instructions are just `merge.tool bc`, which implies `bcomp` is on `PATH` there
  **(unverified)**.
- **P4Merge** takes base, theirs, yours, result
  ([P4MERGE](https://help.perforce.com/helix-core/server-apps/cmdref/2025.1/Content/CmdRef/P4MERGE.html)).
  On macOS Perforce says to run `p4merge.app/Contents/Resources/launchp4merge`
  ([Run P4V from the command line](https://help.perforce.com/helix-core/server-apps/p4v/2026.1/Content/P4V/run-p4v-from-command-line.html)),
  which users put in `mergetool.p4merge.path`.
- **VS Code:** `--wait` "Wait for the files to be closed before returning"; `--merge <path1>
  <path2> <base> <result>` "Perform a three-way merge"
  ([command line](https://code.visualstudio.com/docs/configure/command-line)). For git before
  2.47 VS Code's docs give `mergetool.vscode.cmd 'code --wait --merge "$REMOTE" "$LOCAL" "$BASE"
  "$MERGED"'`, the same command as git's built-in. Its *Complete Merge* button "stages that file
  and closes the merge editor"
  ([Resolve merge conflicts](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts)).
- **TortoiseGitMerge** exits 1 when the user closes it with conflicts left, else 0
  ([TGIT `TortoiseMerge.cpp` L428-444](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseMerge/TortoiseMerge.cpp#L428-L444)),
  but git doesn't trust it unless the user sets `trustExitCode`. It refuses a conflict with no
  base (both sides added the file)
  ([`mergetools/tortoisemerge`](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/mergetools/tortoisemerge)).
  Its own switches are `/base:`, `/mine:`, `/theirs:`, `/merged:`
  ([Automating TortoiseGitMerge](https://tortoisegit.org/docs/tortoisegitmerge/tme-automation.html)).
- **IntelliJ IDEA** (and the other JetBrains IDEs): `idea64.exe` / `idea` / `idea.sh` `merge
  <path1> <path2> [<base>] <output>`. JetBrains' git snippet sets `trustExitCode = true` and
  `keepBackup = false`
  ([command-line merge tool](https://www.jetbrains.com/help/idea/command-line-merge-tool.html);
  [tutorial](https://www.jetbrains.com/help/idea/tutorial-use-idea-as-default-command-line-merge-tool.html)).
  Whether the launcher waits when the IDE is already running isn't documented.
- **Terminal tools** (`vimdiff*`, `nvimdiff*`, `emerge` without a display) need a terminal.
  #139 found a terminal vim under `CREATE_NO_WINDOW` waits forever **(tested there)**. Not run
  here: no vim on the test machine.

**Platform notes (derived/unverified):**
- macOS apps started from Finder or the Dock get a short `PATH` without `/usr/local/bin` or
  `/opt/homebrew/bin`, so `code`, `meld` or `bcomp` there may be "not available" unless
  `mergetool.<name>.path` is absolute **(unverified)**.
- On Windows a GUI tool started from a background process may open behind parterre's window;
  `AllowSetForegroundWindow(ASFW_ANY)` before starting it lets it come to the front
  ([AllowSetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-allowsetforegroundwindow);
  **unverified** for this case).

## 6. Launching the tool from parterre instead

What `git mergetool` does that parterre would have to repeat:

- write the stages (`git checkout-index --temp --stage=1|2|3 -- <file>` prints the temp name;
  tested), keep the backup, run `git merge-file --ours/--theirs` for `hideResolved`;
- pick the tool (§4) and its path, including `mergetool_find_win32_cmd` and Meld's `--help`
  probing;
- **the command line of each built-in tool**, 30 scripts and their variants, which change between
  versions (vimdiff's layout engine in 2.37, `vscode` in 2.47). There is no plumbing that prints
  them. `--tool-help` gives names only. The only way to reuse git's would be to source the
  internal `git-mergetool--lib` from `sh`, which is not an interface;
- a custom `cmd` is shell text and needs `sh` with the variables set (Git for Windows has `sh`);
- `trustExitCode` and the mtime rule, `keepBackup`, `writeToTemp`, then `git add`.

git-gui does exactly this, and shows the cost
([GIT `git-gui/lib/mergetool.tcl`](https://github.com/git/git/blob/8103b446517e0c44e67561b9d0ccce56efa60a71/git-gui/lib/mergetool.tcl)).
It has its own table of command lines (no `vscode`, no `merge.guitool`), uses **meld** when
`merge.tool` is unset, splits `mergetool.<name>.cmd` as a Tcl list (and rejects `[` `]`), trusts
every exit code, keeps a `.orig` only when `mergetool.keepBackup` is explicitly true, and doesn't
stage. It refuses deletion and symlink conflicts ("Cannot resolve deletion or link conflicts using
a tool"). Each of those differs from `git mergetool`.

## 7. For parterre (derived)

**Run `git mergetool` per file, not the tool.** It is git's own definition of "the user's merge
tool" (the map's decision), it covers every built-in and custom tool on 2.34 to today, and every
headless failure found here can be avoided by checking before the run. The cost is the
checks below and cleaning up after a cancel.

Before *Open in merge tool*:

1. **The conflict is a content conflict**: stages 2 and 3 both present, neither mode `120000`
   (symlink) nor `160000` (submodule), the same test git-gui makes. Otherwise git would prompt;
   offer *Use mine* / *Use theirs* (and *Delete* for deleted against modified) instead, which is
   what git's prompts offer. Stage 1 may be missing (both added); TortoiseGitMerge can't do that
   one.
2. **A tool is configured** (§4): `merge.guitool` or `merge.tool`, valid. Otherwise don't run
   (it would guess, prompt and leave temp files); go to *No merge tool configured*.
3. **It isn't a terminal tool** (`vimdiff*`, `nvimdiff*`, `emerge`): those need a terminal, so
   offer *Open in terminal* or say so.

The run:

- `git mergetool --no-prompt --gui -- <path>` with the runner's environment (stdin null, own
  process group, `GIT_TERMINAL_PROMPT=0`). `--no-prompt` beats a `mergetool.prompt=true` in the
  user's config. `--gui` makes `merge.guitool` count where the user has one, with the fall-back to
  `merge.tool`, on every version from 2.22, so `mergetool.guiDefault` (2.41) isn't needed.
  Passing `--tool=<name>` instead names the tool exactly and never guesses. Either is one line;
  which to use is a decision.
- **One run at a time per worktree**, and no parterre operation that writes the index while it
  runs (`index.lock`, §1 step 7).
- **Afterwards, re-read the index; the exit code is a hint.** Exit 0 with the path still unmerged
  means `git add` failed. Exit 0 and staged with conflict markers in it means the user saved
  without resolving (or a trusted tool exited 0); that is where *Mark as resolved*'s marker
  warning belongs too. `git checkout -m -- <path>` recreates the conflict from the resolve-undo
  record (tested on 2.34.1 and 2.43; it rewrites the file with fresh markers, so it undoes the
  user's edit). Exit 1: show git's output (`merge of <file> failed`, `… seems unchanged`, `The
  merge tool … is not available as …`).
- `<file>.orig` appears after every success while `mergetool.keepBackup` is unset (default true).
  That is the user's git config; parterre could pass `-c mergetool.keepBackup=false` but then it
  isn't "as `git mergetool` uses them". A decision.

**Watching and closing:**

- The run takes as long as the tool's window is open; parterre's runner must not time out.
- **Don't kill a run to cancel it.** It leaves four temp files in the worktree and, if only git
  dies, an orphaned tool window. If parterre does kill it (`SIGTERM` to the group), it should
  remove `<name>_{BACKUP,BASE,LOCAL,REMOTE}_<pid><ext>` beside the file, which it can only find
  by pattern.
- **Closing parterre while a tool is open:** the tool and `git mergetool` keep running. Success
  still stages the file (tested with the output pipe's reader gone). But a failure, or the
  "seems unchanged" question, writes a message first, dies of `SIGPIPE` on the closed pipe, and
  leaves the four temp files in the worktree **(tested)**. So either keep the window until the run ends, or give the run a
  log file rather than pipes.
- A tool that returns at once (WinMerge in single-instance mode 1, `BCompare` rather than
  `bcomp`, `code` without `--wait`) looks like "unchanged", or, if trusted, stages the unresolved
  file. Parterre can't tell; the index check above catches the second case.

**No merge tool configured** (the map's fog item):

- git's own guess is useless headless (§4.1): it always prompts, and on Windows and macOS it only
  looks for TortoiseGitMerge and vim.
- Parterre can detect installed tools itself: `git mergetool --tool-help` (0.3 s, names under
  "may be set to one of the following", porcelain), plus Windows' `App Paths` for WinMerge
  per-user installs, which git's lookup misses.
- Then offer to run one with `--tool=<name>` (and, for WinMerge, `-c
  mergetool.winmerge.path=<found>`), and/or to set `merge.tool` in the user's global config.
  Writing the user's config is a step the map hasn't decided.
- *Use mine* / *Use theirs* and editing the file in place plus *Mark as resolved* need no tool.
