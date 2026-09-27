# TortoiseGitBlame's log pane

Research for [#109](https://github.com/aquamoth/parterre/issues/109), part of the map
[#108](https://github.com/aquamoth/parterre/issues/108) (a history pane for parterre's blame
window). The question: how does TortoiseGitBlame's log pane behave? What does it list, what does
choosing do in each direction, where does it sit, what do its row menu and double-click do, and
what happens to it after *Blame previous revision*?

Everything comes from reading TortoiseGit's source and manual at one pinned commit. No GUI was run.
A statement that is my own reading of the code, rather than something the code or docs say
outright, is marked **(derived)**. Things I could not settle are marked **(unverified)**.

## Sources (pinned)

All TortoiseGit links point at `master` @ `acc10fc20afe36aabc4afbc5f1af33f31acae32e`
(2026-06-27), the same commit as the earlier research. The current `master` (`5a8bb215`) has no
changes to `src/TortoiseGitBlame/`, `GitLogListBase.cpp`, `LogDataVector.cpp` or `Git.cpp` since
then.

| Short name | File |
|---|---|
| OW | [src/TortoiseGitBlame/OutputWnd.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp): the docking pane that holds the log list |
| BLL | [src/TortoiseGitBlame/LogListBlameAction.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/LogListBlameAction.cpp): `CGitBlameLogList`, the blame-specific log list |
| BV | [src/TortoiseGitBlame/TortoiseGitBlameView.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp): the gutter and text |
| BVH | [src/TortoiseGitBlame/TortoiseGitBlameView.h](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.h) |
| BD | [src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp): loading a blame |
| BDH | [src/TortoiseGitBlame/TortoiseGitBlameData.h](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameData.h), [.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameData.cpp) (BDC) |
| MF | [src/TortoiseGitBlame/MainFrm.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/MainFrm.cpp): frame and docking |
| PW | [src/TortoiseGitBlame/PropertiesWnd.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/PropertiesWnd.cpp): the "Commit Info" pane |
| APP | [src/TortoiseGitBlame/TortoiseGitBlame.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlame.cpp) |
| BRC | [src/Resources/TortoiseGitBlameENG.rc](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc): menus, accelerators, strings |
| GLB | [src/TortoiseProc/GitLogListBase.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp) and [.h](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.h) (GLBH): the Log dialog's list, which the pane reuses |
| LDV | [src/TortoiseProc/LogDataVector.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDataVector.cpp), [LogDlgHelper.h](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDlgHelper.h) (LDH) |
| GIT | [src/Git/Git.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Git/Git.cpp) |
| CM | [src/TortoiseProc/ColumnManager.h](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/ColumnManager.h) |
| LLC | [src/Resources/TortoiseLoglistCommon.rc2](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseLoglistCommon.rc2): log-list menu labels |
| AU / BC | [src/TortoiseProc/AppUtils.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/AppUtils.cpp), [Commands/BlameCommand.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/Commands/BlameCommand.cpp) |
| DOC | [doc/.../tgit_dug/dug_blame.xml](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/doc/source/en/TortoiseGit/tgit_dug/dug_blame.xml), [dug_settings_blame.xml](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/doc/source/en/TortoiseGit/tgit_dug/dug_settings_blame.xml) (DOCS) |
| git | [git-log, History Simplification](https://git-scm.com/docs/git-log#_history_simplification), [git-blame](https://git-scm.com/docs/git-blame) |

The manual never mentions the log pane or the Commit Info pane. Its blame page is also out of date
in places (§7), so the source is the authority throughout.

## TL;DR

- **What it lists:** by default, every commit that changed the file, reachable from the blamed
  revision: `git log --parents --topo-order <blamed rev> -- <path>`, newest first, with git's
  default history simplification. So there are no newer commits, no rename following unless
  *Follow renames* is on, and a merge only when it changed the file relative to every parent. When
  the history can't be a single-path log (moved or copied lines detected from other files, or
  *first parents only*), it lists only the commits that own lines instead.
- **Columns:** graph, ID (a count with the oldest at 1), full hash, message (with ref labels),
  author, date. More are available in the header menu. The blamed revision is **not marked**.
  Only HEAD is bold, as in the Log dialog.
- **Choosing is one shared set of commits.** Clicking a line in the blame gutter chooses its
  commit: every line of that commit is highlighted in the gutter and the text, the commit's row is
  selected and scrolled into view, and Commit Info shows the commit. Choosing a row (by mouse or
  keyboard) highlights that commit's lines. If the commit wasn't already chosen, the text also
  scrolls to its **first line** and selects that line's text. A commit that owns no lines is
  chosen and shown in Commit Info, but nothing is highlighted and nothing scrolls. Ctrl+click in
  the gutter and multi-select in the list choose several commits.
- **Where it sits:** an MFC docking pane at the **bottom** ("Git Log"). A second docking pane,
  "Commit Info", sits on the **right** as a read-only property grid: hash, author, committer,
  subject, body and parents. Both can be moved, floated or auto-hidden.
- **Row menu:** Blame previous revision, Show changes as unified diff, Compare with previous
  revision, Show log, Browse repository, Switch/Checkout, Create branch/tag, Export, Copy to
  clipboard, Show branches this commit is on. **Double-click** does nothing unless the global
  "diff by double-click in log" setting is on, in which case it compares with the previous
  revision.
- **After *Blame previous revision*** (from the gutter or a row), the list does not change. The
  command starts a **new TortoiseGitBlame process and window** at the parent. That window builds
  its own list from the parent, so the commit you came from and everything newer are gone from it.

## 1. What the pane lists

### 1.1 The command

After the blame is parsed, the document fills the pane in one of two ways
([BD#L268-L282](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L268-L282)).

**Complete log** is the default: the setting "Show complete log" defaults to 1. It is used
when move/copy detection is off or limited to the file (`-M`), and *Only consider first parents*
is off ([BD#L270-L275](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L270-L275),
[BlameDetectMovedOrCopiedLines.h#L30-L33](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/BlameDetectMovedOrCopiedLines.h#L30-L33)).
It calls `LoadHistory(path, m_Rev, follow)`, which runs a log of the path starting at the blamed
revision, with `--follow` only if *Follow renames* is on (default off)
([OW#L120-L131](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L120-L131)).
The info mask is only `LOG_INFO_FOLLOW` or 0, so no `--numstat`, `--raw` or `-c`. The log runs
in process through libgit (`git_open_log`) with these arguments
([LDV#L50-L101](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDataVector.cpp#L50-L101),
[GIT#L1028-L1168](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Git/Git.cpp#L1028-L1168)):

```
git log -z --parents [--follow] --topo-order --end-of-options <blamed rev> -- <path>
```

`--topo-order` is the default of the global "LogOrderBy" setting. It can also be `--date-order`,
`--author-date-order` or none
([LDH#L51](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDlgHelper.h#L51),
[GIT#L1134-L1139](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Git/Git.cpp#L1134-L1139)).
The manual describes the setting as: "the log contains all changes for a file, even the changes
have no impact on the file content of the annotated revision"
([DOCS#L106-L126](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/doc/source/en/TortoiseGit/tgit_dug/dug_settings_blame.xml#L106-L126)).

**Owners only** is used otherwise: "Show complete log" off, detection from other files
(`-C`, `-C -C`, `-C -C -C`), or first parents only. The pane gets exactly the set of commits that
own at least one line (`GetHashes`)
([BD#L276-L282](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L276-L282),
[BDH#L81-L88](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameData.h#L81-L88),
[OW#L133-L141](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L133-L141)).
Each commit is read directly, with no git log. They are sorted so a child comes before its
parent, and otherwise by committer date, newest first
([LDV#L194-L261](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDataVector.cpp#L194-L261)).
That comparator is not a strict weak ordering in general, so the order is "roughly newest
first" **(derived)**.

### 1.2 Which commits, then

- **Only the blamed revision and its ancestors.** The range is the blamed revision, so nothing
  newer is listed. The blamed revision itself is a row only if it changed the file. Otherwise the
  top row is the last commit before it that did **(derived from the command)**.
- **Merges:** these follow git's default history simplification for a path. A merge whose file
  matches one parent is simplified away. A merge that changed the file relative to every parent
  (for example, a conflict resolution) is listed. `--parents` rewrites parents so that the graph
  joins up ([git-log, History Simplification](https://git-scm.com/docs/git-log#_history_simplification)).
  I didn't check how `--follow` changes merge handling **(unverified)**.
- **Renames:** `git blame` always follows whole-file renames
  ([git-blame](https://git-scm.com/docs/git-blame)), but the default log (no `--follow`) stops at
  the rename. So lines older than a rename can belong to commits that aren't in the list. Such
  lines have no row (`m_lineToLogIndex` = -2)
  ([BV#L1514-L1538](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1514-L1538)).
  The effects: they are not age-coloured, since age is computed from the row position
  ([BV#L1672-L1686](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1672-L1686)).
  Their gutter "log ID" shows 0
  ([BV#L953-L964](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L953-L964)).
  Clicking them selects no row **(derived)**.
- **The list is not re-run when you choose anything.** It changes only on a reload: toggling
  *Show complete log*, *Follow renames*, *first parents*, whitespace or detection re-opens the
  document ([BV#L2169-L2238](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L2169-L2238)).

### 1.3 Columns and marking

The pane uses the Log dialog's list class with `m_IsIDReplaceAction = TRUE` and its own
column-settings key "Blame"
([OW#L95-L100](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L95-L100)).
The column table is `{id, title, visibleByDefault, available, width}`
([CM#L23-L30](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/ColumnManager.h#L23-L30),
[GLB#L321-L336](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L321-L336)),
which gives:

- **Shown by default:** Graph, ID, Hash (full SHA-1), Message, Author, Date.
- **Hidden but available:** Email, Committer name, Committer email, Commit date. Bug IDs appear if
  bugtraq is configured, and SVN rev under git-svn.
- **Not available:** the Actions column, which ID replaces.
- The **Graph** column is hidden (width 0) in *Follow renames* mode and in owners-only mode
  ([OW#L126](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L126),
  [OW#L136](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L136),
  [GLB#L3254-L3261](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L3254-L3261)).
- **ID** is a count: the bottom (oldest) row is 1 and the top is N
  ([GLB#L1528-L1532](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L1528-L1532)).
  The gutter's optional "Show log ID instead of SHA-1" uses the same number
  ([BV#L953-L964](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L953-L964)).

**Marking.** No code marks the blamed revision. The only row emphasis is inherited from the Log
dialog: the HEAD commit is drawn bold
([GLB#L1097-L1104](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L1097-L1104),
HEAD read in [GLBH#L498](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.h#L498)),
and branch and tag labels are drawn in the message column. The blamed revision appears only in
the window title, `<path>:<rev>`
([BD#L295-L300](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L295-L300)).
No row is selected when the window opens **(derived: nothing selects one after
`LoadHistory`)**.

## 2. Choosing

TortoiseGitBlame keeps one set of chosen commits, `m_selectedHashes`
([BVH#L222](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.h#L222)).
The gutter, the text background, the list selection and Commit Info all show that set.

### 2.1 Choosing a line → the list

A left click in the **blame gutter** (the info column left of the text) runs `OnLButtonDown`
([BV#L1727-L1780](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1727-L1780)).
The text itself is a separate Scintilla child window, and the view has no click handler for it. So
clicking in the text does not choose anything **(derived)**.

- **Plain click** on a line whose commit isn't the only chosen one makes that commit the only
  chosen one. A plain click on the commit that is already the only chosen one **clears** the
  choice. The manual calls this "sticky"; you click again to turn it off
  ([DOC#L48-L53](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/doc/source/en/TortoiseGit/tgit_dug/dug_blame.xml#L48-L53)).
- **Ctrl+click** (`nFlags == 9`, meaning left button plus Ctrl) adds the line's commit to the set,
  or removes it.
- **In the list:** every row whose commit is in the set is selected and every other row is
  deselected. The clicked line's row is scrolled fully into view (`EnsureVisible(row, FALSE)`).
  Feedback from the list is blocked meanwhile (`m_bBlockUpdates`). Keyboard focus stays where it
  was **(derived: no `SetFocus`)**.
- **Commit Info** shows the commit if exactly one is chosen. Otherwise it is cleared.
- **In the text:** nothing scrolls and the caret does not move. The chosen commit's lines are
  repainted with the highlight (§2.3).
- **Right-click** runs the same code first
  ([BVH#L120](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.h#L120)).
  So right-clicking a line chooses its commit before the menu opens. The toggle rule suggests that
  right-clicking the commit that is already the only chosen one un-chooses it **(derived, not
  observed)**.

### 2.2 Choosing a row → the text

Selection changes in the list arrive as `LVN_ITEMCHANGED`, both selects and deselects. The handler
collects **all** selected rows' hashes and calls `FocusOn(hashes, newlySelectedRow)`
([OW#L143-L174](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L143-L174),
[BV#L1816-L1837](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1816-L1837)):

1. Any text selection is cleared.
2. If a row was newly selected and its commit **wasn't already chosen**, the text jumps to the
   commit's **first line in the file** (`FindFirstLine(hash, 0)`)
   ([BDH#L47-L56](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameData.h#L47-L56))
   via `GotoLine`. If that line is on screen, only the caret moves. Otherwise the view scrolls so
   the line lands about a third of the way down. The whole text of that line is then selected
   (`SCI_SETSEL`)
   ([BV#L678-L718](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L678-L718)).
3. The chosen set becomes the selected rows. **All** lines of every chosen commit are highlighted.
4. Commit Info shows the newly selected commit if exactly one row is selected. Otherwise it is
   cleared. An optional Gravatar picture (off by default) loads for the newly selected row's author
   ([OW#L82-L86](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L82-L86),
   [OW#L171-L172](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L171-L172)).

The list is created without `LVS_SINGLESEL`, so it allows several selected rows
([OW#L71](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L71)).

**A commit that owns no lines** in the blamed revision is common in complete-log mode. Choosing
its row puts it in the set, and Commit Info shows it. `FindFirstLine` returns -1, so the text
doesn't scroll and no line is highlighted. The earlier text selection has already been cleared in
step 1 **(derived)**. Nothing tells the user that the commit has no lines here.

**To walk through a commit's lines**, use *View > Next* / *View > Previous*, which are also
toolbar buttons with no default shortcut
([BRC#L99-L104](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L99-L104),
[BRC#L452-L453](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L452-L453)).
They scroll the text to the next or previous **block** of lines owned by any chosen commit,
without moving the caret
([BV#L1996-L2009](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1996-L2009),
[BDC#L309-L336](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameData.cpp#L309-L336)).

### 2.3 What the highlight looks like

Lines of chosen commits get the same background in the gutter and the text. That background is
the system highlight colour blended toward highlight-text (a dark blue in dark mode), and the
gutter text uses highlight-text
([BV#L911-L933](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L911-L933),
[BV#L1803-L1814](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1803-L1814),
[BV#L1688-L1709](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1688-L1709)).
Hovering greys only the gutter cell of the line under the mouse and shows a tooltip with the
commit's hash, author, date and message
([BV#L1839-L1894](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1839-L1894)).

### 2.4 Keyboard

- **In the list:** it is a standard Win32 report list view, so the arrow keys, Page Up/Down,
  Home/End and Shift/Ctrl selection all change the selection. Each change goes through §2.2, so
  moving with the arrows chooses commit after commit and scrolls the text to each one's first line
  **(derived: the handler reacts to any selection change, however it happens)**. The context-menu
  key opens the row menu at the selected row
  ([GLB#L1651-L1659](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L1651-L1659)).
- **In the text:** there is no keyboard way to choose the current line's commit. The frame's
  accelerators are Esc/Ctrl+Q (exit), Ctrl+F, F3/Shift+F3 (find), Ctrl+G (go to line), Ctrl+C
  (copy text) and F6/Shift+F6 (next/previous pane)
  ([BRC#L312-L331](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L312-L331)).
  Find and Go to line move the caret but do not choose a commit
  ([BV#L643-L662](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L643-L662)).
  I didn't check whether F6 moves focus between the docking panes **(unverified)**.

## 3. Where it sits

The frame is an MFC `CFrameWndEx` with Visual Studio-style smart docking and auto-hide
([MF#L119-L142](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/MainFrm.cpp#L119-L142)).
The blame view (gutter and text) fills the centre.

- **Log pane:** a `CDockablePane` created docked at the **bottom** (`CBRS_BOTTOM`), floatable
  (`CBRS_FLOAT_MULTI`) and dockable to any edge
  ([MF#L219-L230](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/MainFrm.cpp#L219-L230)).
  It is created with the caption "Git Revision List", then retitled "Git Log" in its `OnCreate`
  ([OW#L102](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L102),
  [BRC#L451](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L451),
  [BRC#L539](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L539)).
  I didn't confirm which caption shows on screen **(unverified)**. The list fills the pane. If
  Gravatar is on, an 80 px picture sits at its right
  ([OW#L106-L118](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L106-L118)).
- **Commit Info pane:** a second `CDockablePane`, docked on the **right** (`CBRS_RIGHT`)
  ([MF#L232-L240](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/MainFrm.cpp#L232-L240)).
  It is not a message text box. It is a read-only property grid with a description area
  ([PW#L113-L197](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/PropertiesWnd.cpp#L113-L197)).
  One group holds hash, author name, author date, author email, committer name, committer email,
  commit date, subject and body. A "Parents" group lists each parent's short hash and subject
  ([PW#L242-L310](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/PropertiesWnd.cpp#L242-L310)).
  Its only menu item is Copy, which copies the selected field.
- Both panes can be shown or hidden from *View > Toolbars and Docking Windows*
  ([BRC#L250-L258](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L250-L258)).
  Their layout is saved between runs (`CDockablePaneUnscaledStoredState`), like the window
  placement ([APP#L171-L218](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlame.cpp#L171-L218)).

## 4. The row menu

The pane inherits the Log dialog's menu. `hideUnimplementedCommands` switches on *Blame previous*
and *Show log*, then restricts the menu to this list
([BLL#L33-L49](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/LogListBlameAction.cpp#L33-L49)).
The base menu builder shows them in this order, with labels from
[LLC](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseLoglistCommon.rc2#L45-L162)
([GLB#L1741-L2338](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L1741-L2338)).

With **one row** selected:

| Item | What it does ([BLL#L74-L170](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/LogListBlameAction.cpp#L74-L170)) |
|---|---|
| Blame previous revision (a submenu of parents for merges) | `TortoiseGitProc /command:blame /path:<file in parent> /endrev:<parent>`. It opens a new blame window (§6). It has no `/line`, unlike the gutter's version. |
| Show changes as unified diff | `/command:diff … /startrev:<parent> /endrev:<commit> /unified` for this file |
| &Compare with previous revision | the same diff in the configured diff viewer (Shift for the alternative viewer) |
| Show log... | the Log dialog for the file, starting at this commit (`/rev` = `/endrev` = commit) |
| &Browse repository | the repository browser at this commit |
| Sw&itch/Checkout to this... | not offered for the HEAD commit |
| Create Br&anch / Create &Tag at this version... | the branch or tag dialog |
| E&xport this version... | the export dialog |
| Copy to clipboard ▸ | full info, full without paths, hash, authors (full/name/email), subjects, messages |
| Show branches this commit is &on | the "commit is on refs" dialog |

With **several rows** selected, the single-row block is skipped. What's left is *Copy to
clipboard*, which covers all selected rows
([GLB#L2300-L2321](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L2300-L2321)).
A root commit has no parents, so it gets no blame-previous or compare items.

**Which file and which parent.** The actions use the path as the chosen commit's lines record it.
If the commit owns no lines, they use the blamed path. They pick only parents whose version of the
file was *modified* (not added), and use the old name when the file was renamed
([BLL#L172-L292](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/LogListBlameAction.cpp#L172-L292)).
The blame list's `GetParentHashes(GitRevLoglist*)` takes a different parameter type from the base
virtual `GetParentHashes(GitRev*)`
([GLBH#L485](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.h#L485)),
so it does not override it. The menu therefore seems to list **all** parents of a merge, while the
action counts only the filtered parents. For merges, the submenu entry you pick and the parent
that gets used may not match **(derived, not observed)**.

**Double-click** runs the base handler, which does nothing unless the global setting
`DiffByDoubleClickInLog` (default off) is set. With the setting on, it runs *Compare with previous
revision*
([GLB#L2760-L2767](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L2760-L2767),
[GLB#L2494-L2507](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/GitLogListBase.cpp#L2494-L2507)).
Double-click never re-blames.

## 5. For comparison, the gutter's line menu

Right-clicking a gutter line gives *Blame previous revision*, *Compare with previous revision*,
*Show log*, *Copy SHA-1 to clipboard* and *Copy log to clipboard*
([BV#L324-L453](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L324-L453)).
Here the parents are filtered to those that had the file. The two copy items copy the **list's
selected rows** (`GetLogList()->CopySelectionToClipBoard`), not the clicked line directly
([BV#L514-L520](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L514-L520)).
They get the right commit only because the right-click has just chosen it and selected its row
(§2.1) **(derived)**.

## 6. After "Blame previous revision"

Both versions go through `TortoiseGitProc /command:blame`, which calls `LaunchTortoiseBlame`. That
starts a **new `TortoiseGitBlame.exe` process** with `/path:<file in parent> /rev:<parent>`, plus
`/line:<original line number>` when started from the gutter
([BV#L459-L477](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L459-L477),
[BLL#L86-L104](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/LogListBlameAction.cpp#L86-L104),
[BC#L25-L34](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/Commands/BlameCommand.cpp#L25-L34),
[AU#L712-L729](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/AppUtils.cpp#L712-L729)).
TortoiseGitBlame is a single-document app with no reuse of a running instance
([APP#L132-L168](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlame.cpp#L132-L168)).

- **The old window and its list are untouched.** They keep the old blamed revision and whatever
  was chosen.
- **The new window's list** is built from scratch for the parent (§1). Complete mode gives the
  file's history from the parent down, so the commit you blamed "past" and everything newer are
  not in it. No row is chosen. `/line` only moves the caret to that line and selects its text
  (`GotoLine`); it doesn't choose the line's commit
  ([BD#L284-L287](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L284-L287)).
- There is no back/forward between blamed revisions. Each step is another window **(derived)**.

## 7. Where the manual disagrees with the code

The manual says that hovering darkens all lines of the same revision, and that clicking also
lightly highlights other revisions by the same author
([DOC#L39-L53](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/doc/source/en/TortoiseGit/tgit_dug/dug_blame.xml#L39-L53)).
The code does neither. Hover shades one gutter cell, and there is no same-author colour: the
`m_mouseauthorcolor` colour is computed but never painted, and `m_selectedrevcolor` is only an input to the chosen-line colour (§2.3). The
manual calls the diff item "Show changes", but the code's label is "Compare with previous
revision"
([BRC#L561-L564](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Resources/TortoiseGitBlameENG.rc#L561-L564)).

## 8. Open points

- How `--follow` changes which merges are listed (§1.2). This is git behaviour, not TortoiseGit.
- Which pane caption is actually shown, "Git Log" or "Git Revision List" (§3).
- The merge-parent mismatch in the row menu (§4), and right-click un-choosing (§2.1). Both are
  readings of the code that were not observed in a running program.
