# How git clients let the user finish a stuck worktree

Research note for [#270](https://github.com/aquamoth/parterre/issues/270): *how established git
clients let the user finish a **stuck worktree**: how the state shows, the conflicted-file list,
Continue / Skip / Abort, the merge tool, stops without conflicts, and a conflicted autostash.*
Part of [Finishing a stuck worktree](https://github.com/aquamoth/parterre/issues/267). It feeds
the prototype, [No merge tool configured](https://github.com/aquamoth/parterre/issues/274) and,
later, [A conflict resolver of parterre's own](https://github.com/aquamoth/parterre/issues/256).

It builds on, and doesn't repeat:

- [`git-stops.md`](https://github.com/aquamoth/parterre/blob/research/git-stops/docs/research/git-stops.md)
  (#268, "STOPS" below): what git leaves on disk at each stop, per-file ours/theirs, what
  `--continue` and each `--abort` need, autostash.
- [`external-merge-tools.md`](https://github.com/aquamoth/parterre/blob/research/external-merge-tools/docs/research/external-merge-tools.md)
  (#269, "TOOLS" below): `git mergetool` without a terminal, git's tool guess, `trustExitCode`.
- [`tortoisegit-revision-graph.md`](tortoisegit-revision-graph.md): TortoiseGit's graph, not its
  conflict handling.

Sources are the clients' source at pinned commits where it is open (TortoiseGit, Git Extensions,
VS Code), and official manuals, help pages, changelogs and issue trackers where it isn't (Fork,
Sourcetree, SmartGit, GitKraken, Sublime Merge). All were read on 2026-10-04. No client was run.
Marks:

- **(derived)**: my own conclusion, from code or docs.
- **(unverified)**: I could not check it.
- **[maint]** / **[staff]**: a reply by the vendor's maintainer or staff on its own tracker or
  forum, not a manual.
- **(secondary)**: a user report or a third party, kept only to point at a gap.
- **(binary)**: a string read from the shipped program, not shown in any doc; it says the text
  exists, not where it appears.

## TL;DR

1. **The state shows as a banner above the file list** in most clients: Git Extensions, Fork,
   SmartGit, GitKraken (a panel header). TortoiseGit has none: a rebase lives in its own Rebase
   dialog, a merge shows only as an icon in the Commit dialog. VS Code has no banner: a
   "Merge Changes" group, a *Continue* commit button and `(Rebasing)` in the status bar.
   Sourcetree pops up a dialog. Only TortoiseGit, Fork and SmartGit show **n/m** for a rebase
   (§1).
2. **Cherry-pick and revert are second-class almost everywhere.** Git Extensions never reads
   `CHERRY_PICK_HEAD` or `REVERT_HEAD`; VS Code detects cherry-pick but not revert; TortoiseGit
   handles cherry-pick only inside its own Rebase dialog; Sourcetree got `cherry-pick --continue`
   in June 2026. SmartGit (banner for all four) and Fork are the exceptions (§1, §3).
3. **The conflicted-file list is the ordinary file list** with a conflict mark, except
   TortoiseGit's *Resolve* dialog and Git Extensions' *Resolve merge conflicts* dialog. Only
   VS Code and SmartGit show git's conflict kind on the row ("Both Modified", "Deleted By Us");
   the others show it once a file is selected, or not at all (§2).
4. **Per-file actions are the same everywhere:** open in the merge tool, take one side, mark
   resolved, and a separate question for deleted-against-modified ("keep modified" / "delete").
   *Mark resolved* is `git add` in every client; Fork, Sublime Merge and VS Code call it just
   *Stage* (§2).
5. **Mine/theirs labels during a rebase are each client's own fix.** The trend is to drop the
   words *ours*/*theirs* for **branch or ref names** (TortoiseGit, Fork since 2021, GitKraken
   "Take current (branch)"). Git Extensions keeps both: "Choose local/current (theirs)" with a
   tooltip. SmartGit swaps panes, VS Code's merge editor swaps panes but not labels, Sourcetree
   fixed its reversed labels only in 2023 (§2.3).
6. **Continue is enabled only once nothing is unmerged** in VS Code, Git Extensions and SmartGit;
   TortoiseGit leaves it enabled and refuses with "One or more files are in a conflicted state."
   **Skip** is rare and never says what it drops: Git Extensions and SmartGit have a button,
   Sublime Merge labels *Continue* "(skip)" while it would skip, TortoiseGit offers it only in
   prompts, and Sourcetree and SmartGit run `--skip` themselves when *Continue* would fail. **Abort**
   asks first only in TortoiseGit and Fork; Git Extensions, VS Code and Sourcetree abort in one
   click (§3).
7. **No merge tool configured** (§4.2), for #274:
   - **Own setting, built-in fallback, git config untouched:** TortoiseGit (registry, default
     TortoiseGitMerge), Fork (own preferences, built-in merger), SmartGit (own preferences,
     built-in Conflict Solver), Sublime Merge and VS Code (built-in only).
   - **GitKraken:** own setting, default `<None>` (built-in merge tool), with a
     **"Git Config Default"** entry that uses git's configured tool.
   - **Writes git config:** **Git Extensions** writes `merge.guitool` + `mergetool.<name>.path`
     and `.cmd` at the scope picked in its settings page (repository or global), offers a list of
     known tools whose paths it searches, and with none says "There is no mergetool configured.
     Please go to settings and set a mergetool!". **Sourcetree** writes `merge.tool = sourcetree`,
     `mergetool.sourcetree.cmd` and `trustExitCode = true` into the **global** `~/.gitconfig`
     only, behind an "Allow Sourcetree to modify your global Git … config files" option, and
     never reads them back; with none, *Launch External Merge Tool* is greyed out.
   - Nobody offers detected tools at the moment of conflict; detection, where it exists, is in
     the settings page (Git Extensions, Fork on Windows).
8. **Waiting and auto-marking:** clients that wait for an external tool stage the file when it
   exits 0 (Git Extensions also requires a changed mtime; Sourcetree relies on git's
   `trustExitCode`) and otherwise ask "Was the merge successful?" (TortoiseGit) / "Is the merge
   conflict solved?" (Git Extensions). Fork stopped auto-staging: "no quick and reliable way to
   determine if a conflict is resolved" [maint] (§4.3).
9. **Warning about leftover conflict markers is not common.** VS Code (modal on *Stage*), SmartGit
   (on *Stage*, since 22.1) and Fork (reads the file for `<<<<<<<`) check the text. TortoiseGit's
   *Resolved*, Git Extensions and Sourcetree don't; Sourcetree closed it Won't Fix. Built-in
   mergers (TortoiseGitMerge, SmartGit, Sublime Merge, VS Code's merge editor) warn on save or
   close about their own unresolved blocks (§4.4).
10. **Stops without conflicts get no wording of their own**, except TortoiseGit and Fork: an
    `edit` stop turns *Continue* into **Amend** (TortoiseGit) or adds an *Amend* checkbox (Fork).
    An empty pick is offered *Skip* (TortoiseGit, Sublime Merge), skipped on *Continue*
    (Sourcetree, SmartGit), or left with *Abort* only (Fork). A refused hook is shown as git's
    error everywhere (§5).
11. **A conflicted autostash gets no recovery in any client.** TortoiseGit says "Stash POP failed,
    there are conflicts" and offers to show changes; VS Code says "There are merge conflicts while
    applying the stash"; the rest document nothing. Git keeps the entry; no client says so (§6).

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| TG | TortoiseGit `master` @ `7338078f` (2026-09-27); latest release `REL_2.19.1.0_EXTERNAL` | https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |
| TGDOC | TortoiseGit manual (current) | https://tortoisegit.org/docs/tortoisegit/ |
| GE | Git Extensions `v7.2.1` @ `0aea2d6d` | https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/ |
| GEDOC | Git Extensions manual source @ `62b6a6e4`, rendered at git-extensions-documentation.readthedocs.io | https://github.com/gitextensions/GitExtensionsDoc/blob/62b6a6e40b8d0f4b58cc10f4ad440618fc488aa7/ |
| VSC | VS Code `1.140.0` @ `07f806f9` (`extensions/git`, `extensions/merge-conflict`, `src/vs/workbench/contrib/mergeEditor`) | https://github.com/microsoft/vscode/blob/07f806f999227108933c2e30515b26eecc1fda74/ |
| VSCDOC | VS Code docs @ `35e82eee` (DateApproved 2026-09-30) | https://github.com/microsoft/vscode-docs/blob/35e82eeecd596e95917037b296b1c83fb4a6be64/ |
| FORK | Fork release notes (Mac, Windows) and official trackers `fork-dev/Tracker`, `fork-dev/TrackerWin` | https://fork.dev/releasenotes, https://fork.dev/releasenoteswin |
| ST | Sourcetree release notes (Win 3.4.32, Mac 4.2.19), public Jira SRCTREE / SRCTREEWIN, Atlassian Community | https://product-downloads.atlassian.com/software/sourcetree/windows/ga/ReleaseNotes_3.4.32.html, https://product-downloads.atlassian.com/software/sourcetree/ReleaseNotes/Sourcetree_4.2.19.html |
| SG | SmartGit manual (Latest, source `syntevo/docs` `latest` 2026-10-01) and changelogs 17.1 to 26.1 (26.1.056, 2026-09-30) | https://docs.syntevo.com/SmartGit/Latest/, https://www.syntevo.com/smartgit/changelog-26.1.txt |
| GK | GitKraken Desktop help (branching-and-merging Mar 2026, preferences Sep 2026) and release notes | https://help.gitkraken.com/gitkraken-desktop/ |
| SM | Sublime Merge docs, changelog (stable Build 2132, dev), default menus in Build 2132's `Default.sublime-package` | https://www.sublimemerge.com/docs/, https://www.sublimemerge.com/download |

The VS Code links below use the tag name `1.140.0` in place of the commit for readability; the tag
points at `07f806f9`.

Two things to know before reading the TortoiseGit rows:

- **TortoiseGit's rebase is not `git rebase`.** Its Rebase dialog runs its own loop of `checkout`
  and `cherry-pick` per commit, keeps its state in the dialog and a lock folder
  `.git/tgitrebase.active/` ([TG `RebaseDlg.cpp` L2270-2281](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L2270-L2281),
  [L2453-2463](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L2453-L2463)),
  and never runs `--continue`, `--abort` or `--skip` except for `git am`
  ([TG `ImportPatchDlg.cpp` L391-394](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/ImportPatchDlg.cpp#L391-L394)).
  Closing the dialog aborts; there is no resume
  ([L2579-2582](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L2579-L2582)).
- **It doesn't recognise a rebase git started.** `IsRebaseActive()` checks only `rebase-apply`
  and `tgitrebase.active`; `rebase-merge` appears nowhere in the source
  ([TG `TGitPath.cpp` L857-866](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/TGitPath.cpp#L857-L866)).
  So a rebase from the command line gets no rebase handling at all **(derived)**.

So TortoiseGit is the reference for *how it looks*, not for driving git's own sequencer.

---

## 1. How the state shows

| Client | Where | Wording | n/m | Conflicts vs. none |
|---|---|---|---|---|
| TortoiseGit | Rebase dialog (rebase, and cherry-pick from the Log); merge: an icon in the Commit dialog, "Abort Merge" in menus | "Rebasing... (%1!d!/%2!d!)"; merge icon tooltip "A merge process is active, so this commit will be a merge commit…" | yes | main button "Commit" + "Conflict Files" tab, or "Amend" for `edit` |
| Git Extensions | banner above the revision grid | "{0} is currently in progress." / "…in progress with merge conflicts.", {0} = Rebase, Merge, Patch; else "There are unresolved merge conflicts." | no (a list of commits to re-apply) | orange + "Resolve..." vs. light blue + "Continue" |
| VS Code | "Merge Changes" group; commit button; status bar | button "Continue", tooltip "Continue Rebase" / "Continue Merge"; status bar `<head> (Rebasing)` | no | only whether "Merge Changes" is empty |
| Fork | banner across the top | "Rebasing branch '…' onto '…'. Rebased 0 of 2 commits: … Amending '3135023'."; "Cherry-picking commit '…'. Fix 1 conflict and then continue." | yes | "Fix n conflict(s)" in the text |
| Sourcetree | a dialog, a status-bar notice | "It appears that you have a rebase in progress that was probably interrupted because of a conflict…" | not documented | not documented |
| SmartGit | banner above the file list; repository icons | "The working tree is in **cherry-picking**-state." | yes (rebase, in the graph) | not told apart |
| GitKraken | Commit Panel header | "Merge conflicts detected", "Merging <branch> into <branch>" | not documented | not documented |
| Sublime Merge | the commit button becomes *Continue rebase* | no banner documented | not documented | "Continue rebase (skip)" until resolved **(secondary, binary)** |

### 1.1 Per client

- **TortoiseGit.**
  - *Merge:* Explorer hides Merge, Rebase, Pull and shows "Abort &Merge"
    ([TG `MenuInfo.cpp` L139-140](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseShell/MenuInfo.cpp#L139-L140)).
    The Commit dialog shows a merge icon with the tooltip "A merge process is active, so this
    commit will be a merge commit. In order to abort a merge, you have to perform a reset or
    forced checkout." ([TG `TortoiseProcENG.rc` L4052](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4052)).
    The Log's "Working tree changes" row gets "Abort Merge" and a conflict icon
    ([TG `GitLogListBase.cpp` L1760-1764](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/GitLogListBase.cpp#L1760-L1764)).
    Right after a conflicting merge or pull: "…After resolving all files, you need to perform a
    commit in order to complete the merge. If you want to abort the merge, do a hard reset on HEAD
    or select abort merge on the context menu." with *Resolve* and *Commit* buttons
    ([TG `AppUtils.cpp` L3292-3310](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L3292-L3310)).
  - *Cherry-pick or revert outside the Rebase dialog:* no notice. The only trace is the label of
    the other side: `MERGE_HEAD (branch, abc1234)`, `CHERRY_PICK_HEAD (abc1234)`,
    `Parent of abc1234`, else "changes to-be-integrated"
    ([TG `AppUtils.cpp` L1727-1752](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1727-L1752)).
  - *Rebase dialog:* "Rebasing... (%1!d!/%2!d!)" with a progress bar and taskbar progress, also in
    cherry-pick mode ([TG `TortoiseProcENG.rc` L4803](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4803),
    [`RebaseDlg.cpp` L1917-1945](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L1917-L1945)).
    The commit list shows the current commit bold and done or skipped ones grey
    ([TG `GitLogListBase.cpp` L1084-1095](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/GitLogListBase.cpp#L1084-L1095)).
    The kind of stop sets the main button, the tab and the taskbar colour
    ([`RebaseDlg.cpp` L1797-1838](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L1797-L1838)):

    | Stop | Main button | Tab | Taskbar |
    |---|---|---|---|
    | conflict | "Commit" | "Conflict Files" | red |
    | `edit` | "Amend", plus an "Edit/Split commit" checkbox | "Commit Message" | paused |
    | squash message | "Commit" | message | paused |
    | error | "Continue", disabled | Log | |

- **Git Extensions.** A banner above the graph, refreshed when the window is activated
  ([GE `FormBrowse.cs` L1175-1183](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormBrowse.cs#L1175-L1183)).
  Text "{0} is currently in progress." or "…in progress with merge conflicts.", {0} being Rebase,
  Merge or Patch; with none of those but unmerged files, "There are unresolved merge conflicts."
  Orange with a merge icon when conflicted, light blue otherwise; the first button is
  "Resolve..." or "Continue", then "Abort" and, for rebase and patch, "More..."
  ([GE `InteractiveGitActionControl.cs` L13-20, L130-171](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/UserControls/InteractiveGitActionControl.cs#L13-L171)).
  It knows only rebase, merge (`MERGE_HEAD`), `am` and bisect; a stopped cherry-pick or revert
  shows only while files are unmerged, as the generic text, and not at all after
  ([GE `GitModule.cs` L1968-1991](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitCommands/Git/GitModule.cs#L1968-L1991)).
  After a merge, cherry-pick, revert, pull or stash apply with conflicts it asks "There are
  unresolved merge conflicts, solve conflicts now?"
  ([GE `MergeConflictHandler.cs` L7-48](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/MergeConflictHandler.cs#L7-L48)).
  The Rebase dialog has no counter, but a "Commits to re-apply:" grid from `done` and
  `git-rebase-todo` with status "Applied", "Applying...", "Skipped"
  ([GE `PatchGrid.cs` L82-160](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/UserControls/PatchGrid.cs#L82-L160),
  [`PatchFile.cs` L26-52](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/UserControls/PatchFile.cs#L26-L52)).
  The manual still places the warning in the status bar
  ([GEDOC `modify_history.rst` L164-167](https://github.com/gitextensions/GitExtensionsDoc/blob/62b6a6e40b8d0f4b58cc10f4ad440618fc488aa7/source/modify_history.rst#L164-L167)),
  but its screenshot shows the banner.
- **VS Code.** Reads `.git/rebase-merge|rebase-apply` + `REBASE_HEAD`, `MERGE_HEAD` and
  `CHERRY_PICK_HEAD`; never `REVERT_HEAD`, never `msgnum`/`end`
  ([VSC `repository.ts` L3263-3291](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L3263-L3291)).
  "Merge Changes" is the first group
  ([L1018-1021](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L1018-L1021)).
  During a rebase or merge the commit button is "$(check) Continue", tooltip "Continue Rebase" /
  "Continue Merge"; cherry-pick keeps the plain Commit button
  ([VSC `actionButton.ts` L143-188](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/actionButton.ts#L143-L188),
  [L282-291](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/actionButton.ts#L282-L291)).
  The status bar reads `<head> (Rebasing)`
  ([VSC `statusbar.ts` L51-52](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/statusbar.ts#L51-L52)).
  It builds the paths as `<root>/.git/…`, so in a linked worktree (`.git` is a file) and with
  reftable (`REBASE_HEAD`, `CHERRY_PICK_HEAD` aren't files) it should miss the state
  **(derived, unverified)**.
- **Fork.** A banner with progress and *Abort* / *Resolve* buttons: "Rebasing branch '…' onto '…'.
  Rebased 0 of 2 commits: [bar] Amending '3135023'." [maint]
  ([Tracker#895](https://github.com/fork-dev/Tracker/issues/895)); "Cherry-picking commit '…'.
  Fix 1 conflict and then continue." (secondary, user screenshot,
  [Tracker#2136](https://github.com/fork-dev/Tracker/issues/2136)). Windows shows "Cherry-pick in
  progress" ([TrackerWin#2651](https://github.com/fork-dev/TrackerWin/issues/2651)), but a
  Windows rebase conflict has also surfaced as a modal "Git Error" with git's output
  ([TrackerWin#1965](https://github.com/fork-dev/TrackerWin/issues/1965), secondary). Fork 2.70
  (Mac, 2026-09-04) / 2.23 (Win): "Detect cherry-pick and revert state with the reftable backend"
  ([FORK](https://fork.dev/releasenotes)). Fork also predicts conflicts before a merge, rebase,
  cherry-pick or revert, with `git merge-tree` [maint]
  ([TrackerWin#1992](https://github.com/fork-dev/TrackerWin/issues/1992)).
- **Sourcetree.** No official how-to exists. A "Merge Conflicts" pop-up sends the user to *Resolve
  Conflicts* ([SRCTREE-6881](https://jira.atlassian.com/browse/SRCTREE-6881)). On Windows a
  "Rebase In Progress" dialog: "It appears that you have a rebase in progress that was probably
  interrupted because of a conflict. If you've resolved the conflicts in your working copy, please
  choose whether to continue the rebase, or abort this process where you are now." with *Continue
  Rebase* / *Abort Rebase* / *Cancel* [staff screenshot]
  ([SRCTREEWIN-11385](https://jira.atlassian.com/browse/SRCTREEWIN-11385)). Mac 1.2.9: "Display
  outstanding rebases … more conspicuously in the status bar" ([ST Mac notes](https://product-downloads.atlassian.com/software/sourcetree/ReleaseNotes/Sourcetree_4.2.19.html));
  users still ask for a visible indicator ([SRCTREE-7828](https://jira.atlassian.com/browse/SRCTREE-7828)).
- **SmartGit.** States "Merging", "Rebase", "Cherry-Picking", "Reverting", "Bisecting"; SmartGit
  "will provide a visual alert … and will provide options on how to proceed"
  ([SG Working-Tree-States](https://docs.syntevo.com/SmartGit/Latest/Manual/GitConcepts/Working-Tree-States)).
  The banner reads "The working tree is in cherry-picking-state." with *Continue* and *Abort*
  ([screenshot](https://docs.syntevo.com/SmartGit/Latest/Manual/images/Working-Tree-Status.png)),
  since 19.1: "show banner for current rebasing/merging/cherry-pick/... state with state-specific
  commands what to do" ([changelog-19.1](https://www.syntevo.com/smartgit/changelog-19.1.txt)).
  22.1: "Graph: Rebase: shows number of already rebased + overall number of commits"
  ([changelog-22.1](https://www.syntevo.com/smartgit/changelog-22.1.txt)).
- **GitKraken.** "When conflicts occur, the Commit Panel shows the conflicted files"; the
  screenshot reads "Merge conflicts detected", "Merging <branch> into <branch>", "Conflicted
  Files (1)" and *Mark all resolved*
  ([GK branching-and-merging](https://help.gitkraken.com/gitkraken-desktop/branching-and-merging/),
  [screenshot](https://help.gitkraken.com/wp-content/uploads/merge-conflict@2x.png)). The rebase,
  cherry-pick and revert states are not documented; "If you start the rebase in GitKraken
  Desktop, you must complete it there"
  ([GK interactive-rebase](https://help.gitkraken.com/gitkraken-desktop/interactive-rebase/)).
- **Sublime Merge.** The docs only say to "Select the **Continue rebase** button"
  ([SM getting started](https://www.sublimemerge.com/docs/getting_started)); staff: "The commit
  button should convert to a "Continue rebase" button" [staff]
  ([forum](https://forum.sublimetext.com/t/rebase-continue-after-resolving-conflicts/39562)).
  Build 2059: "Restored missing Continue button when a cherry pick has been paused"
  ([SM changelog](https://www.sublimemerge.com/download)).

## 2. The conflicted-file list

| Client | Where | Kind on the row | Per-file actions | How |
|---|---|---|---|---|
| TortoiseGit | Resolve dialog; Commit, Rebase ("Conflict Files"), Sync ("Conflicts") lists; Explorer | no, "Conflict" only | Edit conflicts (default, double-click), Resolve conflict using "<ref>" ×2, Resolved | context menu; delete/modify and submodule get dialogs |
| Git Extensions | *Resolve merge conflicts* dialog | no; a sentence for the selected file | Merge / Open in mergetool, Choose local / remote / base, Mark conflict as solved, Open / Open With, Reset | buttons + context menu; task dialogs for delete/modify and binary |
| VS Code | "Merge Changes" group | yes: "Conflict: Both Modified", "Deleted By Us", … | open (file with markers, or merge editor), Stage, Accept Current / Incoming / Both (CodeLens) | row click, `+`, CodeLens; modal for delete/modify on stage |
| Fork | Unstaged list, warning icon | in the right pane ("modified" badge per side) | Merge in Fork, Merge in External Tool, Choose '<branch>', stage | pane buttons |
| Sourcetree | File Status, warning icon | not documented | Launch External Merge Tool, Resolve Using 'Mine' / 'Theirs', Mark Resolved, Mark Unresolved, Restart Merge | context menu *Resolve Conflicts* and *Actions* |
| SmartGit | Files view, sorted to the top | yes: "Conflicted (Both modified)", plus whether the file equals ours or theirs | Conflict Solver (default), Take Ours, Take Theirs, Recreate Conflict, Mark Resolved | menus + quick actions in the Changes view |
| GitKraken | Commit Panel, "Conflicted Files (n)" | not documented | Open in external merge tool, Take current (<branch>), Take incoming (<branch>), Mark resolved | context menu + per-row button; built-in merge tool on click |
| Sublime Merge | Pending changes, "Unmerged" section | not documented | Resolve (built-in tool), Resolve Using Ours / Theirs, Stage | button on the file header + dropdown |

### 2.1 What a row shows

- **TortoiseGit:** the status column says just "Conflict"; the columns are Path, Filename,
  Extension, Status, Lines added/removed, Last Modified, Size, LFS Lock Owner
  ([TG `GitStatusListCtrl.cpp` L303-311](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/GitStatusListCtrl.cpp#L303-L311),
  [`TGitPath.cpp` L1782-1785](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/TGitPath.cpp#L1782-L1785)).
  The kind appears only when the file is opened. The *Resolve* dialog is a checkbox list of
  conflicted files with "Reminder: Commit your change after resolve"; OK runs `git add -f` on the
  ticked files ([TG `TortoiseProcENG.rc` L800-811](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L800-L811),
  [`ResolveProgressCommand.cpp` L50-80](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/ProgressCommands/ResolveProgressCommand.cpp#L50-L80);
  [screenshot](https://tortoisegit.org/docs/tortoisegit/images/ResolveConflict.png)).
- **Git Extensions:** one column, "Filename". The selected file gets a sentence, e.g. "The file
  has been deleted locally ({0}) and modified remotely ({1}). Choose to delete the file or keep
  the modified version.", and Local / Base / Remote labels ("no base", "deleted")
  ([GE `FormResolveConflicts.cs` L44-56, L842-875](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L842-L875);
  [screenshot](https://git-extensions-documentation.readthedocs.io/en/main/_images/resolve_merge_conflicts.png)).
- **VS Code:** the letter is always `!`; the tooltip is the kind: "Conflict: Both Deleted",
  "Added By Us", "Deleted By Them", "Added By Them", "Deleted By Us", "Both Added", "Both
  Modified"; deleted kinds are struck through
  ([VSC `repository.ts` L83-141, L247-261](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L96-L120)).
- **SmartGit:** "Conflicted (Both modified)" in the Index State column
  ([screenshot](https://docs.syntevo.com/SmartGit/Latest/HowTos/Workflows/images/how-to-resolve-conflicts.png));
  21.1: "for conflicted files denote whether the current working tree state equals "ours" or
  "theirs" state" ([changelog-21.1](https://www.syntevo.com/smartgit/changelog-21.1.txt)).
- **Fork:** the right pane says "Merge Conflict — The file was changed both locally and remotely.
  Select the changes or merge them manually." with a card per side (branch, a kind badge such as
  "modified", last commit) and *Merge in Fork* / *Merge in External Tool*
  ([Tracker#2136](https://github.com/fork-dev/Tracker/issues/2136), secondary screenshot).

### 2.2 Per-file actions

- **Open in the merge tool** is the default action (double-click or Enter) in TortoiseGit
  ([TG `GitStatusListCtrl.cpp` L2953-2955](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/GitStatusListCtrl.cpp#L2953-L2955)),
  Git Extensions ([GE `FormResolveConflicts.cs` L524-527](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L524-L527))
  and SmartGit (22.1: "the Conflict Solver is shown as default command",
  [changelog-22.1](https://www.syntevo.com/smartgit/changelog-22.1.txt)). In VS Code a click
  opens the file with inline markers; the merge editor only with `git.mergeEditor`, which
  defaults to `false` ([VSC `package.json` L4047-4052](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/package.json#L4047-L4052),
  [`repository.ts` L533-555](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L533-L555)).
- **Take one side.** TortoiseGit `Resolve conflict using "%s"` with a ref name for `%s`
  ([TG `TortoiseProcENG.rc` L5181](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L5181));
  it runs `checkout-index --stage=2|3` + `add`, or `rm` when that side is deleted
  ([TG `ResolveProgressCommand.cpp` L91-175](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/ProgressCommands/ResolveProgressCommand.cpp#L91-L175)).
  Git Extensions "Choose local" Ctrl+1, "Choose remote" Ctrl+2, "Choose base" Ctrl+3:
  `checkout-index -f --stage=N` ([GE `FormResolveConflicts.cs` L877-956](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L877-L956)).
  Sourcetree "Resolve Using 'Mine'" = "the file exactly as the first parent", 'Theirs' = "the
  *whole file* from the second parent… discarding all changes from the other side (even
  non-conflicting ones)" [staff] ([Community](https://community.atlassian.com/forums/Sourcetree-questions/Merge-conflicts-resolve-using-theirs-vs-mine/qaq-p/204061)).
  GitKraken "Take current (branch)" / "Take incoming (branch)" (8.5.0,
  [GK 8.x notes](https://help.gitkraken.com/gitkraken-desktop/8x/),
  [screenshot](https://help.gitkraken.com/wp-content/uploads/current-incoming.png)). Sublime Merge
  Build 2020: "Added Resolve Ours / Resolve Theirs dropdown to unmerged files"
  ([SM changelog](https://www.sublimemerge.com/download)). Fork: untick one side, the button
  becomes "Choose '<branch>'" [maint] ([Tracker#1183](https://github.com/fork-dev/Tracker/issues/1183)).
  VS Code has no whole-file *take ours/theirs* on the row
  ([VSC `package.json` L2436-2480](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/package.json#L2436-L2480));
  only per-block CodeLens "Accept Current Change" / "Accept Incoming Change" / "Accept Both
  Changes" and the palette's "Accept All Current/Incoming"
  ([VSC `codelensProvider.ts` L64-86](https://github.com/microsoft/vscode/blob/1.140.0/extensions/merge-conflict/src/codelensProvider.ts#L64-L86)).
- **Mark resolved** is `git add` everywhere: TortoiseGit "Resolved"
  ([rc L4363](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4363)),
  Git Extensions "Mark conflict as solved"
  ([GE `FormResolveConflicts.cs` L1411-1422](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L1411-L1422)),
  Sourcetree "Mark Resolved" [staff] ([SRCTREEWIN-525](https://jira.atlassian.com/browse/SRCTREEWIN-525)),
  SmartGit "Mark Resolved" (22.1 "replaced "Stage" by "Mark Resolved"",
  [changelog-22.1](https://www.syntevo.com/smartgit/changelog-22.1.txt)), GitKraken "Mark
  resolved". Fork has no button: "Just stage the resolved files and carry on" [maint]
  ([TrackerWin#110](https://github.com/fork-dev/TrackerWin/issues/110)); VS Code's is "Stage
  Changes" / "Stage All Merge Changes"
  ([VSC `package.nls.json` L23-27](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/package.nls.json#L23-L27));
  Sublime Merge's is "Save and stage" in its tool, or Stage
  ([SM getting started](https://www.sublimemerge.com/docs/getting_started)). TortoiseGit asks
  "Are you sure you want to mark the conflicted file(s) as resolved?" before any of its three
  ([TG `GitStatusListCtrl.cpp` L2468](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/GitStatusListCtrl.cpp#L2468)).
- **Deleted against modified** gets its own question in every client that documents it:
  - TortoiseGit: "Edit conflicts" opens a "Delete/modify merge conflict" dialog showing each
    side's ref and status (Deleted / Modified / Created) with *Modified* (`add`), *Delete* (`rm`)
    and *Abort* (closes)
    ([TG `DeleteConflictDlg.cpp` L67-87](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/DeleteConflictDlg.cpp#L67-L87),
    [screenshot](https://tortoisegit.org/docs/tortoisegit/images/resolve-delete-modify-conflict.png)).
    Submodules get a "Resolve Submodule Conflict" dialog with "Use this" per side
    ([TG `SubmoduleResolveConflictDlg.cpp` L223-253](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/SubmoduleResolveConflictDlg.cpp#L223-L253)).
  - Git Extensions: "Delete file", "Keep modified", "Keep base file", with "Apply to '{0}' and {1}
    other file(s)"; binary files get "Choose local (ours)", "Choose remote (theirs)", "Keep base
    file" ([GE `FormResolveConflicts.cs` L1020-1234](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L1100-L1234)).
  - VS Code, on *Stage*: "File "{0}" was deleted by them and modified by us. What would you like
    to do?" with "Keep Our Version" / "Delete File" (mirror for deleted by us)
    ([VSC `commands.ts` L1587-1617](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L1587-L1617)).
  - Fork: "This is a boolean question… We cannot merge `string` and `null`." [maint]
    ([Tracker#608](https://github.com/fork-dev/Tracker/issues/608)).
  - Sourcetree: *Launch External Merge Tool* hangs on git's "(m)odified or (d)eleted" prompt; the
    advice is the context-menu resolve ([SRCTREEWIN-6829](https://jira.atlassian.com/browse/SRCTREEWIN-6829)),
    the same failure TOOLS §3 found.
  - SmartGit, GitKraken, Sublime Merge: not documented beyond bug fixes.
- **Open in an editor:** Git Extensions "Open" / "Open With" hand the file to the OS
  ([GE `FormResolveConflicts.cs` L1472-1490](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L1472-L1490));
  VS Code opens it in place. Not documented for the others on a conflicted row.

### 2.3 Mine and theirs during a rebase

Git's *ours* is the onto side during a rebase (STOPS §2.3). What each client does:

| Client | Labels | During a rebase |
|---|---|---|
| TortoiseGit | ref names: `MERGE_HEAD (feature, 1a2b3c4)`, `HEAD`; its own rebase: head-name and onto, fallbacks "Branch being rebased" / "Branch being rebased onto" | swaps the menu actions and the merge tool's panes, so "Mine" is the commit being replayed; only for its own rebase |
| Git Extensions | "Choose local/current (ours)" / "Choose remote/incoming (theirs)" | "Choose local/current (theirs)", tooltip "Take only the changes from the branch you are rebasing onto"; "Choose remote/incoming (ours)", "…from the branch you are rebasing" |
| VS Code | merge editor "Current" / "Incoming"; CodeLens "Current Change" / "Incoming Change" | merge editor swaps panes, keeps labels; CodeLens knows nothing of rebase |
| Fork | branch names since Mac 2.13 / Win 1.67 ("show branch names instead of ours/theirs") | [maint]: "during rebase the ours/theirs order is opposite"; it guesses names from SHAs |
| Sourcetree | 'Mine' / 'Theirs' | not relabelled; a decade of "reversed" reports closed as fixed in Mac 4.2.6 (2023) |
| SmartGit | pane titles `main ("ours", 65f6f459)` / `Merge source ("theirs", …)` | swapped panes, now a low-level property, "not 100% consistent, e.g. Git writes ours/theirs in the conflict markers" |
| GitKraken | "Take current (<branch>)" / "Take incoming (<branch>)" | not documented |
| Sublime Merge | "Resolve Using Ours" / "Theirs"; tool "Ours" left, "Theirs" right | not documented |

Sources: TortoiseGit [`AppUtils.cpp` L1702-1752](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1702-L1752),
[L1900-1949](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L1900-L1949),
[`GitStatusListCtrl.cpp` L387, L2471-2474](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/GitStatusListCtrl.cpp#L2471-L2474)
(the manual still says *mine*/*theirs*: [tgit-dug-conflicts](https://tortoisegit.org/docs/tortoisegit/tgit-dug-conflicts.html));
Git Extensions [`FormResolveConflicts.cs` L70-84, L256-279](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L256-L279),
[GEDOC `modify_history.rst` L207-210](https://github.com/gitextensions/GitExtensionsDoc/blob/62b6a6e40b8d0f4b58cc10f4ad440618fc488aa7/source/modify_history.rst#L207-L210);
VS Code [`commands.ts` L857-920](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L857-L920),
[VSCDOC `merge-conflicts.md` L44-56](https://github.com/microsoft/vscode-docs/blob/35e82eeecd596e95917037b296b1c83fb4a6be64/docs/sourcecontrol/merge-conflicts.md#L44-L56)
(its toolbar's "Accept All Incoming Changes from Left" is wrong while rebasing:
[`mergeEditor/…/commands.ts` L515-545](https://github.com/microsoft/vscode/blob/1.140.0/src/vs/workbench/contrib/mergeEditor/browser/commands/commands.ts#L515-L545));
Fork [notes](https://fork.dev/releasenotes), [TrackerWin#239](https://github.com/fork-dev/TrackerWin/issues/239),
[Tracker#1659](https://github.com/fork-dev/Tracker/issues/1659);
Sourcetree [SRCTREE-1670](https://jira.atlassian.com/browse/SRCTREE-1670);
SmartGit [Rebase](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Branch/Rebase),
[changelog-23.1](https://www.syntevo.com/smartgit/changelog-23.1.txt),
[Conflict Solver screenshot](https://docs.syntevo.com/SmartGit/Latest/Manual/images/Tools-SmartGit-ConflictSolver.png);
Sublime Merge [getting started](https://www.sublimemerge.com/docs/getting_started).

Stash pop inverts the sides too: VS Code names the incoming side "Stashed Changes"
([VSC `commands.ts` L857-907](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L857-L907));
Fork and Sourcetree users report the inversion as confusing
([TrackerWin#239](https://github.com/fork-dev/TrackerWin/issues/239),
[SRCTREE-3238](https://jira.atlassian.com/browse/SRCTREE-3238)).

## 3. Continue, Skip and Abort

| Client | Continue | Skip | Abort |
|---|---|---|---|
| TortoiseGit | rebase dialog's main button ("Commit" / "Amend" / "Continue"), never disabled; merge: commit in the Commit dialog | no button; only in prompts for an empty or failed pick | rebase: "Abort", asks first; merge: "Abort Merge" dialog with reset type |
| Git Extensions | banner "Continue" or "&Continue rebase", shown only with nothing unmerged | "S&kip currently applying commit", no confirmation | banner / dialog "Abort", no confirmation; resolve dialog's "&Reset" = `reset --hard`, confirms twice |
| VS Code | commit button "Continue" (rebase, merge), enabled when "Merge Changes" is empty | none | palette "Abort Rebase", "Abort Merge", "Abort Cherry Pick", no confirmation |
| Fork | "Continue Rebase" in place of Commit | none documented | banner "Abort", confirms for merge |
| Sourcetree | "Continue Rebase" in the dialog and *Actions* | none; Continue runs `--skip` when nothing is left to commit | "Abort Rebase", no confirmation |
| SmartGit | banner / dialog "Continue rebase", disabled until resolved; *Branch \| Continue* for cherry-pick and revert; merge: *Commit* | "Skip current patch" (rebase), cherry-pick since 22.1 | *Branch \| Abort* / "Abort rebase" |
| GitKraken | not documented | not documented | not documented |
| Sublime Merge | "Continue rebase", "Continue Cherry Pick", "Complete revert" **(binary)** | "Continue rebase (skip)", "Continue cherry pick (skip)" **(binary)** | "Abort rebase"; "Abort merge", "Abort cherry pick", "Abort revert" **(binary)** |

- **TortoiseGit.** The Rebase dialog has two buttons, the multi-purpose one and "Abort" (tooltip
  "Recover to the status before rebase")
  ([TG `TortoiseProcENG.rc` L1527-1528, L4727](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4727)).
  "Commit" with files still unmerged says "One or more files are in a conflicted state." and
  selects the first one; it also warns if the message still has git's `# Conflicts:` lines
  ([TG `RebaseDlg.cpp` L1096-1124, L1376-1377](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L1096-L1124)).
  Abort asks "Are you sure you want to abort the rebase process?", then `reset --hard` back
  ([rc L4818](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4818),
  [`RebaseDlg.cpp` L2584-2700](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L2584-L2700)).
  The current and done commits can't be changed to Skip in the list
  ([TG `GitLogListBase.cpp` L1726-1738](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/GitLogListBase.cpp#L1726-L1738)).
  For a merge, committing with unmerged files warns "One or more files are in a conflicted
  state." with OK / Ignore ([TG `CommitDlg.cpp` L582-600](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/CommitDlg.cpp#L582-L600)).
  "Abort Merge" opens "In order to abort a merge progress a reset (to HEAD) is needed." with
  Reset Type "Merge" (`reset --merge`, the default), "Mixed", "Hard … (discard all local
  changes)" and "Show modified files in working tree"
  ([rc L2020-2037](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L2020-L2037),
  [`AppUtils.cpp` L3396-3402](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L3396-L3402)).
  A revert or a command-line cherry-pick has no Continue or Abort at all.
- **Git Extensions.** Banner *Continue* runs `rebase --continue`, `merge --continue` or
  `am --3way --resolved`; *Abort* runs the matching `--abort`, no confirmation
  ([GE `InteractiveGitActionControl.cs` L183-231](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/UserControls/InteractiveGitActionControl.cs#L183-L231)).
  Rebase dialog: "&Continue rebase" only when nothing is unmerged, else "&Solve conflicts";
  "S&kip currently applying commit" (`rebase --skip`, marks the row "Skipped", no prompt, says
  nothing of what is dropped); "A&bort", no prompt; "&Edit todo..."
  ([GE `FormRebase.cs` L149-315](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormRebase.cs#L149-L315)).
  When the last conflict is resolved the dialog closes and, outside a rebase, asks "All merge
  conflicts are resolved, you can commit. Do you want to commit now?"; during a rebase the caller
  asks "You are in the middle of a rebase, continue rebase?"
  ([GE `FormResolveConflicts.cs` L283-298](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L283-L298),
  [`MessageBoxes.cs` L22-28](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/MessageBoxes.cs#L22-L28)).
  The resolve dialog's "&Reset" is `reset --hard`: "…All changes since the last commit will be
  deleted. Do you want to reset the changes?" then "Are you sure you want to DELETE all changes?
  This action cannot be made undone."
  ([GE `FormResolveConflicts.cs` L755-779](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L755-L779)).
- **VS Code.** *Continue* is `git.commit`: `rebase --continue` with `GIT_EDITOR=true`, otherwise
  `git commit` with the box's message
  ([VSC `repository.ts` L1436-1452](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L1436-L1452)).
  It is enabled when "Merge Changes" is empty, or when anything is staged, so with auto-merged
  files staged it stays enabled while conflicts remain, and git's refusal surfaces as a generic
  "Git: …" error ([VSC `actionButton.ts` L134-140](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/actionButton.ts#L134-L140)).
  The aborts are palette commands; only "Abort Rebase" is also in a menu (… → Commit); none
  confirms ([VSC `package.json` L902-907, L548-553, L721-726](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/package.json#L902-L907),
  [`commands.ts` L3399-3402](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L3399-L3402)).
  No `git.continue`, no Skip, no revert. The docs send cherry-pick and revert to the terminal
  ([VSCDOC `merge-conflicts.md` L145-169](https://github.com/microsoft/vscode-docs/blob/35e82eeecd596e95917037b296b1c83fb4a6be64/docs/sourcecontrol/merge-conflicts.md#L145-L169)).
- **Fork.** *Abort* and *Resolve* on the banner; *Continue Rebase* replaces *Commit*, and is
  missing while anything is unstaged, e.g. a dirty submodule [maint]
  ([Tracker#895](https://github.com/fork-dev/Tracker/issues/895)). Abort merge asks "Do you want
  to abort the merge? — All changes since the last commit will be deleted." with *Abort merge*
  [maint screenshot] ([Tracker#1870](https://github.com/fork-dev/Tracker/issues/1870)); Fork 2.29
  (Mac) / 1.84 (Win): "Do not discard local changes when aborting merge".
- **Sourcetree.** *Actions → Continue Rebase / Abort Rebase* (Mac 1.3.0), or *Commit* mid-rebase
  opens the Rebase In Progress dialog (Mac 1.2.4) ([ST Mac notes](https://product-downloads.atlassian.com/software/sourcetree/ReleaseNotes/Sourcetree_4.2.19.html)).
  Abort Rebase doesn't confirm; requests are open
  ([SRCTREEWIN-6208](https://jira.atlassian.com/browse/SRCTREEWIN-6208)). Mac 1.2.8: "When
  continuing a conflicted rebase… so that no changes are left to commit, use 'rebase --skip'
  since 'rebase --continue' will fail". Cherry-pick continue only in Mac 4.2.18 (2026-06-04,
  [SRCTREE-3133](https://jira.atlassian.com/browse/SRCTREE-3133)).
- **SmartGit.** "the **Rebase in progress** dialog with **Continue rebase**, **Skip current
  patch**, and **Abort rebase**. **Continue rebase** remains disabled until all conflicts have
  been resolved." ([SG Rebase](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Branch/Rebase)).
  Cherry-pick and revert: *Continue* and *Abort* from the banner
  ([SG Cherry-Pick](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Branch/Cherry-Pick),
  [Revert](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Branch/Revert)). 18.2: "Continue
  should invoke "git rebase --skip" if necessary"; 19.2: cherry-pick/revert Abort uses `--abort`
  instead of `reset --hard`; 24.1: "Rebase Continue: warn before applying unstaged working tree
  changes" ([changelog-18.2](https://www.syntevo.com/smartgit/changelog-18.2.txt),
  [changelog-20.1](https://www.syntevo.com/smartgit/changelog-20.1.txt),
  [changelog-24.1](https://www.syntevo.com/smartgit/changelog-24.1.txt)). Merge ends with
  *Commit*; a request to call it *Continue* is unanswered
  ([userecho #979](https://smartgit.userecho.com/communities/1/topics/979-continue-merge-after-conflict-solving)).
- **GitKraken.** Not documented; release notes mention "Cancel Rebase" on the interactive
  rebase panel (8.0.0) and fixes to "continuing a rebase" (10.0.2)
  ([GK 10.x notes](https://help.gitkraken.com/gitkraken-desktop/10x/)).
- **Sublime Merge.** Docs: *Continue rebase*, *Abort rebase*
  ([SM getting started](https://www.sublimemerge.com/docs/getting_started)). The rest is from
  Build 2132's strings **(binary)** and a community member: "When conflicts arise, the action
  buttons are `Abort rebase` & `Continue rebase (skip)`. Once the conflicts are resolved, the
  second becomes `Continue rebase`." (secondary,
  [sublime_merge#1276](https://github.com/sublimehq/sublime_merge/issues/1276)); also "There are
  still unresolved conflicts remaining. Are you sure you want to continue?" **(binary)**.

**What Skip drops** is said by none of them. TortoiseGit's skip is `reset --hard`
([TG `RebaseDlg.cpp` L2278-2286](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L2278-L2286)),
Git Extensions' is `rebase --skip`; neither names the commit or the resolutions it throws away.

## 4. The merge tool

### 4.1 Whose setting

| Client | Setting | Reads git's `merge.tool` | Writes git config | Launches |
|---|---|---|---|---|
| TortoiseGit | Settings → Diff Viewer → Merge Tool, registry `HKCU\Software\TortoiseGit\Merge`, per extension | no | no | the tool itself, `%base %theirs %mine %merged` |
| Git Extensions | Settings → Git → Config, a page over git config | `merge.guitool`, then `merge.tool` | `merge.guitool`, `mergetool.<name>.path`, `.cmd`, at the scope chosen | the tool itself when path and cmd are set, else `git mergetool --gui` |
| VS Code | none (built-in only) | no | no | n/a |
| Fork | Preferences → Integration → Merge Tool, Fork's `settings.json` | no (open requests) | not documented | the tool itself |
| Sourcetree | Tools → Options → Diff | no: "we write the settings… we are not reading from it" [staff] | **global** `merge.tool = sourcetree`, `mergetool.sourcetree.cmd`, `trustExitCode = true` | `git mergetool -y --tool=sourcetree` |
| SmartGit | Preferences → Tools → Conflict Solvers, per file pattern | not documented | not documented | the tool itself |
| GitKraken | Preferences → External Tools → External Merge Tool | only via its "Git Config Default" entry | not documented | not documented |
| Sublime Merge | none (built-in only) | no | no | n/a |

- **TortoiseGit:** "&TortoiseGitMerge" or "&External" with a command line; "Advanced..." per
  extension; a leading `#` marks the Shift "alternative" tool. A grep for `merge.tool`,
  `mergetool` and `diff.tool` finds only the registry keys
  ([TG `AppUtils.cpp` L280-336](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L280-L336),
  [manual](https://tortoisegit.org/docs/tortoisegit/tgit-dug-settings.html#tgit-dug-settings-Merge),
  [screenshot](https://tortoisegit.org/docs/tortoisegit/images/SettingsMergeTool.png)).
- **Git Extensions:** `ConfigureDiffMergeTool` sets `merge.guitool` (Git ≥ 2.20) and
  `mergetool.<name>.path` / `.cmd`
  ([GE `DiffMergeToolConfigurationManager.cs` L33-80, L200-209](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitCommands/DiffMergeTools/DiffMergeToolConfigurationManager.cs#L33-L80),
  [`SettingKeyString.cs` L72-77](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitCommands/Config/SettingKeyString.cs#L72-L77)).
  The page opens read-only on "Effective"; the user picks "Local for current repository" or
  "Global for all repositories" to edit
  ([GE `SettingsPageHeader.cs` L72-134](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/SettingsDialog/SettingsPageHeader.cs#L72-L134)).
- **Sourcetree:** "SourceTree configures the tool in git's own config files… via
  `git mergetool -y --tool=sourcetree -- filename`" [staff]
  ([SRCTREE-2598](https://jira.atlassian.com/browse/SRCTREE-2598)); "SourceTree uses the global
  config only and not local config per repo… Though we write the settings from SourceTree to
  gitconfig, we are not reading from it right now" [staff, 2017]
  ([Community](https://community.atlassian.com/forums/Sourcetree-questions/Can-Does-SourceTree-use-local-git-config-for-Merge-and-Diff-tool/qaq-p/613626)).
  Gated by "Allow Sourcetree to modify your global Git and Mercurial config files"; with it off
  the tool fields are disabled [staff] ([SRCTREE-2711](https://jira.atlassian.com/browse/SRCTREE-2711)).
  It overwrote anyway until Win 3.4.22 ("Sourcetree overwrites .gitconfig without permission",
  [SRCTREEWIN-8484](https://jira.atlassian.com/browse/SRCTREEWIN-8484)), and wrote even with no
  change until 3.4.23 ([ST Win notes](https://product-downloads.atlassian.com/software/sourcetree/windows/ga/ReleaseNotes_3.4.32.html)).
  Its "System Default" choice is not documented; a user report reads it as git's own tool
  (secondary, [SRCTREEWIN-6150](https://jira.atlassian.com/browse/SRCTREEWIN-6150)).
- **Fork:** the Mac list is FileMerge, Beyond Compare, Kaleidoscope, KDiff3, P4Merge, Araxis,
  DiffMerge, VSCode, UnityYAMLMerge, Custom, with no "use git config" entry
  (secondary, [Tracker#2574](https://github.com/fork-dev/Tracker/issues/2574)). Windows: "Fork
  just shows list of the tools found in your system… right click and select 'primary'" [maint]
  ([TrackerWin#2513](https://github.com/fork-dev/TrackerWin/issues/2513)). Requests to respect
  `.gitconfig`'s mergetool have no maintainer answer
  ([TrackerWin#547](https://github.com/fork-dev/TrackerWin/issues/547),
  [Tracker#2235](https://github.com/fork-dev/Tracker/issues/2235)).
- **SmartGit:** "SmartGit comes with a built-in conflict solver (three-way-merge) which will be
  used by default … If you prefer, you can configure external three-way-merge tools."
  ([SG Preferences → Tools](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Preferences/Tools),
  [edit dialog](https://docs.syntevo.com/SmartGit/Latest/Manual/attachments/conflict-solver-vscode.png)).
  No doc or changelog from 17.1 to 26.1 says it reads or writes `merge.tool`; a user asking for
  "like `git mergetool`" got no answer on that
  ([userecho #1199](https://smartgit.userecho.com/communities/1/topics/1199-better-integration-with-external-diffmerge-tools-on-working-tree-and-conflict-resoving)).
- **GitKraken:** supported tools Beyond Compare, FileMerge, Kaleidoscope, KDiff, Araxis, P4Merge;
  not Meld, SemanticMerge, TortoiseMerge, WinMerge. The dropdown has `<None>`, the tools, and
  "Git Config Default": "While GitKraken Client allows Git Config Default merge tools, not all
  tools will be compatible" ([GK branching-and-merging](https://help.gitkraken.com/gitkraken-desktop/branching-and-merging/),
  [GK preferences](https://help.gitkraken.com/gitkraken-desktop/preferences/),
  [screenshot](https://help.gitkraken.com/wp-content/uploads/configureExternalTool@2x.png); the
  quote is from the 2023 page, archived at
  `web.archive.org/web/20231208113705/https://help.gitkraken.com/gitkraken-client/branching-and-merging/`).
  It also has 'Delete ".orig" files after merging' (same screenshot).
- **Sublime Merge:** no external tool setting; "Support external diff/merge tool" has been open
  since 2018 ([sublime_merge#58](https://github.com/sublimehq/sublime_merge/issues/58)). Its docs
  tell the user to run `git config merge.tool smerge` for the reverse direction
  ([SM command line](https://www.sublimemerge.com/docs/command_line)).
- **VS Code:** no `mergetool` anywhere in `extensions/git/src` (grep). Its docs tell the user to
  set `git config --global merge.tool vscode` and `mergetool.vscode.cmd 'code --wait --merge …'`
  for the reverse direction ([VSCDOC `merge-conflicts.md` L192-212](https://github.com/microsoft/vscode-docs/blob/35e82eeecd596e95917037b296b1c83fb4a6be64/docs/sourcecontrol/merge-conflicts.md#L192-L212));
  git ships `vscode` since 2.47 (TOOLS §5).

### 4.2 When no merge tool is configured

| Client | What happens |
|---|---|
| TortoiseGit | can't happen: the default is the bundled TortoiseGitMerge (TortoiseGitIDiff for images) |
| Git Extensions | Settings checklist: "You need to configure merge tool in order to solve merge conflicts." with *Repair* → the Config page; opening the resolve dialog: "There is no mergetool configured.\nPlease go to settings and set a mergetool!"; the Config page lists known tools (kdiff3, p4merge, meld, vscode, …), searches their install folders and *Suggest* fills the command; nothing ships by default since 2.51.02 |
| VS Code | n/a: built-in only |
| Fork | the built-in merger is always there; what *Merge in External Tool* does with none set is not documented; Windows lists the tools it finds |
| Sourcetree | *Launch External Merge Tool* is greyed out; staff: "Open Tools/Options/Diff and select and configure the external tools" |
| SmartGit | the built-in Conflict Solver is the default; no detection documented |
| GitKraken | default `<None>`; the built-in merge tool remains; "Git Config Default" is an explicit choice |
| Sublime Merge | n/a: built-in only |

Sources: TortoiseGit [`AppUtils.cpp` L304-336](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L304-L336);
Git Extensions [`ChecklistSettingsPage.cs` L85-107, L469-490](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/SettingsDialog/Pages/ChecklistSettingsPage.cs#L85-L107),
[`FormResolveConflicts.cs` L35-36, L690-753](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L690-L753),
[`DiffMergeToolConfigurationManager.cs` L133-184](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitCommands/DiffMergeTools/DiffMergeToolConfigurationManager.cs#L133-L184),
[manual "Mergetool"](https://git-extensions-documentation.readthedocs.io/en/main/settings.html#mergetool)
("Git Extensions will search for common merge tools on your system"),
[`ChangeLog.md` L5176-5183](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/Resources/ChangeLog.md#L5176-L5183);
Fork [TrackerWin#2513](https://github.com/fork-dev/TrackerWin/issues/2513);
Sourcetree [Community](https://community.atlassian.com/forums/Sourcetree-questions/How-to-enable-launch-external-merge-tool/qaq-p/914843) [staff],
[SRCTREEWIN-4590](https://jira.atlassian.com/browse/SRCTREEWIN-4590);
SmartGit [Preferences → Tools](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Preferences/Tools);
GitKraken as in §4.1.

**For #274 (derived):**

- No client falls back to git's own guess (TOOLS §4.1). Every client with no configured tool
  either has a built-in merger to fall back to or tells the user to go to settings. Parterre has
  no built-in merger in v1, so it is in Git Extensions' and Sourcetree's position.
- Only the two clients that run `git mergetool` (Git Extensions, Sourcetree) write git config, and
  both write it from a **settings page**, never silently at the moment of conflict. Git
  Extensions lets the user choose repository or global and writes `merge.guitool`; Sourcetree
  writes only global, asks permission once, and has a history of writing when it shouldn't.
- Detection of installed tools lives in settings: Git Extensions searches known install folders
  for its tool list; Fork on Windows lists what it finds. No client offers a "pick one of these"
  choice at the conflict itself.
- GitKraken's "Git Config Default" and Sublime Merge's / VS Code's "set `merge.tool` yourself"
  docs are the only places a client says *git's* tool in so many words.

### 4.3 Waiting, and marking resolved afterwards

- **TortoiseGit:** an external tool is waited for only with "Block TortoiseGit while executing
  the external merge tool"; then "Was the merge of\n%s\nsuccessful?" with *Resolved* / *No*, or,
  with "Trust the exitcode of the external merge tool for auto resolving", `git add -f` on exit 0
  ([TG `AppUtils.cpp` L367-396](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L367-L396),
  [rc L4003](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L4003)).
  TortoiseGitMerge isn't waited for; it marks the file itself with "Mark as resolved", and on a
  save with no conflicts left asks "Do you want to mark the file\n%s\nas resolved?"
  ([TG `MainFrm.cpp` L1739-1786, L2262-2285](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseMerge/MainFrm.cpp#L1739-L1786));
  the list refreshes after (`RefreshFileListAfterResolvingConflict`).
- **Git Extensions:** waits for the tool; stages if it exits 0 **and** the file's mtime changed;
  otherwise asks "Is the merge conflict solved?". With no path or cmd it runs
  `git mergetool --gui -- <file>` in a console and doesn't stage ("git-mergetool does not provide
  exit status") ([GE `FormResolveConflicts.cs` L590-669](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormResolveConflicts.cs#L590-L669)).
  With no base: "There is no base revision for '{0}'. Fall back to 2-way merge?"
- **Sourcetree:** a "Visual Merge In Progress" dialog with *Abort* while the tool runs; it blocked
  the whole Windows app until 3.4.23 ([SRCTREEWIN-6980](https://jira.atlassian.com/browse/SRCTREEWIN-6980)).
  It writes `trustExitCode=true` because otherwise git "tries to prompt you to say whether the
  merge succeeded… impossible because you're not on a terminal… just hangs there forever"
  [staff] ([SRCTREEWIN-1265](https://jira.atlassian.com/browse/SRCTREEWIN-1265)), the prompt
  TOOLS §3 found.
- **Fork:** Mac once staged "when their last-update time get changed" [maint, 2018]
  ([Tracker#178](https://github.com/fork-dev/Tracker/issues/178)), then dropped it: "never
  staged automatically conflicts resolved with 3rd party tools… no quick and reliable way to
  determine if a conflict is resolved" [maint, 2019]
  ([TrackerWin#110](https://github.com/fork-dev/TrackerWin/issues/110)). Since Mac 2.58 / Win
  2.13 it shows the diff of a file resolved in an external tool.
- **SmartGit:** 22.1: "by default SmartGit waits for the external process to finish before
  proceeding (low-level property externalConflictSolver.waitForProcess)" and then shows "the
  resolve-dialog" ([changelog-22.1](https://www.syntevo.com/smartgit/changelog-22.1.txt)); the
  manual still says to run the tool in the background
  ([SG Tools](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Preferences/Tools)).
- **GitKraken:** not documented.

### 4.4 Leftover conflict markers

| Client | Checks the text when marking resolved? |
|---|---|
| TortoiseGit | no: the string "The selected file appears to still have one or more conflict markers in it…" is in the resources but unused ([rc L3713-3714](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L3713-L3714)); TortoiseGitMerge warns on save about its own blocks: "There are still unresolved conflicts in line %d!…" ([`MainFrm.cpp` L3078-3103](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseMerge/MainFrm.cpp#L3078-L3103)) |
| Git Extensions | no (no `<<<<<<<` in the source) |
| VS Code | yes: modal "Are you sure you want to stage {0} with merge conflicts?" for text matching `^<{7}\s\|^={7}$\|^>{7}\s`, always for both-deleted and added-by-one ([`commands.ts` L395-411, L1533-1545](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L1533-L1545)); the merge editor's "Complete Merge" asks before "Complete with Conflicts" |
| Fork | yes: "Fork recognizes conflicts by reading the content of the file and checking if it still contains `<<<<<<` blocks, but for binary files this doesn't work" [maint] ([TrackerWin#2529](https://github.com/fork-dev/TrackerWin/issues/2529)) |
| Sourcetree | no: Won't Fix, "Since external merge tools are sending wrong return code, we can not fix this" ([SRCTREEWIN-14248](https://jira.atlassian.com/browse/SRCTREEWIN-14248)) |
| SmartGit | yes: 22.1 "Stage: warns if a (text) file contains conflict markers" ([changelog-22.1](https://www.syntevo.com/smartgit/changelog-22.1.txt)); Conflict Solver's Close warns too |
| GitKraken | not documented |
| Sublime Merge | in its tool: "Saving a file with unresolved conflicts will warn before saving" (Build 2020); Stage on the list doesn't, per a user (secondary) |

## 5. Stops without conflicts

- **`edit`:** TortoiseGit's button becomes "Amend", with an "Edit/Split commit" checkbox; a dirty
  tree asks "The working tree is not clean and contains unstaged changes.\nReview and commit the
  changes?" ([TG `RebaseDlg.cpp` L1568-1740](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L1568-L1740)).
  Fork shows "Amending '…'" in the banner, an *Amend* checkbox and *Continue Rebase* [maint]
  ([Tracker#895](https://github.com/fork-dev/Tracker/issues/895)). SmartGit's interactive rebase
  stop has *Step*, *Continue*, *Abort*
  ([SG Rebase-Interactive](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Branch/Rebase-Interactive)).
  Git Extensions shows the blue "Rebase is currently in progress." banner with *Continue*
  ([GE `FormRebase.cs` L149-205](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitUI/CommandsDialogs/FormRebase.cs#L149-L205)).
  VS Code shows the same *Continue* as after conflicts **(derived)**; after `break` or a failed
  `exec` git has removed `REBASE_HEAD`, so VS Code shows no rebase at all **(derived from code,
  unverified)**. GitKraken's interactive rebase has no `edit`
  ([GK interactive-rebase](https://help.gitkraken.com/gitkraken-desktop/interactive-rebase/)).
- **Empty pick:** TortoiseGit "The current commit will be empty (e.g., due to conflict
  resolution). Skip the commit or keep the message only commit?" with *Commit* / *Skip* /
  *Cancel* ([rc L3990](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L3990)).
  Sourcetree and SmartGit skip on *Continue* (§3). Fork leaves only *Abort*; [maint]:
  "Probably Fork should not ask and just skip." ([TrackerWin#1471](https://github.com/fork-dev/TrackerWin/issues/1471)).
  VS Code aborts an empty cherry-pick itself: "The changes are already present in the current
  branch." ([VSC `git.ts` L2581-2597](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/git.ts#L2581-L2597)).
  Sublime Merge Build 2130 added "an option to skip empty commits when cherry-picking multiple
  commits".
- **A pick that fails without conflicts** (TortoiseGit): "Cherry-pick failed (please see log in
  the cherry pick/rebase dialog for details)! Skip this commit?" with *Skip* / *Retry* /
  *Cancel* and "Do the same for the rest" ([rc L3891-3892](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L3891-L3892)).
- **A refused hook:** git's output in a message box, the stop stays, retry possible (TortoiseGit
  [L1478-1482](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L1478-L1482);
  VS Code's generic "Git: …"; GitKraken "Any non-zero exit code blocks the Git action",
  [GK githooks](https://help.gitkraken.com/gitkraken-desktop/githooks/)). Fork added "Skip hook"
  when pre-commit fails (Mac 1.0.57). Not documented for the others. No client has a "hook
  refused" state.
- **Prefilled message:** TortoiseGit and VS Code load `MERGE_MSG` (VS Code the `REBASE_HEAD`
  message during a rebase, and refuses edits: "It's not possible to change the commit message in
  the middle of a rebase…") ([VSC `repository.ts` L1121-1141](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/repository.ts#L1121-L1141));
  SmartGit and Fork show git's `# Conflicts:` list.

## 6. A conflicted autostash

None of the clients detects git's "Applying autostash resulted in conflicts" or offers recovery.

- **TortoiseGit** stashes itself with `git stash` when `rebase.autostash` is set, and pops at the
  end: "Stash POP failed, there are conflicts\nDo you want to see changes?" opens Check for
  modifications; otherwise "Stash POP failed!" with git's output
  ([TG `RebaseDlg.cpp` L945-1004](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/RebaseDlg.cpp#L945-L1004),
  [`AppUtils.cpp` L210-255](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L210-L255)).
  The entry stays; "Stash Apply", "Stash Pop", "Stash List" appear while `refs/stash` exists.
- **Git Extensions** draws an "Autostash" pseudo-commit while `rebase-merge/autostash` exists,
  with "Apply stash" on it ([GE `RevisionReader.cs` L356-382](https://github.com/gitextensions/gitextensions/blob/0aea2d6df4c120806c46f1797ba6a45f5d14b345/src/app/GitCommands/RevisionReader.cs#L356-L382)),
  but nothing after a failed reapply.
- **VS Code** maps `^CONFLICT` from `stash pop` to "There are merge conflicts while applying the
  stash. Please resolve them before committing your changes." with *Show Changes*; the merge
  editor calls the side "Stashed Changes"; nothing says the entry was kept
  ([VSC `git.ts` L2683-2698](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/git.ts#L2683-L2698),
  [`commands.ts` L5607-5612](https://github.com/microsoft/vscode/blob/1.140.0/extensions/git/src/commands.ts#L5607-L5612)).
- **Fork** does its own "stash and reapply": "`rebase` and `--autostash` combination in git is
  harmful… In Fork 1.52 the 'stash-and-reapply' functionality is handled by Fork (not by git)"
  [maint] ([TrackerWin#622](https://github.com/fork-dev/TrackerWin/issues/622)); what it shows on
  a failed reapply is not documented.
- **SmartGit** staff recommend git's `rebase.autostash` over its own [staff]
  ([userecho #681](https://smartgit.userecho.com/communities/1/topics/681-rebase-support-autostashing-working-tree-changes));
  not documented further. **Sourcetree** has no autostash (open requests). **GitKraken**,
  **Sublime Merge**: not documented.

## 7. Common against quirks

Common to most (derived):

- The conflicted files are listed where changes are always listed, marked, and sorted or grouped
  first. A separate dialog is TortoiseGit's and Git Extensions' way.
- Per file: open in a merge tool, take one side, mark resolved (`git add`), and a two-button
  question for deleted against modified.
- *Continue* is the commit button renamed, or next to it.
- *Abort* is always offered; Skip is not.
- Cherry-pick and revert get less than merge and rebase.
- No client says what *Skip* drops or that a conflicted stash is still in the stash list.

One client's quirk:

- TortoiseGit's rebase that isn't `git rebase`, and its reset-type dialog for *Abort Merge*.
- Git Extensions writing `merge.guitool` rather than `merge.tool`.
- Sourcetree writing global git config behind a permission option, and never reading it back.
- VS Code's *Continue* enabled while files are still unmerged, when something else is staged.
- SmartGit's swap of the panes during a rebase, now hidden in a low-level property.
- Fork reading the file for markers but refusing to auto-stage.
- GitKraken's "Git Config Default" tool entry.

Detection gaps worth avoiding (derived): TortoiseGit misses `rebase-merge`; Git Extensions misses
`CHERRY_PICK_HEAD` and `REVERT_HEAD`; VS Code builds `<root>/.git/…` paths (linked worktrees) and
expects `REBASE_HEAD` as a file (reftable); Fork had to add reftable detection in 2026. Parterre
already reads the state through git, which avoids all four.

## 8. Not found

- GitKraken: the stopped-state banner, n/m, Continue / Skip / Abort wording, whether it writes git
  config, waiting and auto-marking.
- Sourcetree: n/m, what "System Default" means, the hook-refused UI.
- Fork: what *Merge in External Tool* does with nothing configured; a failed reapply.
- SmartGit: whether it reads git's `merge.tool`, exact dialog texts for Continue / Abort, the
  deleted-against-modified UI.
- Sublime Merge: where its state strings appear; anything about external tools beyond the open
  request.
- Screenshots of a stopped rebase in TortoiseGit's and Git Extensions' manuals (none exist).
- Broken manual links: TortoiseGit's `tgit-dug-settings-progs`, `tgit-dug-resolve`,
  `tgit-dug-merge-abort` (404; the content is in `tgit-dug-settings.html` and
  `tgit-dug-conflicts.html`); GitKraken's `merge-conflict-resolution-tool/` (404; now in
  branching-and-merging).

## Screenshots

- TortoiseGit: [Resolve](https://tortoisegit.org/docs/tortoisegit/images/ResolveConflict.png),
  [delete/modify](https://tortoisegit.org/docs/tortoisegit/images/resolve-delete-modify-conflict.png),
  [submodule](https://tortoisegit.org/docs/tortoisegit/images/resolve-submodule-conflict.png),
  [Rebase dialog](https://tortoisegit.org/docs/tortoisegit/images/GitRebase.png),
  [Merge Tool settings](https://tortoisegit.org/docs/tortoisegit/images/SettingsMergeTool.png).
- Git Extensions: [banner](https://git-extensions-documentation.readthedocs.io/en/main/_images/merge_conflicts.png),
  [resolve dialog](https://git-extensions-documentation.readthedocs.io/en/main/_images/resolve_merge_conflicts.png),
  [its menu](https://git-extensions-documentation.readthedocs.io/en/main/_images/resolve_merge_conflicts_menu.png),
  [settings checklist](https://git-extensions-documentation.readthedocs.io/en/main/_images/settings.png).
- VS Code: [inline markers](https://code.visualstudio.com/assets/docs/sourcecontrol/overview/merge-conflict.png),
  [merge editor](https://code.visualstudio.com/assets/docs/sourcecontrol/overview/merge-editor-overview.png).
- Fork: [rebase `edit` banner](https://user-images.githubusercontent.com/618115/73735363-6dde0500-473f-11ea-8c47-fcccb9b088d7.png) [maint],
  [Choose '<branch>'](https://user-images.githubusercontent.com/618115/99238452-08549400-27fa-11eb-99ad-f12a22c79f67.png) [maint],
  [abort merge](https://github.com/fork-dev/Tracker/assets/618115/5146e0e7-e8c3-4fd9-9f66-88707eff0a98) [maint],
  [merger](https://fork.dev/blog/posts/fork-1.0.73/merger.jpg).
- Sourcetree: [Rebase In Progress](https://jira.atlassian.com/secure/attachment/477890/image-2024-12-26-17-29-48-851.png) [staff].
- SmartGit: [banner](https://docs.syntevo.com/SmartGit/Latest/Manual/images/Working-Tree-Status.png),
  [Conflict Solver](https://docs.syntevo.com/SmartGit/Latest/Manual/images/Tools-SmartGit-ConflictSolver.png).
- GitKraken: [merge conflict](https://help.gitkraken.com/wp-content/uploads/merge-conflict@2x.png),
  [take current/incoming](https://help.gitkraken.com/wp-content/uploads/current-incoming.png),
  [external tool setting](https://help.gitkraken.com/wp-content/uploads/configureExternalTool@2x.png).
- Sublime Merge: [merge tool](https://www.sublimemerge.com/images/merge_tool.png).
