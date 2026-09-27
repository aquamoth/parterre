# Listing a file's history with the git CLI

Research for [#110](https://github.com/aquamoth/parterre/issues/110), part of the map
[#108](https://github.com/aquamoth/parterre/issues/108) (the blame window's **history pane**). The
question: how parterre can list the commits that changed a file, as of a **blamed revision**, using
only the git command line.

Method: git's documentation and source at a pinned commit, TortoiseGit's source for what
TortoiseGitBlame does, and experiments in throwaway repositories built with the git CLI. Every
experiment ran with two gits: the system **git 2.43.0** (Ubuntu 24.04) and **git 2.56.0-rc2**
(`0f8e75ab`, today's `master`, built from source), because `--follow` changed between them. Where a
statement is my reading of code rather than something the code or docs say outright, it is marked
**(derived)**. Recommendations are kept apart from the facts, in [§6](#6-recommendations).

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT | git `master` @ `0f8e75ab` (2.56.0-rc2 + 1) | https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/ |
| RLO | `GIT/Documentation/rev-list-options.adoc` (history simplification) | (same base) |
| TG | TortoiseGit `master` @ `acc10fc2` (same pin as the [log diffs research](https://github.com/aquamoth/parterre/blob/research/tortoisegit-log-diffs/docs/research/tortoisegit-log-diffs.md)) | https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/ |

Scenario repositories (all built by scripts under `/tmp`, described in the [appendix](#appendix-the-experiments)):
**renames** (rename with an edit, a pure rename, a rewrite-and-rename below 50% similarity, a copy,
a new file reusing an old name), **merges** (both sides changed the file; a merge identical to one
side; an `-s ours` merge that discards a side's change; an evil merge), **nonlinear** (a rename on
one branch while another branch keeps editing the old name), and **working tree** cases.

---

## Summary

- **Plain `git log <rev> -- <path>`** lists the commits that changed that path, simplified the way
  `git blame` also walks. It stops at a rename, so it never lists the commits from before one.
  It streams, and commit-graph Bloom filters make it fast.
- **`--follow`** goes past renames, but it also follows copies, never lists merge commits (not even
  the evil merges blame names), turns off history simplification, and can't use Bloom filters.
  Its results depend on the git version: up to 2.55 it misses side-branch edits made under the old
  name. In 2.56.0-rc2 that's fixed, but I found a new case where it drops everything before a
  rename (reproduced in 12 commits).
- **Blame's own chain** (`filename`, `previous`) follows renames correctly on each line of history,
  but it only names commits that still have lines in the file. Listing the full history by walking
  `previous` would take one blame per step.
- For a file that was renamed, most of blame's commits can be missing from plain `git log -- <path>`.
  In git.git, 61 of the 163 commits blame names for `builtin/rev-list.c` are missing, and 48 of 53
  for `Documentation/git-log.adoc`. With no rename in the history, blame's commits were always a
  subset of plain log's (0 missing in both cases tested).
- **Working tree:** show a row when `git diff --quiet HEAD -- <path>` says the file differs.
  Blame's uncommitted lines aren't a reliable signal: a change that only deletes lines leaves none.
  A staged rename and an unfinished merge change which revisions and path the log must be given.
- **Cost:** git flushes each commit to a pipe, so rows arrive as they are found, with the first
  row in milliseconds when the file changed recently. The run always ends with a walk of the whole
  history. On git.git (82k commits in HEAD) that takes 0.25–0.7 s, or 40–55 ms with Bloom filters.
  `--follow` takes 0.45–1.9 s. Closing the pipe doesn't stop git until it next writes, so
  cancelling means killing the process.
- **Snapshot:** every commit the log lists is an ancestor of the blamed revision, so normally
  `Repo::lookup` finds all of them. The exceptions are a snapshot reloaded after history changed,
  and an unfinished merge whose `MERGE_HEAD` is on no ref.

---

## 1. `git log -- <path>` vs `--follow` vs blame's chain

### 1a. What each one does

**Plain `git log <rev> -- <path>`.** It selects commits "modifying the given <paths>"
([RLO#L407-L408](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/rev-list-options.adoc#L407-L408)).
The path is matched literally at every commit, so a rename shows up as the file being created
(the rename commit is the oldest row), and a later, unrelated file at an old name is a different
path.

**`--follow`.** "Continue listing the history of a file beyond renames (works only for a single
file)"
([git-log.adoc#L30-L32](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/git-log.adoc#L30-L32)).
The docs leave several things out:

- **Copies too.** When the followed path is created, git looks for its source with
  `find_copies_harder` set
  ([tree-diff.c#L631-L637](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/tree-diff.c#L631-L637)),
  which turns rename detection into copy detection
  ([diff.c#L5267-L5268](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/diff.c#L5267-L5268)).
  It then takes an `R` *or* `C` pair as the new path
  ([tree-diff.c#L653-L667](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/tree-diff.c#L653-L667)).
  It honours `-M<n>` (`rename_score`, same lines). Verified: `-M20%` followed a rename at 21%
  similarity that the default 50% missed.
- **No history simplification.** With `--follow`, git doesn't prune by path at all ("Can't prune
  commits with rename following: the paths change..")
  ([revision.c#L3198-L3202](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/revision.c#L3198-L3202)).
  Instead it walks every commit and decides from each commit's diff, because "rename following
  need[s] diffs"
  ([revision.c#L3186-L3190](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/revision.c#L3186-L3190)).
- **No merges.** Unless a `--diff-merges` variant is given, "merge commits will not show a diff
  ... nor will they match search options"
  ([git-log.adoc#L130-L135](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/git-log.adoc#L129-L133)).
  So a merge never produces a diff for `--follow` to match. **(derived; verified in §2)**
- **One path at a time, and until 2.56 one global path.** Up to 2.55 the followed name lived in a
  single pathspec that the first rename found replaced for the rest of the walk. The 2.56 commit
  [`304812ed`](https://github.com/git/git/commit/304812ed33dec7ea7e68928feab38199d796ea25)
  ("log: improve --follow following renames for non-linear history", first in v2.56.0-rc0)
  explains that "the end result isn't really defined". It adds a commit → path map
  ([log-tree.c#L1279-L1313](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/log-tree.c#L1279-L1313)).
  2.56.0 isn't released yet. The newest tag is v2.56.0-rc2.
- **`log.follow`.** If the user has set `log.follow=true`, plain `git log -- <one path>` acts as
  `--follow`
  ([config/log.adoc#L53-L56](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/config/log.adoc#L53-L56),
  [builtin/log.c#L817-L819](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/builtin/log.c#L815-L819)).
  Verified: `--no-follow` on the command line overrides it.

**Blame's chain.** Blame follows whole-file renames and "there is no option to turn the
rename-following off"
([git-blame.adoc#L26-L28](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/git-blame.adoc#L26-L28)).
It checks for a rename only where the path is missing from a parent, one parent at a time, with
plain rename detection: renames only, the default threshold, and no option to change it
([blame.c#L1422-L1459](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/blame.c#L1422-L1459)).
If a parent has the identical blob, blame passes all the blame to that parent and doesn't look at
the others
([blame.c#L2435-L2459](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/blame.c#L2435-L2459)).
That is the same rule as log's default simplification (§2). In `--line-porcelain` output every
line has `filename` (its path in the commit it is blamed on) and, when there is one, `previous
<commit> <path>`. parterre already parses both into `Origin.path` and `Origin.previous`
(`crates/parterre-core/src/blame.rs`). The set of commits blame names is exactly the commits that
still own at least one line.

### 1b. Where they disagree

Tested in the scenario repositories. "Lists" means the command prints the commit. For blame, it
means some line is blamed on it.

| Case | `log -- <path>` | `log --follow -- <path>` | `blame` (as parterre runs it) |
|---|---|---|---|
| Rename with an edit (86% similar) | Stops. The rename commit is the oldest row | Continues under the old name | Continues |
| Pure rename (100%) | Stops at it | Continues | Continues. The rename commit owns no lines, so it's not listed |
| Several renames (a→b→c) | Only commits since the newest rename | All of them (2.43). See the 2.56 bug below | All that still own lines |
| Rename plus a rewrite (21% similar) | Stops | Stops, unless `-M20%` | Stops (no threshold option) |
| Copy (`c.txt` copied to `d.txt`) | Starts at the copy | Goes on into `c.txt`'s history (status `C100`) | Starts at the copy. With `-C -C` (parterre's "Across files") lines go into `a.txt`/`b.txt`/`c.txt` commits |
| New, unrelated file at an old name | Listed only when asking for the old name | Same | Not involved |
| Merge that differs from both parents (evil or conflict-resolving merge) | Listed | **Never listed** | Can own lines (evil merge `M4` did) |
| Side change thrown away by an `-s ours` merge | Hidden | **Listed** | Not blamed |
| Rename on main while a side branch edits the old name, merged later | Side edits missing (they touched another path) | **2.43: missing** (walk-order dependent). 2.56: listed | Blamed correctly |

Real repositories:

| File (git.git, git 2.43) | blame's commits | `log` | `--follow` | blame commits missing from `log` | … from `--follow` |
|---|---|---|---|---|---|
| `builtin/rev-list.c` (`rev-list.c` → `builtin-rev-list.c` → `builtin/rev-list.c`) | 163 | 209 | 357 | 61 | 1 (a non-merge: the old-git global-path problem) |
| `Documentation/git-log.adoc` (`git-log-script.txt` → `git-log.txt` → `.adoc`) | 53 | 5 | 120 | 48 | 0 |
| `revision.c` (never renamed) | 504 | 1150 | 782 | 0 | 4 (all merges resolving conflicts) |
| `MainForm.cs` in a private 19,283-commit repository (renamed once, 100%) | 378 | 694 | 1151 | 0 | 0 |

`--follow` lists fewer rows than plain log for `revision.c` because it drops merges. It lists more
for `MainForm.cs`: 1,152 non-merge commits touched the file, and default simplification hides 518
of them (§2), while `--follow` doesn't simplify.

### 1c. A 2.56.0-rc2 `--follow` regression

With 2.56.0-rc2, `git log --follow -- builtin/rev-list.c` in git.git lists 179 commits instead of
357. It stops in March 2009 and never reaches the 2006 rename `rev-list.c → builtin-rev-list.c`,
and 44 of blame's commits go missing. My reading of the new code **(derived)**: each commit stores
the path to follow in each parent, and a later write overwrites an earlier one
([log-tree.c#L1113-L1129](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/log-tree.c#L1113-L1129),
[#L1157-L1181](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/log-tree.c#L1157-L1181)).
When a merge brings in a topic branch whose copy of the file is too different to be matched as a
rename, the topic side keeps the *new* name. The topic commit is walked after the rename commit,
so it overwrites the correct old name on their shared ancestor.

Minimal reproduction, which passes with 2.43 and fails with 2.56.0-rc2
(`/tmp/fh-research/regress3.sh`): `r1` adds `old.txt`, `r2` edits it, and branch `topic` starts
here. `t1` (on `topic`, dated before `r3`) touches another file. `r3` renames to `new.txt`, `r4`
rewrites most of `new.txt`, `r5` merges `topic`, and `r6` edits `new.txt`.

```
git 2.43.0:     log --follow -- new.txt  →  r6 r4 r3 r2 r1
git 2.56.0-rc2: log --follow -- new.txt  →  r6 r4 r3
```

This hasn't been reported upstream from here (see §6).

---

## 2. Merges and history simplification

The rules ([RLO#L503-L545](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/rev-list-options.adoc#L503-L545)):

- **Default.** "Commits are included if they are not TREESAME to any parent ... If the commit was
  a merge, and it was TREESAME to one parent, follow only that parent." A side branch whose result
  the merge didn't keep is never walked.
- **`--full-history`** (without parent rewriting): "always follow all parents of a merge, even if
  it is TREESAME to one of them." A merge is listed when it differs from some parent. With
  `--parents` (parent rewriting), "Merges are always included".
- **`--first-parent`**: "follow only the first parent commit upon seeing a merge commit". In
  `git log` it also makes merges diff against their first parent
  ([RLO#L133-L146](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/rev-list-options.adoc#L133-L146)).
- **`--show-pulls`**: the default plus merges "not TREESAME to the first parent but ... TREESAME to
  a later parent"
  ([RLO#L424-L428](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/rev-list-options.adoc#L424-L428)).
- **`-m`** is documented only as a diff option: "Show diffs for merge commits"
  ([diff-options.adoc#L40-L42](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/Documentation/diff-options.adoc#L39-L42)).
  In the code it also sets `simplify_history = 0`, the same as `--full-history`
  ([diff-merges.c#L35-L40](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/diff-merges.c#L35-L40)).
  **Undocumented, verified below.**

In the **merges** repository, `f.txt` is changed by `m3` (main), `s1` (side1), `s3` (side2), `s4`
(side3, then thrown away by the `-s ours` merge `M3`), and only by merge `M4` (evil). `M1` merged a
side where both sides changed the file. `M2` is identical to its side parent. Both gits gave the
same results:

| Command (`-- f.txt`) | Rows |
|---|---|
| `log` (default) | M4 s3 M1 s1 m3 m1 |
| `log --full-history` (with or without `--parents`) | M4 M3 s4 M2 s3 M1 s1 m3 m1 |
| `log -m` | same as `--full-history` |
| `log --simplify-merges` | M4 M3 s4 s3 M1 s1 m3 m1 |
| `log --show-pulls` | M4 M2 s3 M1 s1 m3 m1 |
| `log --first-parent` | M4 M2 M1 m3 m1 |
| `log --follow` (also with `--full-history`) | s4 s3 s1 m3 m1 (no merges, even `M4`) |
| `log --follow -m` | M4 M4 M3 s4 M2 s3 M1 M1 s1 m3 m1 (merges once per differing parent) |
| `log --follow --first-parent` | M4 M2 M1 m3 m1 |
| `blame` (commits owning lines) | m1 m3 s1 s3 M4 |
| `blame --first-parent` | m1 m3 M1 M2 M4 |

What this shows:

- **The default matches blame.** Every commit blame names is listed, including the evil merge.
  The side change that was thrown away (`s4`) and the merges that only passed a side through (`M2`,
  `M3`) are not. This follows from blame and default simplification sharing the "identical parent
  takes everything" rule (§1a). In real repositories: 0 of blame's commits were missing from
  plain log for the two files that were never renamed (`revision.c`, `MainForm.cs`).
- **`--full-history` and `-m` add noise on real histories.** For git.git's `builtin/rev-list.c`,
  `--full-history` gives 9,334 rows against 209 for the default. Almost all of them are merges that
  differ from one parent only because that parent's branch started before a change.
- **`--follow` with merges is unusable.** On git.git, `--follow -m` lists 8,000–17,000 rows (each
  merge once per differing parent). `--follow --cc` prints a header for all 21,299 merges in the
  history.
- **`--first-parent`** shows merges as the change (`M1`, `M2`) instead of the side commits. That
  matches `blame --first-parent`, which is what TortoiseGitBlame's "Only consider first parents"
  does (it uses a grafts file from `git rev-list --first-parent`;
  [TortoiseGitBlameDoc.cpp](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp),
  see the [log diffs research §8](https://github.com/aquamoth/parterre/blob/research/tortoisegit-log-diffs/docs/research/tortoisegit-log-diffs.md#8-blame-from-the-log-stretch-goal)).

**What TortoiseGitBlame lists.** With "Show complete log" on (the default) and no move/copy
detection across files or first-parent, it runs the Log dialog's query on the file at the blamed
revision. That query uses `--follow` only if "Follow renames" is on, and it is **off** by default
([TortoiseGitBlameDoc.cpp#L270-L282](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameDoc.cpp#L270-L282),
[TortoiseGitBlameView.cpp#L137-L139](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L137-L139),
[OutputWnd.cpp#L120-L131](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L120-L131)).
The query adds `--parents` rather than `--full-history`, so it uses default simplification
([Git.cpp#L1082-L1088](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/Git/Git.cpp#L1082-L1088)).
In the other cases it lists **only the commits blame named**, sorted by date
([OutputWnd.cpp#L133-L141](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/OutputWnd.cpp#L133-L141),
[LogDataVector.cpp#L208-L260](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseProc/LogDataVector.cpp#L208-L260)).
A line whose commit has no row maps to no row, and nothing is selected
([TortoiseGitBlameView.cpp#L1514-L1537](https://github.com/TortoiseGit/TortoiseGit/blob/acc10fc20afe36aabc4afbc5f1af33f31acae32e/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1514-L1537)).

---

## 3. The working tree as the blamed revision

Facts, from the **working tree** experiments (git 2.43):

- `git blame -- <path>` with no revision blames the working tree file. Blame makes a fake commit
  whose parents are `HEAD` and, during a merge, `MERGE_HEAD`
  ([blame.c#L188-L222](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/blame.c#L188-L222)).
  Lines no commit has yet come out as commit `0000…0000`, author "Not Committed Yet", with
  `previous <HEAD> <path in HEAD>`. parterre stores those as `Origin.commit = None`.
- **Uncommitted lines don't tell whether the file changed.** An edit that only deletes lines leaves
  no uncommitted lines, yet `git diff --quiet HEAD -- <path>` exits 1. A staged-only edit counts
  as uncommitted for both blame and `diff HEAD`. Plain `git diff` (index against working tree) says
  it's clean.
- **A staged rename (`git mv e.txt f.txt`, not committed):** blame of `f.txt` follows it, because
  for the working tree blame's rename check diffs `HEAD` against the index
  ([blame.c#L1438-L1439](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/blame.c#L1438-L1439)).
  Every line is attributed to commits under `e.txt`, and there are no uncommitted lines. But
  `git log HEAD -- f.txt` and `git log --follow HEAD -- f.txt` both list nothing, because `f.txt`
  isn't in `HEAD`. A path-limited `git diff -M HEAD -- f.txt` shows `A f.txt`, since limiting to one
  path hides the rename. Only an unlimited `git diff -M --name-status HEAD` (or `git status`) shows
  `R100 e.txt f.txt`. So do blame's `filename` fields.
- **An unstaged rename** (plain `mv`): `blame` fails with "no such path 'g.txt' in HEAD".
  **A deleted file**: `blame` fails ("Cannot lstat").
- **During an unfinished merge**, blame of the working tree gives lines to commits from
  `MERGE_HEAD`'s side (`s9` in `/tmp/fh-research/midmerge.sh`). `git log HEAD -- <path>` doesn't
  list them. `git log HEAD MERGE_HEAD -- <path>` does. The merge had no conflicts, so there were no
  uncommitted lines.
- TortoiseGitBlame has no row for the working tree. Its lines with the null hash simply map to no
  log row **(derived from MapLineToLogIndex above)**. TortoiseGit's Log dialog does have a "Working
  tree changes" row.

---

## 4. Cost: time to first row, streaming, cancelling

**Streaming.** `git log` writes each commit as soon as it decides to show it. When stdout isn't a
regular file it flushes after every commit
([log-tree.c#L1317](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/log-tree.c#L1317),
[write-or-die.c#L19-L41](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/write-or-die.c#L19-L41)).
A pipe gets rows one by one. Options that need the whole walk before the first row:
`--simplify-merges` (always), and `--topo-order` (and so `--graph`) when the repository has no
commit-graph with generation numbers
([revision.c#L3195-L3196](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/revision.c#L3195-L3196)).

**Measurements.** Machine: 32 cores, NVMe, warm cache, best of three, system git 2.43.
"First" is the time to the first row, "total" the time to exit. "Max gap" is the longest silence
between rows or before exit.

git.git (my full clone, 82,307 commits in HEAD, 85,787 in all). An ordinary clone has no
commit-graph at all:

| Command | `builtin/rev-list.c` first / total (rows) | `COPYING` (2 rows) | `t/t4219-log-follow-merge.sh` (1 row, added 2026) |
|---|---|---|---|
| `rev-list HEAD` (bare walk, for scale) | — / 422 ms | | |
| `log --format=%H -- P` | 3 / 321 ms (209) | 174 / 251 ms | 17 / 713 ms |
| `log --follow` | 4 / 942 ms (357) | 603 / 762 ms | 122 / 1919 ms |
| `log --full-history` | 2 / 1245 ms (9334) | 4 / 912 ms (486) | 3 / 3153 ms (113) |
| `log --first-parent` | 3 / 322 ms (112) | 175 / 253 ms | 17 / 712 ms |
| `log --topo-order` | **300** / 315 ms | 246 / 259 ms | **703** / 717 ms |
| `log --simplify-merges` | **1269** / 1292 ms | 948 / 971 ms | **3141** / 3165 ms |
| `blame --line-porcelain` (for comparison) | 294 / 311 ms | 242 / 255 ms | 17 / 18 ms |

The same after `git commit-graph write --reachable --changed-paths` (Bloom filters, 3.5 s to
write):

| Command | `builtin/rev-list.c` | `COPYING` | `t/t4219-…` |
|---|---|---|---|
| `rev-list HEAD` | — / 97 ms | | |
| `log --format=%H -- P` | 2 / **53 ms** | 30 / **43 ms** | 4 / **48 ms** |
| `log --follow` | 4 / 615 ms | 362 / 451 ms | 116 / 1585 ms |
| `log --topo-order` | 3 / 50 ms (streams now) | 31 / 45 ms | 4 / 49 ms |
| `blame` | 110 / 125 ms | 44 / 49 ms | 4 / 5 ms |

Private 19,283-commit repository (1,096 merges, no commit-graph), a file with 694 plain-log rows:
plain log 2 / 112 ms, `--follow` 4 / 284 ms, `--topo-order` 109 / 112 ms, blame 622 / 651 ms.

What the numbers say:

- **The first row is fast when the file changed recently. The last row always needs the whole
  walk.** git can't know that no older commit touches the path, so a file changed only by its
  newest commit still costs a full walk (`t4219`: first row at 17 ms, exit at 713 ms).
- **Bloom filters speed up plain log 6–15×, but barely help `--follow`.** `--follow` diffs every
  commit and prunes nothing (§1a). Its remaining cost is copy detection at the commit that creates
  the file (`t4219`: 1.5 s whether or not there's a commit-graph). Bloom filters exist only if
  something wrote them. Neither the fresh clone nor the private repository had any.
- **Plain history costs about as much as a blame**, and less than one on the private repository
  (112 ms against 651 ms). The pane doesn't have to wait for blame, or blame for it.
- **Larger repositories: not measured.** The instructions for this ticket ruled out cloning a huge
  repository, and a full clone of linux from GitHub stalled anyway. The walk is linear in commits,
  so a repository with ten times git.git's commits should take roughly ten times these totals
  **(extrapolation, unverified)**: seconds for plain log without Bloom filters, well under a second
  with them, and tens of seconds for `--follow`. Time to first row doesn't depend on repository
  size when the file changed recently.

**Cancelling.** When the reader closes the pipe, git notices only at its next write, and then it
exits quietly (`check_pipe`,
[write-or-die.c#L37-L39](https://github.com/git/git/blob/0f8e75abebff0877cae681a3d5ff31ac47f54220/write-or-die.c#L37-L39)).
That can be the whole remaining walk: `git log --follow -- t/t4219-… | head -1` still took 1.66 s.
Stopping git promptly needs `Child::kill` (SIGKILL on Unix, TerminateProcess on Windows) followed by
`wait`. Today parterre's `Git::run`/`run_bytes` collect all output with `Command::output`
(`crates/parterre-core/src/git.rs`), so a streaming, killable reader would be new code.

---

## 5. Mapping the hashes onto parterre's snapshot

Facts:

- `Git::load` builds the snapshot from `git log --stdin` over every ref (notes excluded) plus `HEAD`
  (`crates/parterre-core/src/git.rs`). `Repo::lookup(oid)` looks up the `by_oid` map
  (`crates/parterre-core/src/repo.rs`).
- Every commit `git log <rev> -- <path>` lists is an ancestor of `<rev>`, with or without
  `--follow`. If `<rev>` is in the snapshot, so are all of them. Both commands run the same git
  over the same object store, so shallow boundaries, grafts and `refs/replace` apply to both
  **(derived)**.
- Blame's commits are ancestors of the blamed revision too, except in the working tree during a
  merge, where they can come from `MERGE_HEAD` (§3).

When a hash has no commit in the snapshot:

1. **An unfinished merge of a commit no ref points at** (`git merge <sha>`, or a pull that leaves
   only `FETCH_HEAD`). Blame and `git log HEAD MERGE_HEAD` name commits the snapshot never loaded.
2. **A reloaded snapshot.** parterre reloads when refs change. If history was rewritten (rebase,
   branch deleted, gc), a blame window opened earlier can show commits the new snapshot doesn't
   have. The blamed revision itself may be gone.
3. **Stash entries other than the newest** (`stash@{1}` and older) aren't loaded, since only
   `refs/stash` is. This matters only if blame could start there.

---

## 6. Recommendations

Kept apart from the facts above, for the grilling tickets to accept or overturn.

1. **List with plain `git log`, no `--follow`:**
   `git log --no-follow --no-decorate -z --format=<parterre's LOG_FORMAT fields> <rev> -- <path>`,
   in git's default order and with default simplification. This is TortoiseGitBlame's default. It
   lists every commit blame names when there's no rename (§2), it streams, and it is the cheap,
   Bloom-accelerated path. `--no-follow` keeps a user's `log.follow=true` from silently turning the
   query into `--follow`.
2. **Don't use `--follow`, and don't offer it now.** It drops the evil merges blame names, adds
   copied-from history that blame doesn't have, turns off simplification, costs 2–15× more, and
   gives different answers on different git versions (§1c). If users ask for "complete history
   across renames", that's a later ticket. TortoiseGit has the option, off by default.
3. **Add a row for every commit blame names that the log didn't list**, so that choosing works both
   ways for every line. These are commits from before a rename, and with "Across files" move
   detection, commits of other files. This needs no extra git call: blame's porcelain already has
   hash, author, times, summary and `filename`, and parterre already parses them into `Origin`. Put
   each row in by committer time and show its older path. The alternative is TortoiseGit's
   behaviour, where such lines select nothing. For a renamed file that is most of its lines (48 of
   53 commits for `git-log.adoc`), which would make choosing look broken.
4. **Default simplification, not `--full-history` or `-m`.** Those add thousands of merge rows on
   real histories, and the extra rows are commits whose change didn't survive, so no line points at
   them. Don't use `--topo-order`/`--graph` either (no streaming without a commit-graph). If "Only
   consider first parents" comes to parterre's blame later, the pane should switch to
   `--first-parent` along with it, as in §2.
5. **Working tree:** show a "Working tree changes" row on top when `git diff --quiet HEAD -- <path>`
   exits 1, whether or not blame has uncommitted lines. The `None`-commit lines choose that row. Run
   the log on the file's path in `HEAD`, not the working-tree path: take it from `previous` on the
   uncommitted origin, or from the lines' `filename` when there are none (a staged rename). During a
   merge, also give `MERGE_HEAD` as a revision.
6. **Stream, and kill to cancel.** Read git's stdout line by line on a worker thread and show rows
   as they come, with a visible "still listing" state until git exits. Kill the child (then `wait`)
   when the blame window closes or re-blames. The long silence before exit (§4) is normal and not an
   error.
7. **Rows own their data; the snapshot is only for links.** Keep the `Oid` and the fields git printed
   in each row, and call `Repo::lookup` only for actions that need the graph (such as "show in log
   / graph"). When lookup fails (§5), show the row as usual but mark it visibly as not in the
   current snapshot, and disable the graph actions. Don't drop the row silently.
8. **Question for the human:** report the 2.56.0-rc2 `--follow` regression (§1c) to the git mailing
   list before 2.56.0 ships? It doesn't affect recommendation 2, but the reproduction is ready.

---

## Appendix: the experiments

Scripts and repositories are in `/tmp/fh-research/` (throwaway, not committed). All commits have
fixed dates one minute apart, so walk order is deterministic.

- `renames.sh`: `c01` add `a.txt` · `c02` edit · `c03` rename a→b plus an edit (R086) · `c04` edit
  · `c05` pure rename b→c · `c06` edit · `c07` unrelated · `c08` new unrelated `a.txt` · `c09` copy
  c→d · `c10` edit `d.txt` · `c11` rename c→e plus a rewrite (R021) · `c12` edit `e.txt`.
- `merges.sh`: the history in §2 (`--no-ff` merges; `M3` is `-s ours`; `M4` edits `f.txt` in the
  merge itself).
- `nonlinear.sh`: `n1` add `old.txt` · `n2` edit · branch · `n3` rename to `new.txt` (main) · `n4`,
  `n5` edit `old.txt` (side, dated after `n3`) · `n6` edit `new.txt` · `n7` merge · `n8` edit.
  git 2.43 `log --follow -- new.txt`: n8 n6 n3 n2 n1 (misses n4, n5). git 2.56.0-rc2: n8 n6 n5 n4 n3
  n2 n1. Blame: n1 n2 n4 n5 n6 n8.
- `worktree.sh`, `worktree2.sh`, `midmerge.sh`: the working-tree cases in §3.
- `regress3.sh`: the 2.56.0-rc2 reproduction in §1c.
- `cost.sh` with `ttfr.py`: the timings in §4 (time to first line, total, longest gap).
