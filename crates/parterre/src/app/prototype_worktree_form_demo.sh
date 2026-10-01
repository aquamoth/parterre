#!/bin/sh
# PROTOTYPE — throwaway (#163). Makes a repository for trying the branch form in worktree mode:
# `sh prototype_worktree_form_demo.sh <folder>`, then `parterre <folder>/demo --worktrees`.
# - main: three commits; fix/typo and docs/readme are free local branches.
# - origin/feature/remote-only: a remote branch with no local branch.
# - release: checked out in demo.worktrees/release.
# - demo.worktrees/fix-typo: a non-empty folder, so fix/typo's suggested folder is taken.
set -e
root=${1:?usage: prototype_worktree_form_demo.sh <folder>}
rm -rf "$root" && mkdir -p "$root" && cd "$root" && root=$(pwd)
git init -q --bare origin.git
git init -q -b main demo && cd demo
git remote add origin "$root/origin.git"
f() { echo "$2" >> "$1"; git add "$1"; git commit -q -m "$3"; }
f a.txt 1 "Start the project"
f a.txt 2 "Parse numbers in the input"
git push -q -u origin main
git switch -q -c feature/remote-only && f r.txt r "Remote work" && git push -q origin feature/remote-only
git switch -q main && git branch -q -D feature/remote-only
git switch -q -c release && f rel.txt 1 "Prepare release" && git switch -q main
f a.txt 3 "Fix a typo in the docs"
git branch -q fix/typo && git branch -q docs/readme
git worktree add -q ../demo.worktrees/release release
mkdir -p ../demo.worktrees/fix-typo && echo "left over" > ../demo.worktrees/fix-typo/notes.txt
echo "made $root/demo"
