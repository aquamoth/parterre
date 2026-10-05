#!/usr/bin/env bash
# Makes a repository with a worktree stuck in each way git can stop, for trying parterre's
# stuck banner and conflicted files by hand (#299). The main worktree stays clean, with
# branches that conflict when merged, rebased or cherry-picked from parterre itself.
#
#   scripts/stuck-demo.sh [DIR]      (default /tmp/stuck-demo; DIR is deleted first)
#   cargo run -- DIR/repo
#
# DIR/README.txt says what each worktree holds and what to try. Run it again to start over.
set -euo pipefail

out=${1:-/tmp/stuck-demo}
case "$out" in
  /|"$HOME"|"$HOME/") echo "refusing to delete $out" >&2; exit 1 ;;
esac
rm -rf "$out"
mkdir -p "$out/tools" "$out/try"
out=$(cd "$out" && pwd)
repo="$out/repo"

g() { git -C "$repo" "$@"; }
# In a worktree; failures are what we are after.
w() { local dir=$1; shift; git -C "$out/$dir" "$@" >/dev/null 2>&1 || true; }
put() { mkdir -p "$(dirname "$1")"; printf "$2" > "$1"; }
# A submodule's commit, straight into the index: no submodule needed for its conflict.
# Commit right after, without `git add -A`, which would drop it again.
gitlink() { g update-index --add --cacheinfo "160000,$2,$1"; }
commit() { g add -A && g commit -q -m "$1"; }

# Merge tools for trying the flow. Neither is set as merge.tool, so parterre's picker shows.
cat > "$out/tools/slow-merge" <<'T'
#!/bin/sh
# Takes their side after 3 seconds, as if the user resolved it by hand.
sleep 3; cp "$2" "$1"
T
cat > "$out/tools/marker-merge" <<'T'
#!/bin/sh
# Saves the file with its conflict markers still in it: git stages it anyway.
sleep 2; touch "$1"
T
chmod +x "$out/tools/"*

git init -q -b main "$repo"
g config user.name "Demo"
g config user.email "demo@example.com"
g config commit.gpgsign false
g config mergetool.slow-merge.cmd "$out/tools/slow-merge \"\$MERGED\" \"\$REMOTE\""
g config mergetool.slow-merge.trustExitCode true
g config mergetool.marker-merge.cmd "$out/tools/marker-merge \"\$MERGED\""
g config mergetool.marker-merge.trustExitCode true
g config mergetool.keepBackup false
if command -v code >/dev/null; then
  g config mergetool.vscode.cmd 'code --wait --merge "$REMOTE" "$LOCAL" "$BASE" "$MERGED"'
fi
# A hook that refuses commits in any worktree holding a `.refuse-commit` file.
cat > "$repo/.git/hooks/prepare-commit-msg" <<'H'
#!/bin/sh
if [ -e "$(git rev-parse --show-toplevel)/.refuse-commit" ]; then
  echo "prepare-commit-msg: refusing (delete .refuse-commit to let it through)" >&2
  exit 1
fi
H
chmod +x "$repo/.git/hooks/prepare-commit-msg"

# The common base: one file for each kind of conflict.
cd "$repo"
put text.txt 'one\ntwo\nthree\n'
put deleted-by-them.txt 'base\n'
put deleted-by-us.txt 'base\n'
put image.bin 'PNG\0base'
put guide 'the guide\n'
put list.txt 'one\ntwo\nthree\n'
put old.txt 'old\n'
put settings.ini 'colour = blue\nsize = 3\n'
ln -s target-base link
g add -A
gitlink sub 1111111111111111111111111111111111111111
g commit -q -m "Base"
base=$(g rev-parse HEAD)

# feature: changes every one of them its way.
g switch -q -c feature
put text.txt 'one\nTWO (feature)\nthree\n'
g rm -q deleted-by-them.txt
put deleted-by-us.txt 'feature\n'
put image.bin 'PNG\0feature'
put both-added.txt "feature's\n"
g rm -q guide
put guide/index.md 'the guide, as a folder\n'
ln -sfn target-feature link
g add -A
gitlink sub 2222222222222222222222222222222222222222
# Not `commit`: its `git add -A` would drop the submodule, which has no folder on disk.
g commit -q -m "Feature: every kind of change"

# main: changes them another way.
g switch -q main
put text.txt 'one\n2 (main)\nthree\n'
put deleted-by-them.txt 'main\n'
g rm -q deleted-by-us.txt
put image.bin 'PNG\0main'
put both-added.txt "main's\n"
put guide 'the guide, longer\n'
put list.txt 'one\n2 (main)\nthree\n'
g rm -q old.txt
ln -sfn target-main link
g add -A
gitlink sub 3333333333333333333333333333333333333333
g commit -q -m "Main: every kind of change"

# topic: three commits from the base, the second conflicting with main.
g switch -q -c topic "$base"
put a.txt 'topic a\n'; commit "Topic: add a"
put list.txt 'one\nTWO (topic)\nthree\n'; g mv old.txt new.txt; commit "Topic: edit list, rename old"
put c.txt 'topic c\n'; commit "Topic: add c"

