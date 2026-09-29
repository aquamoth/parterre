#!/bin/sh
# PROTOTYPE — throwaway. Makes a small repository for trying the operation menus
# (prototype_operation_menus.rs): run `sh prototype_operation_menus_demo.sh <folder>`, then
# `parterre <folder>/demo`.
#
# - main: the open worktree, level with origin/main.
# - feature/login: pushed, then amended, so it has diverged from origin/feature/login.
# - feature/search: ahead of origin/feature/search (a plain push).
# - fix/typo: no upstream.
# - release: checked out in the worktree demo.worktrees/release.
# - origin/feature/remote-only: only on the remote.
# - a detached worktree demo.worktrees/experiment, one commit on no branch.
set -e
root=${1:?usage: prototype_operation_menus_demo.sh <folder>}
mkdir -p "$root"
cd "$root"
root=$(pwd)
git init -q --bare -b main origin.git
git init -q -b main demo
cd demo
git config core.autocrlf false
c() { echo "$1" >> file.txt; git add file.txt; git commit -q -m "$1"; }
c "Initial commit"
c "Add the parser"
git remote add origin ../origin.git
git push -q -u origin main
git switch -q -c release
c "Release notes"
git push -q -u origin release
git switch -q main
c "Parse numbers"
git switch -q -c feature/login
c "Login form"
git push -q -u origin feature/login
git commit -q --amend -m "Login form, reworked"
git switch -q main
git switch -q -c feature/search
c "Search box"
git push -q -u origin feature/search
c "Search results"
git switch -q main
git switch -q -c feature/remote-only
c "Someone else's work"
git push -q origin feature/remote-only
git switch -q main
git branch -q -D feature/remote-only
c "Parse strings"
git switch -q -c fix/typo
c "Fix a typo"
git switch -q main
git push -q origin main
git worktree add -q ../demo.worktrees/release release
git worktree add -q --detach ../demo.worktrees/experiment main~1
cd ../demo.worktrees/experiment
c "An experiment"
echo "made $root/demo"
