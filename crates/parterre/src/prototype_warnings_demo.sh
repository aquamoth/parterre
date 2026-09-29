#!/bin/sh
# PROTOTYPE — throwaway. Makes a repository with one case for every warning in
# prototype_warnings.rs ("Warnings before losing work: prototype", aquamoth/parterre#147):
# run `sh prototype_warnings_demo.sh <folder>`, then `parterre <folder>/demo`.
#
# On top of prototype_operation_menus_demo.sh:
# - fix/typo: `-d` refuses, one commit lost. spike/many: 25 commits lost.
# - old/release-copy: `-d` refuses, nothing lost (release reaches it). feature/done: merged.
# - origin/feature/remote-only: lost on deleting it. origin/feature/search: feature/search
#   loses its upstream. origin/main: the remote's default branch. origin/feature/stale: moved
#   on the remote since the last fetch.
# - worktrees: release with uncommitted changes (and an ignored file), clean on feature/clean
#   (2 unpushed commits), experiment (detached, 1 lost commit), gone (folder deleted outside
#   git, detached, 1 lost commit), big (40 changed files), conflict (a rebase stopped on a
#   conflict).
# - the open worktree has changes to file.txt and an untracked notes.txt: switching to
#   fix/typo or feature/notes is blocked.
# - force push: feature/login (amended, fine); feature/shared (a colleague's commit never on
#   the branch: git refuses); feature/shared-picked (same, but the colleague's commit
#   cherry-picked onto it).
set -e
root=${1:?usage: prototype_warnings_demo.sh <folder>}
here=$(cd "$(dirname "$0")" && pwd)
rm -rf "$root"
sh "$here/prototype_operation_menus_demo.sh" "$root" >/dev/null
cd "$root"
root=$(pwd)
cd demo
c() { echo "$1" >> file.txt; git add file.txt; git commit -q -m "$1"; }
f() { echo "$2" >> "$1"; git add "$1"; git commit -q -m "$3"; }
colleague() { # colleague <branch> <file> <message>: someone else pushes to <branch>
  rm -rf "$root/other"
  git clone -q "$root/origin.git" "$root/other"
  (cd "$root/other" && git switch -q "$1" && echo x >> "$2" && git add "$2" && git commit -q -m "$3" && git push -q origin "$1")
  rm -rf "$root/other"
}

git remote set-head origin main
printf 'build/\n' >> .git/info/exclude

git switch -q -c spike/many main
i=1; while [ $i -le 25 ]; do f spike.txt "$i" "Spike step $i"; i=$((i + 1)); done
git branch -q old/release-copy release
git switch -q -c feature/done main && f done.txt done "Finished feature"
git switch -q main && git merge -q --no-ff -m "Merge feature/done" feature/done && git push -q origin main
git switch -q -c feature/notes main && f notes.txt "tracked" "Add notes"
git switch -q -c feature/stale main && f stale.txt a "Stale work" && git push -q -u origin feature/stale

# Force pushes.
git switch -q -c feature/shared main && f shared.txt a "Shared start" && git push -q -u origin feature/shared
colleague feature/shared colleague.txt "Colleague's fix"
git fetch -q origin
git commit -q --amend -m "Shared start, reworked"
git switch -q -c feature/shared-picked main && f picked.txt a "Picked start" && git push -q -u origin feature/shared-picked
colleague feature/shared-picked colleague.txt "Colleague's fix to picked"
git fetch -q origin
git commit -q --amend -m "Picked start, reworked"
git cherry-pick origin/feature/shared-picked >/dev/null

# Worktrees.
git switch -q -c feature/clean main && f clean.txt a "Clean one" && f clean.txt b "Clean two"
git switch -q main
git worktree add -q ../demo.worktrees/clean feature/clean
git worktree add -q --detach ../demo.worktrees/gone main
(cd ../demo.worktrees/gone && echo gone >> file.txt && git commit -qam "Work in a folder that's gone")
rm -rf ../demo.worktrees/gone
git branch -q spike/big-refactor main
git worktree add -q ../demo.worktrees/big spike/big-refactor
git switch -q main
(cd ../demo.worktrees/big && echo changed >> file.txt && mkdir -p src &&
  i=1; while [ $i -le 39 ]; do echo "new" > "src/module_$i.rs"; i=$((i + 1)); done)
(cd ../demo.worktrees/release && echo edit >> file.txt && echo staged > staged.txt && git add staged.txt &&
  echo draft > draft.txt && mkdir -p build && echo out > build/app.bin)
git switch -q -c feature/conflict main && echo mine > conflict.txt && git add conflict.txt && git commit -q -m "Mine"
git switch -q main
git switch -q -c conflict-base main && echo theirs > conflict.txt && git add conflict.txt && git commit -q -m "Theirs"
git switch -q main
git switch -q main
git worktree add -q ../demo.worktrees/conflict feature/conflict
(cd ../demo.worktrees/conflict && git rebase -q conflict-base >/dev/null 2>&1 || true)

# Moved on the remote after the last fetch.
colleague feature/stale stale.txt "Newer stale work (colleague)"

# The open worktree: local changes that block some switches.
git switch -q main
echo "local edit" >> file.txt
echo "my notes" > notes.txt
echo "made $root/demo"