# fixes: three fixes from the base; the second conflicts with main.
g switch -q -c fixes "$base"
put one.txt 'one\n'; commit "Fix one"
put text.txt 'one\n2 (fixed)\nthree\n'; commit "Fix two"
put three.txt 'three\n'; commit "Fix three"
# picked-already: main with the second fix already in, so picking it comes out empty.
g switch -q -c picked-already main
put one.txt 'one\n'; commit "Fix one"
g switch -q main

# For trying failures from parterre in the clean main worktree.
g branch conflicts-with-main feature
g switch -q -c merges-cleanly main
put notes.txt 'merges cleanly\n'; commit "A change main doesn't have"
g switch -q main

# One worktree per way to be stuck, each on a branch of its own.
add() { g worktree add -q -b "$2" "$out/$1" "$3"; }

add merge merge-here main
w merge merge feature

add rebase rebase-here topic
w rebase rebase main

add cherry-pick pick-here main
w cherry-pick cherry-pick fixes~2 fixes~1 fixes

add empty-pick empty-pick-here picked-already
put "$out/empty-pick/text.txt" 'one\n2 (fixed)\nthree\n'
w empty-pick commit -qam "Fix two, already"
w empty-pick cherry-pick fixes~1 fixes

add revert revert-here main
put "$out/revert/r1.txt" 'r1\n'; w revert add -A; w revert commit -qm "Add r1"
put "$out/revert/r2.txt" 'r2\n'; w revert add -A; w revert commit -qm "Add r2"
touch "$out/revert/.refuse-commit"
w revert revert --no-edit HEAD HEAD~1

add stash-pop stash-here main
put "$out/stash-pop/settings.ini" 'colour = green\nsize = 3\n'
w stash-pop stash push -q -m "stash-pop worktree's change"
put "$out/stash-pop/settings.ini" 'colour = red\nsize = 3\n'
w stash-pop commit -qam "Red"
w stash-pop stash pop

add edit edit-here topic
GIT_SEQUENCE_EDITOR="sed -i 1s/^pick/edit/" w edit rebase -i HEAD~2

add bisect bisect-here topic
w bisect bisect start HEAD "$base"

# Operations in a terminal, for the banner's wait.
cat > "$out/try/quick-terminal-rebase.sh" <<EOF
#!/bin/sh
# A rebase that's in progress for under a second: the banner should never show.
cd "$repo" && git rebase -q --force-rebase --exec "sleep 0.2" main merges-cleanly && git switch -q main
EOF
cat > "$out/try/slow-terminal-rebase.sh" <<EOF
#!/bin/sh
# A rebase that's in progress for about 6 seconds: the banner shows after 1.5 s, then goes.
cd "$repo" && git rebase -q --force-rebase --exec "sleep 6" main merges-cleanly && git switch -q main
EOF
chmod +x "$out/try/"*.sh

cat > "$out/README.txt" <<EOF
Stuck worktrees for trying parterre (#299). Open $repo, and go to another worktree from a
node's menu (Go to worktree ▸ …). Only the open worktree's state raises the banner.

  repo         main, clean. Merge conflicts-with-main into it, rebase it onto that, or
               cherry-pick "Fix two" from fixes, to make a conflict from parterre itself.
               merges-cleanly merges without one.
  merge        merge of feature, stopped on every kind of conflict: content (UU),
               both added (AA), deleted by them (UD), deleted by us (DU), binary,
               symlink, submodule, and a file against a folder (guide~HEAD).
  rebase       rebase of topic onto main, stopped at 2/3: a content conflict and a
               rename against a delete (DU).
  cherry-pick  cherry-pick of three fixes, stopped at 2/3 on a conflict.
  empty-pick   cherry-pick of two fixes, stopped because the first is already there.
  revert       revert of two commits, stopped by the prepare-commit-msg hook.
               Delete .refuse-commit there to let commits through.
  stash-pop    a conflicted stash pop: conflicted files, no operation in progress.
  edit         interactive rebase stopped at an edit: in progress, nothing conflicted.
  bisect       a bisect in progress.

Merge tools: none is configured, so the first "Open in merge tool" shows the picker.
  slow-merge    takes their side after 3 s
  marker-merge  saves the file with its markers left in (git stages it anyway)
  vscode        VS Code's merge editor, if code is installed
To try a configured tool:  git -C $repo config merge.tool slow-merge

In a terminal, for the banner's wait (run with $repo open in parterre, main worktree):
  $out/try/quick-terminal-rebase.sh   in progress for under a second: no banner
  $out/try/slow-terminal-rebase.sh    in progress for ~6 s: banner after 1.5 s, then gone

Start over:  scripts/stuck-demo.sh $out
EOF

echo "Made $out:"
for d in repo merge rebase cherry-pick empty-pick revert stash-pop edit bisect; do
  printf '  %-12s %s\n' "$d" "$(git -C "$out/$d" status --short --branch | head -1)"
done
echo "Open it with: cargo run -- $repo    (what to try: $out/README.txt)"
