#!/bin/sh
# PROTOTYPE — throwaway. Makes a repository for trying the Add worktree dialog
# (prototype_add_worktree.rs, "Add worktree dialog: prototype", aquamoth/parterre#158): run
# `sh prototype_add_worktree_demo.sh <folder>`, then `parterre <folder>/demo`.
#
# On top of prototype_operation_menus_demo.sh:
# - docs/readme and docs/readme-2: two free local branches on one commit.
# - feature/shared: pushed and level with origin/feature/shared (the remote is that branch).
# - demo.worktrees/fix-typo: a folder with a file in it, so fix/typo's suggestion is taken.
# - release: checked out in demo.worktrees/release, and level with origin/release.
set -e
root=${1:?usage: prototype_add_worktree_demo.sh <folder>}
here=$(cd "$(dirname "$0")" && pwd)
rm -rf "$root"
sh "$here/prototype_operation_menus_demo.sh" "$root" >/dev/null
cd "$root"
root=$(pwd)
cd demo
f() { echo "$2" >> "$1"; git add "$1"; git commit -q -m "$3"; }
git switch -q -c docs/readme main && f README.md "Read me" "Write a readme"
git branch -q docs/readme-2
git switch -q -c feature/shared main && f shared.txt a "Shared work" && git push -q -u origin feature/shared
git switch -q main
mkdir -p ../demo.worktrees/fix-typo && echo "left over" > ../demo.worktrees/fix-typo/notes.txt
echo "made $root/demo"
